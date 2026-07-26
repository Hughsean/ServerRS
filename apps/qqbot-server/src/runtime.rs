use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use personal_secretary::{
    ActionPlannerT, BackfillGapUseCase, ConnectionEndReason, ConnectionEpochId,
    ConservativeThreadSemanticExtractor, DeterministicThreadPlanner, DeterministicThreadPolicy,
    FollowUpUseCase, InboundEventStoreError, InboundIdentityError, MessageSource,
    PersonalSecretaryStoreT, PlannerError, PlannerInput, PlannerOutput, SourceAccountRef,
    ThreadLinkUseCase, ThreadProjectionUseCase, ThreadSemanticExtractorT, ThreadSemanticUseCase,
    build_mysql_action_store, build_mysql_backfill_store, build_mysql_follow_up_store,
    build_mysql_inbound_event_store, build_mysql_memory_store, build_mysql_thread_link_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
use qqbot::napcat::{
    NapCatApiClient, NapCatConnectionObserver, NapCatError, NapCatEvent, NapCatEventHandler,
    NapCatListener,
};
use sea_orm::{ConnectOptions, Database};
use thiserror::Error;
use tokio::sync::watch;

use crate::backfill::{BackfillHandle, spawn_backfill_worker};
use crate::config::AppConfig;
use crate::follow_up_worker::{FollowUpHandle, spawn_follow_up_worker};
use crate::inbound::NapCatInboundMapper;
use crate::ingestion_worker::{IngestionQueue, WorkerReport, spawn_ingestion_worker};
use crate::llm::{LlmThreadSemanticExtractor, OpenAiCompatibleClient};
use crate::qq_open_platform::{OfficialPlatformHandle, spawn_official_platform};
use crate::thread_links::{ThreadLinksHandle, spawn_thread_links_worker};
use crate::thread_projection::{ThreadProjectionHandle, spawn_thread_projection_worker};
use crate::thread_semantics::{ThreadSemanticsHandle, spawn_thread_semantics_worker};
use crate::worker_lifecycle::RuntimeWorkers;

/// 全局关闭期限：所有 Worker 并发关闭的总上限。
/// backfill 内部有 10s `SHUTDOWN_GRACE`，外层必须留出额外余量，否则会抢先中止内部清理。
const WORKER_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(25);

/// 个人秘书入站边界：统一身份后先幂等落库，只有新事件才允许进入后续处理。
struct PersonalSecretaryInboundHandler {
    mapper: NapCatInboundMapper,
    queue: IngestionQueue,
    /// 群白名单。非空时只处理白名单内群的消息；为空表示不启用白名单（放行所有群）。
    group_whitelist: Arc<std::collections::HashSet<i64>>,
}

struct ConnectionObserver {
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    connected: AtomicBool,
    backfill_wake: Option<Arc<BackfillWake>>,
}

/// 供 ConnectionObserver 和 runtime 共享的回补唤醒句柄。
/// 实际持有 `BackfillHandle` 的唤醒通知；避免在观察者中持有整个 JoinHandle。
pub(crate) struct BackfillWake {
    notify: Arc<tokio::sync::Notify>,
}

impl BackfillWake {
    pub(crate) fn new(notify: Arc<tokio::sync::Notify>) -> Self {
        Self { notify }
    }

    pub(crate) fn wake(&self) {
        self.notify.notify_one();
    }
}

impl ConnectionObserver {
    fn was_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl NapCatConnectionObserver for ConnectionObserver {
    async fn connected(&self) -> Result<(), NapCatError> {
        self.store
            .mark_connection_connected(&self.connection_epoch_id)
            .await
            .map_err(|error| NapCatError::Handler(error.to_string()))?;
        self.connected.store(true, Ordering::Release);
        // 重连成功不等于历史已补齐：仅唤醒回补 Worker 尽快扫描 uncertain Gap。
        // Gap 是否转为 verified_complete 由回补用例的证据判定决定，不由重连决定。
        if let Some(wake) = &self.backfill_wake {
            wake.wake();
        }
        Ok(())
    }
}

/// 判断群消息是否应被处理。白名单为空时放行所有群（不启用过滤）。
/// 这是一个纯函数，便于单元测试。
fn should_accept_group_message(group_id: i64, whitelist: &std::collections::HashSet<i64>) -> bool {
    whitelist.is_empty() || whitelist.contains(&group_id)
}

#[async_trait::async_trait]
impl NapCatEventHandler for PersonalSecretaryInboundHandler {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError> {
        match event {
            NapCatEvent::GroupMessage(event) => {
                if !should_accept_group_message(event.group_id, &self.group_whitelist) {
                    tracing::debug!(group_id = event.group_id, "群消息不在白名单内，跳过");
                    return Ok(());
                }
                self.queue.try_enqueue(self.mapper.map_group(event)?)?
            }
            NapCatEvent::PrivateMessage(event) => {
                self.queue.try_enqueue(self.mapper.map_private(event)?)?
            }
            NapCatEvent::GroupMemberIncrease(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                "NapCat 入群通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::GroupMemberDecrease(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                sub_type = %event.sub_type,
                "NapCat 退群通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::Poke(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                target_id = ?event.target_id,
                "NapCat 戳一戳通知已接收；QQBot 业务尚未接入"
            ),
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
    #[error(transparent)]
    Identity(#[from] InboundIdentityError),
    #[error("invalid qqbot configuration: {0}")]
    Config(String),
    #[error("QQ Open Platform runtime failed: {0}")]
    OfficialPlatform(String),
    #[error("LLM runtime failed: {0}")]
    Llm(String),
}

/// 生产入口：使用 OS 信号（Ctrl-C / SIGTERM）触发优雅关闭。
pub async fn run(config: AppConfig, config_dir: std::path::PathBuf) -> Result<(), RuntimeError> {
    run_with_shutdown(config, config_dir, ShutdownSource::OsSignal).await
}

/// 可编程关闭入口：接收一个 `watch::Receiver<bool>`，当收到 `true` 时触发优雅关闭。
///
/// 用于 E2E 集成测试在不依赖 OS 信号的情况下驱动真实服务并验证关闭。与 [`run`]
/// 共享同一套 Worker 装配和监听器循环，区别只在关闭信号来源。
pub async fn run_with_cancellation(
    config: AppConfig,
    config_dir: std::path::PathBuf,
    shutdown: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    run_with_shutdown(config, config_dir, ShutdownSource::Watch(shutdown)).await
}

/// 关闭信号来源：OS 信号或可编程 watch 通道。
enum ShutdownSource {
    OsSignal,
    Watch(watch::Receiver<bool>),
}

impl ShutdownSource {
    /// 等待关闭信号触发。返回后调用方应开始优雅关闭。
    async fn wait(&mut self) {
        match self {
            ShutdownSource::OsSignal => shutdown_signal().await,
            ShutdownSource::Watch(receiver) => {
                // 只在收到 true 时才关闭；忽略 false 变化（如 watch 初始化或其他误触发）。
                loop {
                    if *receiver.borrow() {
                        return;
                    }
                    // changed() 在值变化时返回；若 sender 被 drop 则返回 Err（视为关闭）。
                    if receiver.changed().await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// 聚合所有可选 Worker 句柄，支持并发优雅关闭。
///
/// 装配过程中逐步 push 已启动的 Worker；若后续步骤失败，调用 [`WorkerHandles::shutdown_all`]
/// 回收已启动的任务，避免资源泄漏。正常运行结束时同样调用该方法。
struct WorkerHandles {
    backfill: Option<BackfillHandle>,
    thread_projection: Option<ThreadProjectionHandle>,
    thread_semantics: Option<ThreadSemanticsHandle>,
    thread_links: Option<ThreadLinksHandle>,
    follow_up: Option<FollowUpHandle>,
    official_platform: Option<OfficialPlatformHandle>,
    action_planner: Option<crate::action_planner_worker::ActionPlannerHandle>,
}

impl WorkerHandles {
    fn new() -> Self {
        Self {
            backfill: None,
            thread_projection: None,
            thread_semantics: None,
            thread_links: None,
            follow_up: None,
            official_platform: None,
            action_planner: None,
        }
    }

    /// 取出所有句柄，发出停止信号并用单一全局 deadline 并发回收。
    async fn shutdown_all(self) {
        let mut workers = RuntimeWorkers::new();
        if let Some(handle) = self.backfill {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.thread_projection {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.thread_semantics {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.thread_links {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.follow_up {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.official_platform {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.action_planner {
            workers.push(handle.signal_and_detach());
        }
        workers.shutdown_all(WORKER_SHUTDOWN_DEADLINE).await;
    }
}

async fn run_with_shutdown(
    config: AppConfig,
    config_dir: std::path::PathBuf,
    mut shutdown_source: ShutdownSource,
) -> Result<(), RuntimeError> {
    let mut database_options = ConnectOptions::new(config.database.url.clone());
    database_options.max_connections(config.database.max_connections.max(1));
    let db = Database::connect(database_options).await?;
    tracing::info!("个人秘书数据库已连接");

    let store = build_mysql_inbound_event_store(db.clone());
    let account =
        SourceAccountRef::new(MessageSource::NapCat, config.napcat.self_qq_id.to_string())?;

    // 在启动任何 Worker 之前加载群白名单，避免文件读取失败时遗留 Worker。
    let group_whitelist = Arc::new(
        config
            .whitelist
            .load_groups(&config_dir)
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
    );
    if group_whitelist.is_empty() {
        tracing::info!("群白名单未启用（whitelist.whitelist_file 未配置），将处理所有群消息");
    } else {
        tracing::info!(
            group_count = group_whitelist.len(),
            "群白名单已启用，只处理白名单内群的消息"
        );
    }

    // 装配 Worker：若后续步骤失败，已启动的 Worker 必须被回收。
    // handles 在装配过程中累积，失败时通过 shutdown_all 统一清理。
    let mut handles = WorkerHandles::new();

    let follow_up_use_case = Arc::new(FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        build_mysql_memory_store(db.clone()),
    ));
    if config.follow_up.enabled {
        tracing::info!(
            scan_interval_ms = config.follow_up.scan_interval_ms,
            horizon_secs = config.follow_up.horizon_secs,
            batch_size = config.follow_up.batch_size,
            "结构化记忆维护与承诺提醒调度已启用；通知仅写入 Outbox"
        );
        handles.follow_up = Some(spawn_follow_up_worker(
            Arc::clone(&follow_up_use_case),
            config.follow_up.clone(),
        ));
    } else {
        tracing::info!("承诺提醒调度已禁用（follow_up.enabled=false）");
    }

    // P0 修复：Action Planner 必须在 QQ Open Platform 之前装配，
    // 因为 OwnerCommand 入站时需要 PlannerUseCase 创建 action_run。
    let action_planner_use_case = assemble_action_planner(&mut handles, db.clone(), &config)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "Action Planner 装配失败，正在回收已启动的任务");
            let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
            // 不能在闭包中 await，所以用 spawn
            tokio::spawn(async move {
                handles_to_clean.shutdown_all().await;
            });
            error
        })?;

    if config.qq_open_platform.enabled {
        let inbound = build_mysql_inbound_event_store(db.clone());
        let official_handle = spawn_official_platform(
            config.qq_open_platform.clone(),
            db.clone(),
            inbound,
            Arc::clone(&follow_up_use_case),
            account.clone(),
            action_planner_use_case.clone(),
        )
        .await
        .map_err(|error| RuntimeError::OfficialPlatform(error.to_string()));
        // 装配失败时回收已启动的 follow_up Worker。
        let official_handle = match official_handle {
            Ok(handle) => handle,
            Err(error) => {
                tracing::error!(error = %error, "QQ 开放平台装配失败，正在回收已启动的任务");
                let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
                handles_to_clean.shutdown_all().await;
                return Err(error);
            }
        };
        handles.official_platform = Some(official_handle);
        tracing::info!(
            app_id = config.qq_open_platform.app_id,
            "QQ Open Platform Gateway 与 Owner-only Outbox 投递已启动"
        );
    } else {
        tracing::info!("QQ Open Platform 已禁用（qq_open_platform.enabled=false）");
    }

    // 后续装配（线程投影/语义/关联/回补）可能失败，失败时回收已启动的 Worker。
    if let Err(error) =
        assemble_thread_workers(&mut handles, db.clone(), &config, account.clone()).await
    {
        tracing::error!(error = %error, "Worker 装配失败，正在回收已启动的任务");
        let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
        handles_to_clean.shutdown_all().await;
        return Err(error);
    }

    let backfill_wake = handles
        .backfill
        .as_ref()
        .map(|handle| Arc::new(BackfillWake::new(handle.wake_notifier())));

    let mut backoff = config.napcat.reconnect_initial_secs;
    tracing::info!(
        queue_capacity = config.ingestion.queue_capacity,
        retry_initial_ms = config.ingestion.retry_initial_ms,
        retry_max_ms = config.ingestion.retry_max_ms,
        shutdown_drain_timeout_secs = config.ingestion.shutdown_drain_timeout_secs,
        "个人秘书有界持久化队列配置已生效"
    );

    loop {
        let connection_epoch_id = match store.begin_connection(&account).await {
            Ok(id) => id,
            Err(error) => {
                tracing::error!(error = %error, "begin_connection 失败，正在回收 Worker");
                let handles_to_shutdown = std::mem::replace(&mut handles, WorkerHandles::new());
                handles_to_shutdown.shutdown_all().await;
                return Err(error.into());
            }
        };
        let (queue, mut ingestion_worker) = spawn_ingestion_worker(
            Arc::clone(&store),
            connection_epoch_id.clone(),
            config.ingestion.clone(),
        );
        let handler: Arc<dyn NapCatEventHandler> = Arc::new(PersonalSecretaryInboundHandler {
            mapper: NapCatInboundMapper::new(config.napcat.self_qq_id),
            queue,
            group_whitelist: Arc::clone(&group_whitelist),
        });
        let observer = Arc::new(ConnectionObserver {
            store: Arc::clone(&store),
            connection_epoch_id: connection_epoch_id.clone(),
            connected: AtomicBool::new(false),
            backfill_wake: backfill_wake.clone(),
        });
        let connection_observer: Arc<dyn NapCatConnectionObserver> = observer.clone();
        let listener = NapCatListener::new(
            config.napcat.ws_url.clone(),
            config.napcat.self_qq_id,
            handler,
        )
        .with_connection_observer(connection_observer);

        let (reason, shutting_down) = tokio::select! {
            _ = shutdown_source.wait() => {
                (ConnectionEndReason::ProcessShutdown, true)
            }
            result = listener.run_forward() => {
                match result {
                    Ok(()) => {
                        tracing::warn!("NapCat WebSocket 已断开");
                        (ConnectionEndReason::RemoteClosed, false)
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "NapCat WebSocket 运行失败");
                        let reason = if matches!(error, NapCatError::Handler(_)) {
                            ConnectionEndReason::ObserverRejected
                        } else {
                            ConnectionEndReason::TransportError
                        };
                        (reason, false)
                    }
                }
            }
        };
        drop(listener);
        drain_ingestion_worker(
            &mut ingestion_worker,
            Duration::from_secs(config.ingestion.shutdown_drain_timeout_secs),
            &connection_epoch_id,
        )
        .await;

        let gap_id = match store.finish_connection(&connection_epoch_id, reason).await {
            Ok(gap_id) => gap_id,
            Err(error) => {
                tracing::error!(error = %error, "finish_connection 失败，正在回收 Worker");
                let handles_to_shutdown = std::mem::replace(&mut handles, WorkerHandles::new());
                handles_to_shutdown.shutdown_all().await;
                return Err(error.into());
            }
        };
        if let Some(gap_id) = gap_id {
            tracing::warn!(
                gap_id = %gap_id.as_str(),
                connection_epoch_id = %connection_epoch_id.as_str(),
                reason = reason.as_str(),
                "NapCat 连接结束，已创建待历史回补验证的不确定空窗"
            );
            // 连接结束产生新的 uncertain Gap，唤醒回补 Worker 尽快处理。
            if let Some(wake) = &backfill_wake {
                wake.wake();
            }
        }
        if shutting_down {
            tracing::info!("QQBot NapCat 适配器正在退出");
            let handles_to_shutdown = std::mem::replace(&mut handles, WorkerHandles::new());
            handles_to_shutdown.shutdown_all().await;
            return Ok(());
        }
        if observer.was_connected() {
            backoff = config.napcat.reconnect_initial_secs;
        }

        tracing::info!(backoff_secs = backoff, "等待后重新连接 NapCat");
        tokio::select! {
            _ = shutdown_source.wait() => {
                let handles_to_shutdown = std::mem::replace(&mut handles, WorkerHandles::new());
                handles_to_shutdown.shutdown_all().await;
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
        }
        backoff = backoff
            .saturating_mul(2)
            .min(config.napcat.reconnect_max_secs);
    }
}

/// 装配线程投影、线程语义、跨会话关联和历史回补 Worker。
/// LLM 禁用时的保守 Action Planner：总是返回 NoAction，不执行任何动作。
struct NoopActionPlanner;

#[async_trait::async_trait]
impl ActionPlannerT for NoopActionPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::NoAction {
            reason: "LLM 已禁用，不执行动作规划".into(),
        })
    }
}

/// 装配线程相关 Worker（投影、语义、关联、回补、Action Planner）。
///
/// 这些装配步骤可能因配置或 LLM 客户端构造失败而返回错误；调用方在错误路径
/// 必须回收此前已启动的 Worker（见 `run_with_shutdown` 中的 `shutdown_all`）。
async fn assemble_thread_workers(
    handles: &mut WorkerHandles,
    db: sea_orm::DatabaseConnection,
    config: &AppConfig,
    account: SourceAccountRef,
) -> Result<(), RuntimeError> {
    if config.thread_projection.enabled {
        let policy =
            DeterministicThreadPolicy::new(config.thread_projection.same_conversation_window_secs)
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let use_case = Arc::new(
            ThreadProjectionUseCase::new(
                build_mysql_thread_projection_store(db.clone()),
                DeterministicThreadPlanner::new(policy),
                config.thread_projection.batch_size,
                config.thread_projection.lease_secs,
                config.thread_projection.same_conversation_window_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.thread_projection = Some(spawn_thread_projection_worker(
            use_case,
            config.thread_projection.clone(),
        ));
    } else {
        tracing::info!("确定性线程投影已禁用（thread_projection.enabled=false）");
    }

    if config.thread_semantics.enabled {
        let extractor: Arc<dyn ThreadSemanticExtractorT> = if config.llm.enabled {
            let client = Arc::new(
                OpenAiCompatibleClient::new(&config.llm)
                    .map_err(|error| RuntimeError::Llm(error.to_string()))?,
            );
            tracing::info!(
                model = config.llm.model,
                endpoint_host = client.endpoint_host(),
                max_input_chars = config.llm.max_input_chars,
                max_output_tokens = config.llm.max_output_tokens,
                max_candidates_per_kind = config.llm.max_candidates_per_kind,
                "LLM 有界线程语义提取已启用；模型输出仍须通过来源与领域策略校验"
            );
            Arc::new(
                LlmThreadSemanticExtractor::from_openai(client, config.llm.max_candidates_per_kind)
                    .map_err(|error| RuntimeError::Llm(error.to_string()))?,
            )
        } else {
            tracing::info!("LLM 已禁用；线程语义使用保守零模型提取器");
            Arc::new(
                ConservativeThreadSemanticExtractor::new(config.thread_semantics.max_event_chars)
                    .map_err(|error| RuntimeError::Config(error.to_string()))?,
            )
        };
        let use_case = Arc::new(
            ThreadSemanticUseCase::new(
                build_mysql_thread_semantic_store(db.clone()),
                extractor,
                config.thread_semantics.max_events,
                config.thread_semantics.max_total_chars,
                config.thread_semantics.lease_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.thread_semantics = Some(spawn_thread_semantics_worker(
            use_case,
            config.thread_semantics.clone(),
        ));
    } else {
        tracing::info!("线程类型化语义已禁用（thread_semantics.enabled=false）");
    }

    if config.thread_links.enabled {
        let use_case = Arc::new(
            ThreadLinkUseCase::new(
                build_mysql_thread_link_store(db.clone()),
                config.thread_links.max_events,
                config.thread_links.max_total_chars,
                config.thread_links.lease_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.thread_links = Some(spawn_thread_links_worker(
            use_case,
            config.thread_links.clone(),
        ));
    } else {
        tracing::info!("跨会话线程关联候选已禁用（thread_links.enabled=false）");
    }

    // 装配历史回补：只读 NapCat 客户端 + 回补状态仓储 + 协议无关用例 + 独立 Worker。
    // 分页算法、Gap 完整性判定和 SQL 不在 runtime 内，而分别在领域层与 MySQL 仓储。
    if config.backfill.enabled {
        let backfill_store = build_mysql_backfill_store(db.clone(), config.backfill.lease_secs);
        let napcat_readonly = Arc::new(NapCatApiClient::new(config.napcat.http_base_url.clone()));
        let history_source = Arc::new(
            crate::backfill::napcat_history_source::NapCatHistorySource::new(
                napcat_readonly,
                account.clone(),
                config.napcat.self_qq_id,
            ),
        );
        let budget = config
            .backfill
            .budget()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let use_case = Arc::new(BackfillGapUseCase::new(
            backfill_store,
            history_source,
            budget,
        ));
        let handle = spawn_backfill_worker(use_case, config.backfill.clone());
        tracing::info!(
            page_size = config.backfill.page_size,
            max_pages_per_scope = config.backfill.max_pages_per_scope,
            max_events_per_run = config.backfill.max_events_per_run,
            max_concurrency = config.backfill.max_concurrency,
            lease_secs = config.backfill.lease_secs,
            "历史回补 Worker 已装配，与实时 WebSocket 接收解耦"
        );
        handles.backfill = Some(handle);
    } else {
        tracing::info!("历史回补已禁用（backfill.enabled=false）");
    }

    Ok(())
}

/// 装配 Action Planner。必须在 QQ Open Platform 之前装配，因为 OwnerCommand
/// 入站时需要 PlannerUseCase 创建 action_run。返回 use_case 供 official_platform 注入。
async fn assemble_action_planner(
    handles: &mut WorkerHandles,
    db: sea_orm::DatabaseConnection,
    config: &AppConfig,
) -> Result<Option<Arc<personal_secretary::PlannerUseCase>>, RuntimeError> {
    if !config.action_planner.enabled {
        tracing::info!("Action Planner 已禁用（action_planner.enabled=false）");
        return Ok(None);
    }
    let action_store = build_mysql_action_store(db.clone());
    let planner: Arc<dyn personal_secretary::ActionPlannerT> = if config.llm.enabled {
        let client = Arc::new(
            OpenAiCompatibleClient::new(&config.llm)
                .map_err(|error| RuntimeError::Llm(error.to_string()))?,
        );
        Arc::new(
            crate::action_planner::LlmActionPlanner::from_openai(client)
                .map_err(|error| RuntimeError::Llm(error.to_string()))?,
        )
    } else {
        tracing::info!("LLM 已禁用；Action Planner 使用空 NoAction 规划器");
        Arc::new(NoopActionPlanner)
    };
    // P0-3 修复：注入 DatabaseConnection，per-run 构造绑定业务 ActionRunId 的 CheckpointStore。
    // P0-2 修复：接入 RetrieverUseCase，让 PlanNode 检索数据库证据 + EffectExecutor 执行真实查询。
    let retriever_store = personal_secretary::build_mysql_retriever_store(db.clone());
    let retriever = Arc::new(personal_secretary::RetrieverUseCase::new(
        retriever_store,
        personal_secretary::RetrieverPolicy::default(),
    ));
    // checkpoint_store 参数仅为满足签名；生产用 with_checkpoint_db 注入 MySQL。
    let placeholder_checkpoint: Arc<
        dyn personal_secretary::CheckpointStore<personal_secretary::SecretaryAgentState>,
    > = Arc::new(personal_secretary::InMemoryCheckpointStore::new());
    let use_case = Arc::new(
        personal_secretary::PlannerUseCase::new(
            action_store,
            planner,
            placeholder_checkpoint,
            config.action_planner.lease_secs,
        )
        .with_retriever(retriever)
        .with_checkpoint_db(db),
    );
    let handle = crate::action_planner_worker::spawn_action_planner_worker(
        Arc::clone(&use_case),
        config.action_planner.clone(),
    );
    tracing::info!(
        lease_secs = config.action_planner.lease_secs,
        scan_interval_ms = config.action_planner.scan_interval_ms,
        "Action Planner Worker 已装配"
    );
    handles.action_planner = Some(handle);
    Ok(Some(use_case))
}

async fn drain_ingestion_worker(
    worker: &mut tokio::task::JoinHandle<WorkerReport>,
    timeout: Duration,
    connection_epoch_id: &ConnectionEpochId,
) {
    match tokio::time::timeout(timeout, &mut *worker).await {
        Ok(Ok(report)) => tracing::debug!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            accepted = report.accepted,
            duplicates = report.duplicates,
            invalid = report.invalid,
            retries = report.retries,
            dropped = report.dropped,
            "持久化 Worker 已在连接结束前排空"
        ),
        Ok(Err(error)) => tracing::error!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            error = %error,
            "持久化 Worker 异常退出；连接空窗将保持 uncertain"
        ),
        Err(_) => {
            tracing::warn!(
                connection_epoch_id = %connection_epoch_id.as_str(),
                timeout_ms = timeout.as_millis(),
                "持久化 Worker 未在期限内排空，将中止并依赖历史回补"
            );
            worker.abort();
            let _ = worker.await;
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut signal) = signal(SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_allows_listed_group() {
        let mut whitelist = std::collections::HashSet::new();
        whitelist.insert(671260344);
        assert!(should_accept_group_message(671260344, &whitelist));
    }

    #[test]
    fn whitelist_rejects_non_listed_group() {
        let mut whitelist = std::collections::HashSet::new();
        whitelist.insert(671260344);
        assert!(!should_accept_group_message(999999999, &whitelist));
    }

    #[test]
    fn empty_whitelist_allows_all_groups() {
        let whitelist = std::collections::HashSet::new();
        // 空白名单 = 不启用过滤，放行所有群
        assert!(should_accept_group_message(671260344, &whitelist));
        assert!(should_accept_group_message(999999999, &whitelist));
    }
}
