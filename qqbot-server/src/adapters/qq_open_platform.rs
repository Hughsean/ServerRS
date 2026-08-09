use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use personal_secretary::{
    ContentSegment, ConversationKind, ConversationRef, FollowUpUseCase, InboundEventStoreT,
    InboundMessageEnvelope, IngestMessageOutcome, MessageSource, NotificationFailureKind,
    OwnerBinding, OwnerBindingStoreT, OwnerNotificationContent, OwnerResponseDeliveryScope,
    OwnerResponseDeliveryStoreT, OwnerResponseDeliveryUseCase, OwnerResponseTarget,
    SecretaryActionResumeInput, SecretaryApprovalDecision, SourceAccountRef, SourceMessageRef,
    VerifiedActor, VerifiedActorKind,
};
use qq_open_platform::{
    GatewayEventHandlerT, GatewayRunError, GatewaySessionStoreT, QqApiError, QqGatewayClient,
    QqGatewayEvent, QqGatewayEventKind, QqOpenPlatformClient, QqTarget,
};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::config::QqOpenPlatformConfig;
use crate::owner_approval::{ApprovalCommand, parse_owner_approval_command};
use crate::worker_lifecycle::WorkerHandle;

pub(crate) struct OfficialPlatformHandle {
    shutdown: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

pub(crate) struct OfficialPlatformPorts {
    inbound: Arc<dyn InboundEventStoreT>,
    owner_bindings: Arc<dyn OwnerBindingStoreT>,
    session_store: Arc<dyn GatewaySessionStoreT>,
    raw_events: Arc<dyn OfficialRawEventStoreT>,
    owner_responses: Arc<dyn OwnerResponseDeliveryStoreT>,
}

impl OfficialPlatformPorts {
    pub(crate) fn new(
        inbound: Arc<dyn InboundEventStoreT>,
        owner_bindings: Arc<dyn OwnerBindingStoreT>,
        session_store: Arc<dyn GatewaySessionStoreT>,
        raw_events: Arc<dyn OfficialRawEventStoreT>,
        owner_responses: Arc<dyn OwnerResponseDeliveryStoreT>,
    ) -> Self {
        Self {
            inbound,
            owner_bindings,
            session_store,
            raw_events,
            owner_responses,
        }
    }
}

impl OfficialPlatformHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("qq_open_platform", self.join)
    }
}

pub(crate) async fn spawn_official_platform(
    config: QqOpenPlatformConfig,
    ports: OfficialPlatformPorts,
    follow_up: Arc<FollowUpUseCase>,
    managed_account: SourceAccountRef,
    action_planner: Option<Arc<personal_secretary::PlannerUseCase>>,
) -> Result<OfficialPlatformHandle, GatewayRunError> {
    let OfficialPlatformPorts {
        inbound,
        owner_bindings,
        session_store,
        raw_events,
        owner_responses,
    } = ports;
    let credentials = config
        .credentials()
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
    let api = Arc::new(QqOpenPlatformClient::new(credentials)?);
    let command_account = SourceAccountRef::new(MessageSource::QqOpenPlatform, api.app_id())
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
    let owner_response_delivery = Arc::new(OwnerResponseDeliveryUseCase::new(
        owner_responses,
        OwnerResponseDeliveryScope::new(
            managed_account.clone(),
            command_account.clone(),
            config.owner_openid.clone(),
        )
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?,
    ));
    owner_bindings
        .ensure_owner_binding(&OwnerBinding {
            managed_account: managed_account.clone(),
            command_account: command_account.clone(),
            owner_actor_id: config.owner_openid.clone(),
        })
        .await
        .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
    let handler: Arc<dyn GatewayEventHandlerT> = Arc::new(OfficialInboundHandler {
        inbound,
        raw_events,
        owner_openid: config.owner_openid.clone(),
        action_planner,
        managed_account: managed_account.clone(),
        command_account,
        owner_timezone: config.owner_timezone.clone(),
    });
    let gateway = Arc::new(QqGatewayClient::new(
        Arc::clone(&api),
        session_store,
        handler,
    ));
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_official_workers(
        gateway,
        api,
        follow_up,
        owner_response_delivery,
        managed_account,
        config,
        receiver,
    ));
    Ok(OfficialPlatformHandle { shutdown, join })
}

