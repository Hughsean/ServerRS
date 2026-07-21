use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use super::message_parser::{normalize_text, parse_message_segments};
use super::{
    GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupMessageEvent, NapCatError,
    NapCatEvent, NapCatEventHandler, PokeEvent, SenderInfo,
};

/// NapCat 上报的原始群消息。
///
/// 所有非关键字段均允许缺失，以兼容 OneBot 实现之间的载荷差异；在转成公开事件前
/// 会验证群号、用户号和消息 ID。
#[derive(Debug, serde::Deserialize)]
struct OneBotGroupMessageEvent {
    #[serde(default)]
    message_type: String,
    #[serde(default, rename = "sub_type")]
    _sub_type: String,
    #[serde(default, deserialize_with = "deserialize_message_id")]
    message_id: String,
    #[serde(default)]
    group_id: i64,
    #[serde(default)]
    user_id: i64,
    #[serde(default, rename = "message")]
    _message: Value,
    #[serde(default)]
    raw_message: String,
    #[serde(default)]
    time: i64,
    #[serde(default)]
    sender: Option<OneBotSender>,
}

#[derive(Debug, serde::Deserialize)]
struct OneBotSender {
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    card: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

/// OneBot 的 message_id 在不同实现中可能是数字、字符串或 null。
fn deserialize_message_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    use serde::de;

    struct MessageIdVisitor;

    impl<'de> de::Visitor<'de> for MessageIdVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a number, string, or null")
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<String, E> {
            Ok(value.to_string())
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<String, E> {
            Ok(value.to_string())
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
            Ok(value.to_string())
        }

        fn visit_string<E: de::Error>(self, value: String) -> Result<String, E> {
            Ok(value)
        }

        fn visit_none<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }
    }

    d.deserialize_any(MessageIdVisitor)
}

#[derive(Debug, serde::Deserialize)]
struct OneBotNoticeEvent {
    #[serde(default)]
    notice_type: String,
    #[serde(default)]
    group_id: i64,
    #[serde(default)]
    user_id: i64,
    #[serde(default)]
    sub_type: String,
    #[serde(default)]
    operator_id: Option<i64>,
    #[serde(default)]
    target_id: Option<i64>,
    #[serde(default)]
    time: i64,
}

/// 通过正向 WebSocket 连接 NapCat，并把 OneBot 事件交给协议回调。
pub struct NapCatListener {
    ws_url: String,
    self_qq_id: i64,
    handler: Arc<dyn NapCatEventHandler>,
}

impl NapCatListener {
    pub fn new(ws_url: String, self_qq_id: i64, handler: Arc<dyn NapCatEventHandler>) -> Self {
        Self {
            ws_url,
            self_qq_id,
            handler,
        }
    }

