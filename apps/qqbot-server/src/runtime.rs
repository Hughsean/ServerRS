use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use personal_secretary::{
    BackfillGapUseCase, ConnectionEndReason, ConnectionEpochId,
    ConservativeThreadSemanticExtractor, DeterministicThreadPlanner, DeterministicThreadPolicy,
    InboundEventStoreError, InboundIdentityError, MessageSource, PersonalSecretaryStoreT,
    SourceAccountRef, ThreadLinkUseCase, ThreadProjectionUseCase, ThreadSemanticUseCase,
    build_mysql_backfill_store, build_mysql_inbound_event_store, build_mysql_thread_link_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
use qqbot::napcat::{
    NapCatApiClient, NapCatConnectionObserver, NapCatError, NapCatEvent, NapCatEventHandler,
    NapCatListener,
};
use sea_orm::{ConnectOptions, Database};
use thiserror::Error;

use crate::backfill::spawn_backfill_worker;
use crate::config::AppConfig;
use crate::inbound::NapCatInboundMapper;
use crate::ingestion_worker::{IngestionQueue, WorkerReport, spawn_ingestion_worker};
use crate::thread_links::spawn_thread_links_worker;
use crate::thread_projection::spawn_thread_projection_worker;
use crate::thread_semantics::spawn_thread_semantics_worker;

/// 个人秘书入站边界：统一身份后先幂等落库，只有新事件才允许进入后续处理。
struct PersonalSecretaryInboundHandler {
    mapper: NapCatInboundMapper,
    queue: IngestionQueue,
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

#[async_trait::async_trait]
impl NapCatEventHandler for PersonalSecretaryInboundHandler {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError> {
        match event {
            NapCatEvent::GroupMessage(event) => {
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
}

pub async fn run(config: AppConfig) -> Result<(), RuntimeError> {
    let mut database_options = ConnectOptions::new(config.database.url.clone());
    database_options.max_connections(config.database.max_connections.max(1));
    let db = Database::connect(database_options).await?;
    tracing::info!("个人秘书数据库已连接");

    let store = build_mysql_inbound_event_store(db.clone());
    let account =
        SourceAccountRef::new(MessageSource::NapCat, config.napcat.self_qq_id.to_string())?;

    let thread_projection_handle = if config.thread_projection.enabled {
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
        Some(spawn_thread_projection_worker(
            use_case,
            config.thread_projection.clone(),
        ))
    } else {
        tracing::info!("确定性线程投影已禁用（thread_projection.enabled=false）");
        None
    };

    let thread_semantics_handle = if config.thread_semantics.enabled {
        let extractor = Arc::new(
            ConservativeThreadSemanticExtractor::new(config.thread_semantics.max_event_chars)
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
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
        Some(spawn_thread_semantics_worker(
            use_case,
            config.thread_semantics.clone(),
        ))
    } else {
        tracing::info!("线程类型化语义已禁用（thread_semantics.enabled=false）");
        None
    };

    let thread_links_handle = if config.thread_links.enabled {
        let use_case = Arc::new(
            ThreadLinkUseCase::new(
                build_mysql_thread_link_store(db.clone()),
                config.thread_links.max_events,
                config.thread_links.max_total_chars,
                config.thread_links.lease_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        Some(spawn_thread_links_worker(
            use_case,
            config.thread_links.clone(),
        ))
    } else {
        tracing::info!("跨会话线程关联候选已禁用（thread_links.enabled=false）");
        None
    };

    // 装配历史回补：只读 NapCat 客户端 + 回补状态仓储 + 协议无关用例 + 独立 Worker。
    // 分页算法、Gap 完整性判定和 SQL 不在 runtime 内，而分别在领域层与 MySQL 仓储。
    let backfill_handle = if config.backfill.enabled {
        let backfill_store = build_mysql_backfill_store(db.clone(), config.backfill.lease_secs);
        let napcat_readonly = Arc::new(NapCatApiClient::new(
            config.napcat.http_base_url.clone(),
            if config.napcat.http_token.trim().is_empty() {
                None
            } else {
                Some(config.napcat.http_token.clone())
            },
        ));
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
        Some(handle)
    } else {
        tracing::info!("历史回补已禁用（backfill.enabled=false）");
        None
    };
    let backfill_wake = backfill_handle
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
        let connection_epoch_id = store.begin_connection(&account).await?;
        let (queue, mut ingestion_worker) = spawn_ingestion_worker(
            Arc::clone(&store),
            connection_epoch_id.clone(),
            config.ingestion.clone(),
        );
        let handler: Arc<dyn NapCatEventHandler> = Arc::new(PersonalSecretaryInboundHandler {
            mapper: NapCatInboundMapper::new(config.napcat.self_qq_id),
            queue,
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
            _ = shutdown_signal() => {
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

        let gap_id = store
            .finish_connection(&connection_epoch_id, reason)
            .await?;
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
            if let Some(handle) = backfill_handle {
                handle.shutdown().await;
            }
            if let Some(handle) = thread_projection_handle {
                handle.shutdown().await;
            }
            if let Some(handle) = thread_semantics_handle {
                handle.shutdown().await;
            }
            if let Some(handle) = thread_links_handle {
                handle.shutdown().await;
            }
            return Ok(());
        }
        if observer.was_connected() {
            backoff = config.napcat.reconnect_initial_secs;
        }

        tracing::info!(backoff_secs = backoff, "等待后重新连接 NapCat");
        tokio::select! {
            _ = shutdown_signal() => {
                if let Some(handle) = backfill_handle {
                    handle.shutdown().await;
                }
                if let Some(handle) = thread_projection_handle {
                    handle.shutdown().await;
                }
                if let Some(handle) = thread_semantics_handle {
                    handle.shutdown().await;
                }
                if let Some(handle) = thread_links_handle {
                    handle.shutdown().await;
                }
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
        }
        backoff = backoff
            .saturating_mul(2)
            .min(config.napcat.reconnect_max_secs);
    }
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