async fn run_official_workers(
    gateway: Arc<QqGatewayClient>,
    api: Arc<QqOpenPlatformClient>,
    follow_up: Arc<FollowUpUseCase>,
    owner_response_delivery: Arc<OwnerResponseDeliveryUseCase>,
    managed_account: SourceAccountRef,
    config: QqOpenPlatformConfig,
    shutdown: watch::Receiver<bool>,
) {
    let mut tasks = JoinSet::new();
    tasks.spawn(run_gateway_loop(gateway, config.clone(), shutdown.clone()));
    if config.proactive_notifications {
        tasks.spawn(run_outbox_loop(
            Arc::clone(&api),
            follow_up,
            managed_account,
            config.clone(),
            shutdown.clone(),
        ));
    } else {
        tracing::info!("Owner 主动业务提醒已禁用；被动回复与生命周期通知保持启用");
    }
    if config.lifecycle_notifications {
        tasks.spawn(run_lifecycle_notifications(
            Arc::clone(&api),
            config.owner_openid.clone(),
            shutdown.clone(),
        ));
    }
    tasks.spawn(run_owner_response_loop(
        api,
        owner_response_delivery,
        config,
        shutdown,
    ));
    while tasks.join_next().await.is_some() {}
}

const OWNER_RESPONSE_SCAN_INTERVAL: Duration = Duration::from_millis(250);
const OWNER_RESPONSE_MAX_AGE_SECS: u64 = 240;

async fn run_owner_response_loop(
    api: Arc<QqOpenPlatformClient>,
    delivery: Arc<OwnerResponseDeliveryUseCase>,
    config: QqOpenPlatformConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let result = delivery
            .claim_pending_response(
                unix_now(),
                config.notification_lease_secs,
                OWNER_RESPONSE_MAX_AGE_SECS,
            )
            .await;
        match result {
            Ok(Some(response)) => {
                let text = render_owner_response(&response.draft);
                if text.is_empty() {
                    let _ = delivery
                        .mark_response_failed(
                            &response.response_id,
                            &response.lease_token,
                            "empty_response",
                            NotificationFailureKind::Permanent,
                        )
                        .await;
                    continue;
                }
                let target = match &response.target {
                    OwnerResponseTarget::C2c => QqTarget::C2c {
                        user_openid: config.owner_openid.clone(),
                    },
                    OwnerResponseTarget::Group { group_openid } => QqTarget::Group {
                        group_openid: group_openid.clone(),
                    },
                };
                match api
                    .send_text_reply(&target, &text, &response.reply_to_platform_message_id, 1)
                    .await
                {
                    Ok(receipt) => {
                        if let Err(error) = delivery
                            .mark_response_delivered(
                                &response.response_id,
                                &response.lease_token,
                                &receipt.platform_message_id,
                            )
                            .await
                        {
                            tracing::error!(
                                error = %error,
                                error_code = "owner_response_receipt_persistence_failed",
                                "Owner 被动回复已提交，但本地回执持久化失败"
                            );
                        } else {
                            tracing::info!("Owner 被动回复已送达");
                        }
                    }
                    Err(error) => {
                        let failure_kind = classify_delivery_failure(&error);
                        let error_code = delivery_error_code(&error);
                        if let Err(store_error) = delivery
                            .mark_response_failed(
                                &response.response_id,
                                &response.lease_token,
                                error_code,
                                failure_kind,
                            )
                            .await
                        {
                            tracing::warn!(
                                error = %store_error,
                                error_code,
                                "Owner 被动回复失败状态持久化失败"
                            );
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(
                error = %error,
                error_code = "owner_response_claim_failed",
                "Owner 被动回复领取失败"
            ),
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = tokio::time::sleep(OWNER_RESPONSE_SCAN_INTERVAL) => {}
        }
    }
}

const LIFECYCLE_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(8);

async fn run_lifecycle_notifications(
    api: Arc<QqOpenPlatformClient>,
    owner_openid: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let target = QqTarget::C2c {
        user_openid: owner_openid,
    };
    send_lifecycle_notification(&api, &target, "秘书已上线", "online").await;
    loop {
        if *shutdown.borrow() || shutdown.changed().await.is_err() {
            break;
        }
    }
    send_lifecycle_notification(&api, &target, "秘书正在安全下线", "offline").await;
}

async fn send_lifecycle_notification(
    api: &QqOpenPlatformClient,
    target: &QqTarget,
    content: &str,
    stage: &'static str,
) {
    match tokio::time::timeout(
        LIFECYCLE_NOTIFICATION_TIMEOUT,
        api.send_text(target, content),
    )
    .await
    {
        Ok(Ok(_)) => tracing::info!(stage, "Owner 生命周期通知已送达"),
        Ok(Err(error)) => tracing::warn!(
            stage,
            error_code = delivery_error_code(&error),
            "Owner 生命周期通知发送失败"
        ),
        Err(_) => tracing::warn!(
            stage,
            error_code = "lifecycle_notification_timeout",
            "Owner 生命周期通知发送超时"
        ),
    }
}

fn render_owner_response(draft: &personal_secretary::OwnerResponseDraft) -> String {
    const MAX_CHARS: usize = 4_000;
    const TRUNCATED_SUFFIX: &str = "\n\n（内容已截断）";

    let text = draft
        .segments()
        .iter()
        .map(personal_secretary::ResponseSegment::text)
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.chars().count() <= MAX_CHARS {
        return text;
    }
    let prefix_chars = MAX_CHARS.saturating_sub(TRUNCATED_SUFFIX.chars().count());
    let mut truncated = text.chars().take(prefix_chars).collect::<String>();
    truncated.push_str(TRUNCATED_SUFFIX);
    truncated
}

async fn run_gateway_loop(
    gateway: Arc<QqGatewayClient>,
    config: QqOpenPlatformConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut delay = config.reconnect_initial_ms;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let result = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
                continue;
            }
            result = gateway.run_once() => result,
        };
        match result {
            Ok(()) => delay = config.reconnect_initial_ms,
            Err(error) => {
                tracing::warn!(error = %error, retry_ms = delay, "QQ Open Platform Gateway disconnected")
            }
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
        }
        delay = delay.saturating_mul(2).min(config.reconnect_max_ms);
    }
}