    /// 连接一次并持续读取，连接关闭后返回，由宿主决定是否重连。
    pub async fn run_forward(&self) -> Result<(), NapCatError> {
        info!(url = %self.ws_url, "正在通过 WebSocket 连接 NapCat");
        let (mut stream, _) = connect_async(self.ws_url.as_str()).await.map_err(|error| {
            NapCatError::Connection(format!("WebSocket connect failed: {error}"))
        })?;

        info!("NapCat WebSocket 已连接");
        while let Some(message) = stream.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    if let Err(error) = self.handle_ws_message(text.as_str()).await {
                        warn!(error = %error, "NapCat 事件处理失败");
                    }
                }
                Ok(Message::Ping(payload)) => {
                    stream.send(Message::Pong(payload)).await.map_err(|error| {
                        NapCatError::Connection(format!("WebSocket pong failed: {error}"))
                    })?;
                }
                Ok(Message::Close(_)) => {
                    info!("NapCat WebSocket 已关闭");
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(NapCatError::Connection(format!(
                        "WebSocket receive failed: {error}"
                    )));
                }
            }
        }

        Ok(())
    }

    async fn handle_ws_message(&self, text: &str) -> Result<(), NapCatError> {
        let raw_event: Value = serde_json::from_str(text)
            .map_err(|error| NapCatError::Protocol(format!("invalid JSON: {error}")))?;
        let post_type = raw_event
            .get("post_type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match post_type {
            "notice" => self.handle_notice(raw_event).await,
            "message" => self.handle_message(raw_event).await,
            _ => {
                debug!(post_type, "忽略尚未建模的 OneBot 事件");
                Ok(())
            }
        }
    }

    async fn handle_notice(&self, raw_event: Value) -> Result<(), NapCatError> {
        let event: OneBotNoticeEvent = serde_json::from_value(raw_event.clone())
            .map_err(|error| NapCatError::Protocol(format!("invalid notice event: {error}")))?;
        validate_actor_ids(event.group_id, event.user_id, "notice")?;

        let event = match (event.notice_type.as_str(), event.sub_type.as_str()) {
            ("group_increase", _) => NapCatEvent::GroupMemberIncrease(GroupMemberIncreaseEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                operator_id: event.operator_id,
                time: event.time,
                raw_event,
            }),
            ("group_decrease", _) => NapCatEvent::GroupMemberDecrease(GroupMemberDecreaseEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                operator_id: event.operator_id,
                sub_type: event.sub_type,
                time: event.time,
                raw_event,
            }),
            ("notify", "poke") => NapCatEvent::Poke(PokeEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                target_id: event.target_id,
                time: event.time,
                raw_event,
            }),
            _ => {
                debug!(notice_type = %event.notice_type, "忽略尚未建模的群通知");
                return Ok(());
            }
        };

        self.handler.handle(event).await
    }

    async fn handle_message(&self, raw_event: Value) -> Result<(), NapCatError> {
        let event: OneBotGroupMessageEvent = serde_json::from_value(raw_event.clone())
            .map_err(|error| NapCatError::Protocol(format!("invalid message event: {error}")))?;
        if event.message_type != "group" {
            debug!(message_type = %event.message_type, "忽略非群聊消息");
            return Ok(());
        }
        validate_actor_ids(event.group_id, event.user_id, "group message")?;
        if event.message_id.trim().is_empty() {
            return Err(NapCatError::Protocol(
                "group message requires a non-empty message_id".into(),
            ));
        }

        let (normalized_text, at_bot) = normalize_text(&event.raw_message, self.self_qq_id);
        let group_id = event.group_id;
        let user_id = event.user_id;
        let message = GroupMessageEvent {
            message_id: event.message_id,
            group_id,
            user_id,
            segments: parse_message_segments(&event.raw_message),
            raw_message: event.raw_message,
            normalized_text,
            at_bot,
            time: event.time,
            sender: event.sender.map(|sender| SenderInfo {
                nickname: sender.nickname,
                card: sender.card,
                role: sender.role,
            }),
            raw_event,
        };

        info!(group_id, user_id, at_bot, "收到 NapCat 群消息事件");
        self.handler
            .handle(NapCatEvent::GroupMessage(message))
            .await
    }
}

fn validate_actor_ids(group_id: i64, user_id: i64, event_name: &str) -> Result<(), NapCatError> {
    if group_id <= 0 || user_id <= 0 {
        return Err(NapCatError::Protocol(format!(
            "{event_name} requires positive group_id and user_id"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    struct RecordingHandler {
        events: Mutex<Vec<NapCatEvent>>,
    }

    #[async_trait::async_trait]
    impl NapCatEventHandler for RecordingHandler {
        async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    fn listener(handler: Arc<RecordingHandler>) -> NapCatListener {
        NapCatListener::new("ws://127.0.0.1:6700".into(), 10001, handler)
    }

    #[tokio::test]
    async fn numeric_message_id_and_cq_segments_are_preserved() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        listener
            .handle_ws_message(
                r#"{"post_type":"message","message_type":"group","message_id":42,"group_id":7,"user_id":8,"raw_message":"[CQ:at,qq=10001] 你好","time":9}"#,
            )
            .await
            .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::GroupMessage(message) = &events[0] else {
            panic!("expected group message");
        };
        assert_eq!(message.message_id, "42");
        assert_eq!(message.normalized_text, "你好");
        assert!(message.at_bot);
        assert!(matches!(
            message.segments[0],
            super::super::MessageSegment::At { .. }
        ));
    }

    #[tokio::test]
    async fn poke_event_is_forwarded_without_business_filtering() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        listener
            .handle_ws_message(
                r#"{"post_type":"notice","notice_type":"notify","sub_type":"poke","group_id":7,"user_id":8,"target_id":99,"time":9}"#,
            )
            .await
            .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::Poke(event) = &events[0] else {
            panic!("expected poke event");
        };
        assert_eq!(event.target_id, Some(99));
    }

    #[tokio::test]
    async fn invalid_group_message_is_rejected_before_callback() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        let error = listener
            .handle_ws_message(
                r#"{"post_type":"message","message_type":"group","message_id":42,"group_id":0,"user_id":8}"#,
            )
            .await
            .unwrap_err();

        assert!(matches!(error, NapCatError::Protocol(_)));
        assert!(handler.events.lock().unwrap().is_empty());
    }
}
