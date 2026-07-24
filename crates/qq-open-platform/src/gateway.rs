use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use thiserror::Error;
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{QqApiError, QqOpenPlatformClient};

const OP_DISPATCH: i64 = 0;
const OP_HEARTBEAT: i64 = 1;
const OP_IDENTIFY: i64 = 2;
const OP_RESUME: i64 = 6;
const OP_RECONNECT: i64 = 7;
const OP_INVALID_SESSION: i64 = 9;
const OP_HELLO: i64 = 10;
const OP_HEARTBEAT_ACK: i64 = 11;
// 个人秘书只申请群/C2C 与互动事件；不申请尚未消费的频道公域权限。
const INTENTS: u64 = (1 << 25) | (1 << 26);
const MAX_GATEWAY_ENVELOPE_BYTES: usize = 1_048_576;
const MAX_MESSAGE_CONTENT_BYTES: usize = 262_144;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewaySession {
    pub app_id: String,
    pub session_id: String,
    pub sequence: u64,
}

#[async_trait]
pub trait GatewaySessionStoreT: Send + Sync {
    async fn load(&self, app_id: &str) -> Result<Option<GatewaySession>, GatewayRunError>;
    async fn save(&self, session: &GatewaySession) -> Result<(), GatewayRunError>;
    async fn clear(&self, app_id: &str) -> Result<(), GatewayRunError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QqGatewayEventKind {
    C2cMessage,
    GroupAtMessage,
    GroupMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QqGatewayEvent {
    pub app_id: String,
    pub event_kind: QqGatewayEventKind,
    pub platform_message_id: String,
    pub sender_openid: String,
    pub group_openid: Option<String>,
    pub content: String,
    pub timestamp: String,
    pub mentions: Vec<String>,
    pub raw_envelope: String,
}

#[async_trait]
pub trait GatewayEventHandlerT: Send + Sync {
    /// 只有在事件已可靠持久化后才返回成功；成功后 Gateway 才推进 Resume sequence。
    async fn persist(&self, event: QqGatewayEvent) -> Result<(), GatewayRunError>;
}

pub struct QqGatewayClient {
    api: Arc<QqOpenPlatformClient>,
    sessions: Arc<dyn GatewaySessionStoreT>,
    handler: Arc<dyn GatewayEventHandlerT>,
}

impl QqGatewayClient {
    pub fn new(
        api: Arc<QqOpenPlatformClient>,
        sessions: Arc<dyn GatewaySessionStoreT>,
        handler: Arc<dyn GatewayEventHandlerT>,
    ) -> Self {
        Self {
            api,
            sessions,
            handler,
        }
    }

    /// 运行一次 Gateway 连接。断线、服务端 RECONNECT 或无效会话均返回，由宿主统一退避重连。
    pub async fn run_once(&self) -> Result<(), GatewayRunError> {
        let gateway_url = self.api.get_gateway_url().await?;
        let access_token = self.api.access_token().await?;
        let (mut socket, _) = connect_async(gateway_url.as_str())
            .await
            .map_err(|error| GatewayRunError::Transport(error.to_string()))?;
        let app_id = self.api.app_id().to_owned();
        let mut session = self.sessions.load(&app_id).await?;
        let mut next_heartbeat: Option<Instant> = None;
        let mut heartbeat_interval = Duration::from_secs(45);
        let mut awaiting_heartbeat_ack = false;

        loop {
            tokio::select! {
                message = socket.next() => {
                    let Some(message) = message else { return Ok(()); };
                    match message.map_err(|error| GatewayRunError::Transport(error.to_string()))? {
                        Message::Text(text) => {
                            let raw = text.to_string();
                            if raw.len() > MAX_GATEWAY_ENVELOPE_BYTES {
                                return Err(GatewayRunError::Protocol(
                                    "gateway envelope exceeds the admission limit".into(),
                                ));
                            }
                            let payload: Value = serde_json::from_str(&raw)
                                .map_err(|error| GatewayRunError::Protocol(error.to_string()))?;
                            let op = payload.get("op").and_then(Value::as_i64)
                                .ok_or_else(|| GatewayRunError::Protocol("gateway payload has no op".into()))?;
                            match op {
                                OP_HELLO => {
                                    let interval_ms = payload.pointer("/d/heartbeat_interval")
                                        .and_then(Value::as_u64)
                                        .filter(|value| *value >= 1000)
                                        .ok_or_else(|| GatewayRunError::Protocol("HELLO has invalid heartbeat interval".into()))?;
                                    heartbeat_interval = Duration::from_millis(interval_ms);
                                    next_heartbeat = Some(Instant::now() + heartbeat_interval);
                                    let auth = if let Some(saved) = &session {
                                        serde_json::json!({
                                            "op": OP_RESUME,
                                            "d": {
                                                "token": format!("QQBot {access_token}"),
                                                "session_id": saved.session_id,
                                                "seq": saved.sequence,
                                            }
                                        })
                                    } else {
                                        serde_json::json!({
                                            "op": OP_IDENTIFY,
                                            "d": {
                                                "token": format!("QQBot {access_token}"),
                                                "intents": INTENTS,
                                                "shard": [0, 1],
                                            }
                                        })
                                    };
                                    socket.send(Message::Text(auth.to_string().into())).await
                                        .map_err(|error| GatewayRunError::Transport(error.to_string()))?;
                                }
                                OP_DISPATCH => {
                                    let sequence = payload.get("s").and_then(Value::as_u64)
                                        .ok_or_else(|| GatewayRunError::Protocol("DISPATCH has no sequence".into()))?;
                                    let event_type = payload.get("t").and_then(Value::as_str).unwrap_or_default();
                                    if event_type == "READY" {
                                        let session_id = required_bounded_string(payload.pointer("/d/session_id"), "READY.session_id", 512)?;
                                        session = Some(GatewaySession { app_id: app_id.clone(), session_id, sequence });
                                    } else if let Some(event) = map_message_event(&app_id, event_type, &payload, raw)? {
                                        // 持久化失败时不推进 sequence，并主动结束连接，让 QQ Resume 重投。
                                        self.handler.persist(event).await?;
                                        if let Some(saved) = &mut session { saved.sequence = sequence; }
                                    } else if let Some(saved) = &mut session {
                                        saved.sequence = sequence;
                                    }
                                    if let Some(saved) = &session { self.sessions.save(saved).await?; }
                                }
                                OP_RECONNECT => return Ok(()),
                                OP_INVALID_SESSION => {
                                    let can_resume = payload.get("d").and_then(Value::as_bool).unwrap_or(false);
                                    if !can_resume {
                                        self.sessions.clear(&app_id).await?;
                                        self.api.clear_token().await;
                                    }
                                    return Ok(());
                                }
                                OP_HEARTBEAT_ACK => awaiting_heartbeat_ack = false,
                                _ => tracing::debug!(app_id, op, "ignored QQ gateway opcode"),
                            }
                        }
                        Message::Ping(payload) => socket.send(Message::Pong(payload)).await
                            .map_err(|error| GatewayRunError::Transport(error.to_string()))?,
                        Message::Close(_) => return Ok(()),
                        _ => {}
                    }
                }
                _ = wait_for(next_heartbeat) => {
                    if awaiting_heartbeat_ack {
                        return Err(GatewayRunError::Transport(
                            "QQ gateway heartbeat ACK timed out".into(),
                        ));
                    }
                    let sequence = session.as_ref().map(|value| value.sequence);
                    let heartbeat = serde_json::json!({ "op": OP_HEARTBEAT, "d": sequence });
                    socket.send(Message::Text(heartbeat.to_string().into())).await
                        .map_err(|error| GatewayRunError::Transport(error.to_string()))?;
                    awaiting_heartbeat_ack = true;
                    next_heartbeat = Some(Instant::now() + heartbeat_interval);
                }
            }
        }
    }
}

async fn wait_for(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn map_message_event(
    app_id: &str,
    event_type: &str,
    payload: &Value,
    raw_envelope: String,
) -> Result<Option<QqGatewayEvent>, GatewayRunError> {
    let data = payload
        .get("d")
        .ok_or_else(|| GatewayRunError::Protocol("DISPATCH has no data".into()))?;
    let (event_kind, sender_path, group_openid) = match event_type {
        "C2C_MESSAGE_CREATE" => (QqGatewayEventKind::C2cMessage, "/author/user_openid", None),
        "GROUP_AT_MESSAGE_CREATE" => (
            QqGatewayEventKind::GroupAtMessage,
            "/author/member_openid",
            Some(required_bounded_string(
                data.get("group_openid"),
                "group_openid",
                191,
            )?),
        ),
        "GROUP_MESSAGE_CREATE" => (
            QqGatewayEventKind::GroupMessage,
            "/author/member_openid",
            Some(required_bounded_string(
                data.get("group_openid"),
                "group_openid",
                191,
            )?),
        ),
        _ => return Ok(None),
    };
    let mentions = data
        .get("mentions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mention| {
            mention
                .get("member_openid")
                .or_else(|| mention.get("user_openid"))
                .and_then(Value::as_str)
                .filter(|value| value.len() <= 191)
                .map(str::to_owned)
        })
        .take(100)
        .collect();
    Ok(Some(QqGatewayEvent {
        app_id: app_id.to_owned(),
        event_kind,
        platform_message_id: required_bounded_string(data.get("id"), "message.id", 191)?,
        sender_openid: required_bounded_string(data.pointer(sender_path), "author openid", 191)?,
        group_openid,
        content: optional_bounded_string(
            data.get("content"),
            "message.content",
            MAX_MESSAGE_CONTENT_BYTES,
        )?,
        timestamp: required_bounded_string(data.get("timestamp"), "message.timestamp", 128)?,
        mentions,
        raw_envelope,
    }))
}

fn required_bounded_string(
    value: Option<&Value>,
    field: &str,
    max_bytes: usize,
) -> Result<String, GatewayRunError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= max_bytes)
        .map(str::to_owned)
        .ok_or_else(|| GatewayRunError::Protocol(format!("missing or oversized {field}")))
}

