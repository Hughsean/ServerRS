use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use personal_secretary::{
    ContentSegment, ConversationKind, ConversationRef, FollowUpUseCase, InboundEventStoreT,
    InboundMessageEnvelope, IngestMessageOutcome, MessageSource, NotificationFailureKind,
    OwnerBinding, SourceAccountRef, SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
use qq_open_platform::{
    GatewayEventHandlerT, GatewayRunError, GatewaySession, GatewaySessionStoreT, QqApiError,
    QqGatewayClient, QqGatewayEvent, QqGatewayEventKind, QqOpenPlatformClient, QqTarget,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::config::QqOpenPlatformConfig;
use crate::worker_lifecycle::WorkerHandle;

pub(crate) struct OfficialPlatformHandle {
    shutdown: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
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
    db: DatabaseConnection,
    inbound: Arc<dyn InboundEventStoreT>,
    follow_up: Arc<FollowUpUseCase>,
    managed_account: SourceAccountRef,
    action_planner: Option<Arc<personal_secretary::PlannerUseCase>>,
) -> Result<OfficialPlatformHandle, GatewayRunError> {
    let credentials = config
        .credentials()
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
    let api = Arc::new(QqOpenPlatformClient::new(credentials)?);
    let command_account = SourceAccountRef::new(MessageSource::QqOpenPlatform, api.app_id())
        .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
    let owner_bindings = personal_secretary::build_mysql_owner_binding_store(db.clone());
    owner_bindings
        .ensure_owner_binding(&OwnerBinding {
            managed_account: managed_account.clone(),
            command_account: command_account.clone(),
            owner_actor_id: config.owner_openid.clone(),
        })
        .await
        .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
    let session_store: Arc<dyn GatewaySessionStoreT> =
        Arc::new(MySqlGatewaySessionStore::new(db.clone()));
    let handler: Arc<dyn GatewayEventHandlerT> = Arc::new(OfficialInboundHandler {
        inbound,
        raw_events: MySqlRawEventStore::new(db),
        owner_openid: config.owner_openid.clone(),
        action_planner,
        managed_account: managed_account.clone(),
        command_account,
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
    managed_account: SourceAccountRef,
    config: QqOpenPlatformConfig,
    shutdown: watch::Receiver<bool>,
) {
    let mut tasks = JoinSet::new();
    tasks.spawn(run_gateway_loop(gateway, config.clone(), shutdown.clone()));
    tasks.spawn(run_outbox_loop(
        api,
        follow_up,
        managed_account,
        config,
        shutdown,
    ));
    while tasks.join_next().await.is_some() {}
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
                let text = format!(
                    "⏰ 承诺提醒\n事项：{}\n截止：{}",
                    notification.commitment.action, due
                );
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

struct OfficialInboundHandler {
    inbound: Arc<dyn InboundEventStoreT>,
    raw_events: MySqlRawEventStore,
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
}

impl OfficialInboundHandler {
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
            timezone_offset_secs: 0,
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
        let is_owner = event.event_kind == QqGatewayEventKind::C2cMessage
            && event.sender_openid == self.owner_openid;
        let conversation = match event.event_kind {
            QqGatewayEventKind::C2cMessage if is_owner => {
                ConversationRef::new(ConversationKind::OwnerControl, &event.sender_openid)
            }
            QqGatewayEventKind::C2cMessage => {
                ConversationRef::new(ConversationKind::Private, &event.sender_openid)
            }
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
            self.ensure_action_run_for_owner_command(&envelope, outcome.source_event_id())
                .await;
        }
        tracing::info!(
            app_id = event.app_id,
            message_id = event.platform_message_id,
            owner_command = is_owner,
            duplicate = matches!(outcome, IngestMessageOutcome::Duplicate { .. }),
            "QQ Open Platform event durably admitted"
        );
        Ok(())
    }
}

struct MySqlGatewaySessionStore {
    db: DatabaseConnection,
}

impl MySqlGatewaySessionStore {
    fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GatewaySessionStoreT for MySqlGatewaySessionStore {
    async fn load(&self, app_id: &str) -> Result<Option<GatewaySession>, GatewayRunError> {
        Ok(SessionRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT app_id, session_id, last_sequence FROM secretary_qq_gateway_sessions WHERE app_id = ?",
            [app_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(|error| GatewayRunError::Persistence(error.to_string()))?
        .map(|row| GatewaySession {
            app_id: row.app_id,
            session_id: row.session_id,
            sequence: row.last_sequence,
        }))
    }

    async fn save(&self, session: &GatewaySession) -> Result<(), GatewayRunError> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_qq_gateway_sessions (app_id, session_id, last_sequence)
               VALUES (?, ?, ?)
               ON DUPLICATE KEY UPDATE
                 last_sequence = IF(session_id = VALUES(session_id),
                   GREATEST(last_sequence, VALUES(last_sequence)), VALUES(last_sequence)),
                 session_id = VALUES(session_id)"#,
                [
                    session.app_id.clone().into(),
                    session.session_id.clone().into(),
                    session.sequence.into(),
                ],
            ))
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        Ok(())
    }

    async fn clear(&self, app_id: &str) -> Result<(), GatewayRunError> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "DELETE FROM secretary_qq_gateway_sessions WHERE app_id = ?",
                [app_id.into()],
            ))
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, FromQueryResult)]
struct SessionRow {
    app_id: String,
    session_id: String,
    last_sequence: u64,
}

struct MySqlRawEventStore {
    db: DatabaseConnection,
}

impl MySqlRawEventStore {
    fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    async fn persist(
        &self,
        source_event_id: &str,
        event: &QqGatewayEvent,
    ) -> Result<(), GatewayRunError> {
        let kind = match event.event_kind {
            QqGatewayEventKind::C2cMessage => "c2c_message",
            QqGatewayEventKind::GroupAtMessage => "group_at_message",
            QqGatewayEventKind::GroupMessage => "group_message",
        };
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_qq_raw_events
                 (source_event_id, app_id, event_kind, envelope_json)
               VALUES (?, ?, ?, CAST(? AS JSON))
               ON DUPLICATE KEY UPDATE source_event_id = VALUES(source_event_id)"#,
                [
                    source_event_id.into(),
                    event.app_id.clone().into(),
                    kind.into(),
                    event.raw_envelope.clone().into(),
                ],
            ))
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