async fn run_outbox_loop(
    api: Arc<QqOpenPlatformClient>,
    follow_up: Arc<FollowUpUseCase>,
    managed_account: SourceAccountRef,
    config: QqOpenPlatformConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let now = unix_now();
        match follow_up
            .claim_due_notification(&managed_account, now, config.notification_lease_secs)
            .await
        {
            Ok(Some(notification)) => {
                if notification.managed_account != managed_account {
                    let _ = follow_up
                        .mark_notification_failed(
                            &notification.notification_id,
                            &notification.lease_token,
                            "managed_account_mismatch",
                            NotificationFailureKind::Permanent,
                        )
                        .await;
                    continue;
                }
                let due = DateTime::<Utc>::from_timestamp(notification.due_at_unix_secs, 0)
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| notification.due_at_unix_secs.to_string());
                let text = match notification.content {
                    OwnerNotificationContent::FollowUp { commitment } => {
                        format!("⏰ 承诺提醒\n事项：{}\n截止：{}", commitment.action, due)
                    }
                    OwnerNotificationContent::Agenda { kind, title } => format!(
                        "⏰ {}提醒\n事项：{}\n时间：{}",
                        agenda_kind_label(kind),
                        title,
                        due
                    ),
                    OwnerNotificationContent::ResponseExpectation {
                        question_excerpt, ..
                    } => format!(
                        "⏰ 待回复提醒\n有一条外部联系人的问题仍未见你的回复：{}",
                        question_excerpt.chars().take(300).collect::<String>()
                    ),
                    OwnerNotificationContent::ProjectBlocker {
                        project_key,
                        blockers,
                    } => format!(
                        "⏰ 项目阻塞提醒\n项目：{}\n阻塞：{}",
                        project_key.chars().take(120).collect::<String>(),
                        blockers
                            .iter()
                            .take(5)
                            .map(|value| value.chars().take(120).collect::<String>())
                            .collect::<Vec<_>>()
                            .join("；")
                    ),
                };
                let target = QqTarget::C2c {
                    user_openid: config.owner_openid.clone(),
                };
                match api.send_text(&target, &text).await {
                    Ok(receipt) => {
                        if let Err(error) = follow_up
                            .mark_notification_delivered(
                                &notification.notification_id,
                                &notification.lease_token,
                                &receipt.platform_message_id,
                            )
                            .await
                        {
                            // 外部已成功、本地回执失败：租约到期后进入 unknown_commit，禁止盲重试。
                            tracing::error!(error = %error, notification_id = notification.notification_id.as_str(), "QQ notification receipt persistence failed");
                        }
                    }
                    Err(error) => {
                        let kind = classify_delivery_failure(&error);
                        let code = delivery_error_code(&error);
                        let _ = follow_up
                            .mark_notification_failed(
                                &notification.notification_id,
                                &notification.lease_token,
                                code,
                                kind,
                            )
                            .await;
                    }
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(error = %error, "failed to claim QQ notification outbox"),
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

fn agenda_kind_label(kind: personal_secretary::AgendaItemKind) -> &'static str {
    match kind {
        personal_secretary::AgendaItemKind::Schedule => "日程",
        personal_secretary::AgendaItemKind::Task => "任务",
        personal_secretary::AgendaItemKind::Reminder => "提醒",
    }
}

fn classify_delivery_failure(error: &QqApiError) -> NotificationFailureKind {
    match error {
        QqApiError::RateLimited => NotificationFailureKind::Retryable,
        // POST 传输中断或 5xx 无法证明服务端未提交，必须人工对账。
        QqApiError::Transport(_)
        | QqApiError::Rejected {
            status: 500..=599, ..
        } => NotificationFailureKind::UnknownCommit,
        _ => NotificationFailureKind::Permanent,
    }
}

fn delivery_error_code(error: &QqApiError) -> &'static str {
    match error {
        QqApiError::InvalidEndpoint => "invalid_endpoint",
        QqApiError::InvalidTarget => "invalid_target",
        QqApiError::InvalidContent => "invalid_content",
        QqApiError::InvalidReplyContext => "invalid_reply_context",
        QqApiError::Unauthorized => "unauthorized",
        QqApiError::RateLimited => "rate_limited",
        QqApiError::Rejected { .. } => "provider_rejected",
        QqApiError::Transport(_) => "transport_unknown",
        QqApiError::Protocol(_) => "protocol_error",
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

/// 配置已在启动阶段验证；极端时间戳越界时安全回退 UTC 偏移。
fn timezone_offset_secs(timezone: &str, unix_secs: i64) -> i64 {
    use chrono::{Offset, TimeZone};

    timezone
        .parse::<chrono_tz::Tz>()
        .ok()
        .and_then(|tz| tz.timestamp_opt(unix_secs, 0).single())
        .map(|datetime| datetime.offset().fix().local_minus_utc() as i64)
        .unwrap_or(0)
}

struct OfficialInboundHandler {
    inbound: Arc<dyn InboundEventStoreT>,
    raw_events: Arc<dyn OfficialRawEventStoreT>,
    owner_openid: String,
    /// Action Planner 用例。OwnerCommand 入库后调用 ensure_action_run 创建运行。
    /// 为 Option 以支持 action_planner.enabled=false 的场景。
    action_planner: Option<Arc<personal_secretary::PlannerUseCase>>,
    /// P0 修复：被管理账号（NapCat 等），用于 ActionRun 检索数据范围。
    /// 区别于 command_account（QQ 开放平台 Bot 自身）。
    managed_account: personal_secretary::SourceAccountRef,
    /// OwnerCommand 来源账号（QQ 开放平台 Bot），仅供审计。
    #[allow(dead_code)]
    command_account: personal_secretary::SourceAccountRef,
    /// 已在启动阶段校验的 Owner IANA 时区。
    owner_timezone: String,
}

#[async_trait]
pub(crate) trait OfficialRawEventStoreT: Send + Sync {
    async fn persist(
        &self,
        source_event_id: &str,
        event: &QqGatewayEvent,
    ) -> Result<(), GatewayRunError>;
}

impl OfficialInboundHandler {
    async fn try_resume_owner_approval(
        &self,
        envelope: &InboundMessageEnvelope,
        approval_source_event_id: &personal_secretary::SourceEventId,
    ) -> Result<bool, String> {
        let Some(command) = parse_owner_approval_command(&envelope.normalized_text) else {
            return Ok(false);
        };
        let planner = self
            .action_planner
            .as_ref()
            .ok_or_else(|| "action planner is disabled".to_string())?;
        let candidates = planner
            .list_suspended_runs(&self.managed_account, 100)
            .await
            .map_err(|error| error.to_string())?;
        let matches: Vec<_> = match command.proposal_short_id.as_deref() {
            Some(short_id) => candidates
                .into_iter()
                .filter(|candidate| candidate.proposal_id.starts_with(short_id))
                .collect(),
            None => candidates,
        };
        let [candidate] = matches.as_slice() else {
            // 0 项与多项均不选择“最新一条”，避免 Owner 的模糊确认误操作。
            return Err("approval requires exactly one matching suspended proposal".into());
        };
        let decision = match command.command {
            ApprovalCommand::Approve => SecretaryApprovalDecision::Approve,
            ApprovalCommand::Reject => SecretaryApprovalDecision::Reject,
        };
        planner
            .resume_run(
                &candidate.run_id,
                &candidate.checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id: candidate.proposal_id.clone(),
                    decision,
                    // 运行仍绑定原始命令；审批事件只追加为不可变审计证据。
                    command_source_event_id: candidate.command_source_event_id.clone(),
                    approval_source_event_id: Some(approval_source_event_id.clone()),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        tracing::info!(
            run_id = candidate.run_id.as_str(),
            proposal_id = %candidate.proposal_id,
            "owner approval resumed suspended action"
        );
        Ok(true)
    }

    /// OwnerCommand 入库后幂等创建 action_run。
    /// run_id 从 source_event_id 派生（非随机 UUID），保证重复投递不创建多个运行。
    async fn ensure_action_run_for_owner_command(
        &self,
        envelope: &InboundMessageEnvelope,
        source_event_id: &personal_secretary::SourceEventId,
    ) {
        let Some(planner) = &self.action_planner else {
            return;
        };
        // 稳定 UUIDv5 同时满足幂等与数据库 CHAR(36) 边界。
        let run_id = personal_secretary::ActionRunId::for_owner_command(source_event_id, "v1");
        // P0 修复：用 managed_account（NapCat 被管理账号）作为数据检索范围，
        // 而非 command_account（QQ 开放平台 Bot）。两者通过 OwnerBinding 验证。
        let seed = personal_secretary::ActionRunSeed {
            account: self.managed_account.clone(),
            command_source_event_id: source_event_id.clone(),
            command_text: envelope.normalized_text.clone(),
            conversation_id: envelope.conversation.id.clone(),
            occurred_at_unix_secs: envelope.occurred_at_unix_secs,
            timezone_offset_secs: timezone_offset_secs(
                &self.owner_timezone,
                envelope.occurred_at_unix_secs,
            ),
            timezone: self.owner_timezone.clone(),
            recent_events: Vec::new(),
        };
        match planner.ensure_action_run(&run_id, &seed).await {
            Ok(created) => tracing::info!(
                run_id = run_id.as_str(),
                created,
                "action_run ensured for OwnerCommand"
            ),
            Err(error) => tracing::error!(
                run_id = run_id.as_str(),
                error = %error,
                "failed to ensure action_run; OwnerCommand will not be planned"
            ),
        }
    }
}

#[async_trait]
impl GatewayEventHandlerT for OfficialInboundHandler {
    async fn persist(&self, event: QqGatewayEvent) -> Result<(), GatewayRunError> {
        let is_owner = is_owner_command(&event, &self.owner_openid);
        if event.event_kind == QqGatewayEventKind::C2cMessage && !is_owner {
            tracing::warn!(
                error_code = "owner_openid_mismatch",
                "QQ 开放平台 C2C 发送者不是配置 Owner；仅持久化观察，不创建 Action 或回复"
            );
        }
        let conversation = match event.event_kind {
            QqGatewayEventKind::C2cMessage if is_owner => {
                ConversationRef::new(ConversationKind::OwnerControl, &event.sender_openid)
            }
            QqGatewayEventKind::C2cMessage => {
                ConversationRef::new(ConversationKind::Private, &event.sender_openid)
            }
            QqGatewayEventKind::GroupAtMessage if is_owner => ConversationRef::new(
                ConversationKind::OwnerControl,
                event.group_openid.as_deref().unwrap_or_default(),
            ),
            QqGatewayEventKind::GroupAtMessage | QqGatewayEventKind::GroupMessage => {
                ConversationRef::new(
                    ConversationKind::Group,
                    event.group_openid.as_deref().unwrap_or_default(),
                )
            }
        }
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
        let occurred = DateTime::parse_from_rfc3339(&event.timestamp)
            .map_err(|error| GatewayRunError::Protocol(format!("invalid QQ timestamp: {error}")))?
            .timestamp();
        let mut segments = Vec::with_capacity(event.mentions.len() + 1);
        if !event.content.is_empty() {
            segments.push(ContentSegment::Text {
                content: event.content.clone(),
            });
        }
        segments.extend(
            event
                .mentions
                .iter()
                .cloned()
                .map(|actor_id| ContentSegment::Mention { actor_id }),
        );
        let envelope = InboundMessageEnvelope::new(
            SourceMessageRef::new(
                MessageSource::QqOpenPlatform,
                &event.app_id,
                &event.platform_message_id,
            )
            .map_err(|error| GatewayRunError::Protocol(error.to_string()))?,
            conversation,
            VerifiedActor::new(
                if is_owner {
                    VerifiedActorKind::Owner
                } else {
                    VerifiedActorKind::External
                },
                &event.sender_openid,
            )
            .map_err(|error| GatewayRunError::Protocol(error.to_string()))?,
            occurred,
            event.content.clone(),
            segments,
        )
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
        let outcome = self
            .inbound
            .insert_message_if_absent(&envelope)
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        self.raw_events
            .persist(outcome.source_event_id().as_str(), &event)
            .await?;
        // P0 修复：OwnerCommand 入库后幂等创建 action_run。
        // Accepted 和 Duplicate 都执行 ensure_action_run，防止首次入库后
        // ensure_action_run 短暂失败导致命令永久没有 Run（双写丢失窗口）。
        // ensure_action_run 本身是幂等的（INSERT IGNORE + 业务唯一键）。
        if envelope.accepts_instructions() {
            match self
                .try_resume_owner_approval(&envelope, outcome.source_event_id())
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    self.ensure_action_run_for_owner_command(&envelope, outcome.source_event_id())
                        .await;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Owner approval command was not resumed");
                }
            }
        }
        tracing::info!(
            owner_command = is_owner,
            duplicate = matches!(outcome, IngestMessageOutcome::Duplicate { .. }),
            "QQ Open Platform event durably admitted"
        );
        Ok(())
    }
}

fn is_owner_command(event: &QqGatewayEvent, owner_openid: &str) -> bool {
    event.sender_openid == owner_openid
        && matches!(
            event.event_kind,
            QqGatewayEventKind::C2cMessage | QqGatewayEventKind::GroupAtMessage
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gateway_event(kind: QqGatewayEventKind, sender: &str) -> QqGatewayEvent {
        QqGatewayEvent {
            app_id: "app".into(),
            event_kind: kind,
            platform_message_id: "message".into(),
            sender_openid: sender.into(),
            group_openid: Some("group".into()),
            content: "content".into(),
            timestamp: "2026-08-07T00:00:00Z".into(),
            mentions: Vec::new(),
            raw_envelope: "{}".into(),
        }
    }

    #[test]
    fn only_owner_c2c_or_explicit_group_at_is_a_command() {
        assert!(is_owner_command(
            &gateway_event(QqGatewayEventKind::C2cMessage, "owner"),
            "owner"
        ));
        assert!(is_owner_command(
            &gateway_event(QqGatewayEventKind::GroupAtMessage, "owner"),
            "owner"
        ));
        assert!(!is_owner_command(
            &gateway_event(QqGatewayEventKind::GroupMessage, "owner"),
            "owner"
        ));
        assert!(!is_owner_command(
            &gateway_event(QqGatewayEventKind::GroupAtMessage, "external"),
            "owner"
        ));
    }

    #[test]
    fn ambiguous_post_failures_never_retry_blindly() {
        assert_eq!(
            classify_delivery_failure(&QqApiError::Transport("timeout".into())),
            NotificationFailureKind::UnknownCommit
        );
        assert_eq!(
            classify_delivery_failure(&QqApiError::RateLimited),
            NotificationFailureKind::Retryable
        );
    }

    #[test]
    fn invalid_reply_context_has_a_stable_redacted_error_code() {
        assert_eq!(
            delivery_error_code(&QqApiError::InvalidReplyContext),
            "invalid_reply_context"
        );
    }

    #[test]
    fn owner_response_segments_are_joined_for_passive_reply() {
        let draft = personal_secretary::OwnerResponseDraft::new(
            vec![
                personal_secretary::ResponseSegment::Summary {
                    text: "第一段".into(),
                },
                personal_secretary::ResponseSegment::Summary {
                    text: "第二段".into(),
                },
            ],
            Vec::new(),
            1,
        )
        .unwrap();
        assert_eq!(render_owner_response(&draft), "第一段\n\n第二段");
    }

    #[test]
    fn owner_response_is_truncated_to_platform_character_limit() {
        let draft = personal_secretary::OwnerResponseDraft::new(
            (0..5)
                .map(|_| personal_secretary::ResponseSegment::Summary {
                    text: "文".repeat(1_000),
                })
                .collect(),
            Vec::new(),
            1,
        )
        .unwrap();
        let rendered = render_owner_response(&draft);
        assert_eq!(rendered.chars().count(), 4_000);
        assert!(rendered.ends_with("（内容已截断）"));
    }
}