fn optional_bounded_string(
    value: Option<&Value>,
    field: &str,
    max_bytes: usize,
) -> Result<String, GatewayRunError> {
    let value = value.and_then(Value::as_str).unwrap_or_default();
    if value.len() > max_bytes {
        return Err(GatewayRunError::Protocol(format!("oversized {field}")));
    }
    Ok(value.to_owned())
}

#[derive(Debug, Error)]
pub enum GatewayRunError {
    #[error(transparent)]
    Api(#[from] QqApiError),
    #[error("QQ gateway transport failed: {0}")]
    Transport(String),
    #[error("QQ gateway protocol error: {0}")]
    Protocol(String),
    #[error("QQ gateway persistence failed: {0}")]
    Persistence(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_c2c_and_group_identity_without_cross_account_guessing() {
        let c2c: Value = serde_json::from_str(
            r#"{"op":0,"s":1,"t":"C2C_MESSAGE_CREATE","d":{"id":"m1","content":"hello","timestamp":"2026-07-24T00:00:00Z","author":{"user_openid":"u1"}}}"#,
        ).unwrap();
        let event = map_message_event("app-a", "C2C_MESSAGE_CREATE", &c2c, c2c.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(event.app_id, "app-a");
        assert_eq!(event.sender_openid, "u1");
        assert_eq!(event.group_openid, None);

        let group: Value = serde_json::from_str(
            r#"{"op":0,"s":2,"t":"GROUP_AT_MESSAGE_CREATE","d":{"id":"m2","content":"@bot hi","timestamp":"2026-07-24T00:00:01Z","group_openid":"g1","author":{"member_openid":"member1"},"mentions":[{"member_openid":"bot"}]}}"#,
        ).unwrap();
        let event = map_message_event(
            "app-b",
            "GROUP_AT_MESSAGE_CREATE",
            &group,
            group.to_string(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(event.group_openid.as_deref(), Some("g1"));
        assert_eq!(event.mentions, vec!["bot"]);
    }

    #[test]
    fn requests_only_the_intents_consumed_by_this_adapter() {
        assert_eq!(INTENTS, (1 << 25) | (1 << 26));
        assert_eq!(INTENTS & (1 << 30), 0);
    }
}
