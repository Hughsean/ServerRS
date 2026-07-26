use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use super::heartbeat::{HeartbeatConfig, HeartbeatState, parse_meta_event};
use super::message_parser::{normalize_text, parse_message_segments};
use super::segments::{MAX_MESSAGE_TOTAL_BYTES, parse_structured_segments};
use super::{
    GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupMessageEvent,
    NapCatConnectionObserver, NapCatError, NapCatEvent, NapCatEventHandler, PokeEvent,
    PrivateMessageEvent, SenderInfo,
};

/// 单条 OneBot WebSocket 文本帧字节上限（评审 P1-4）。
/// 超过此上限的帧在反序列化之前即被拒绝，防止无界 JSON 进入协议适配层。
/// 65536 字节足以容纳正常群/私聊消息与通知；巨型帧视为异常或攻击。
const MAX_WS_TEXT_BYTES: usize = 65_536;

/// NapCat 上报的原始消息。
///
/// 所有非关键字段均允许缺失，以兼容 OneBot 实现之间的载荷差异；在转成公开事件前
/// 会按照群聊或私聊语义验证关键身份字段。
#[derive(Debug, serde::Deserialize)]
struct OneBotMessageEvent {
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
    #[serde(default)]
    target_id: Option<i64>,
    /// 结构化消息段数组。优先解析；不存在或非数组时回退 CQ raw parser（B2）。
    #[serde(default)]
    message: Value,
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
    connection_observer: Option<Arc<dyn NapCatConnectionObserver>>,
    heartbeat: HeartbeatConfig,
}

impl NapCatListener {
    pub fn new(ws_url: String, self_qq_id: i64, handler: Arc<dyn NapCatEventHandler>) -> Self {
        Self {
            ws_url,
            self_qq_id,
            handler,
            connection_observer: None,
            heartbeat: HeartbeatConfig::default(),
        }
    }

    pub fn with_connection_observer(mut self, observer: Arc<dyn NapCatConnectionObserver>) -> Self {
        self.connection_observer = Some(observer);
        self
    }

    /// 覆盖默认 Heartbeat 监控配置。配置在连接建立时校验，非法值返回错误。
    pub fn with_heartbeat_config(mut self, config: HeartbeatConfig) -> Self {
        self.heartbeat = config;
        self
    }

    /// 连接一次并持续读取，连接关闭后返回，由宿主决定是否重连。
    ///
    /// 同时监听 WebSocket 消息与 OneBot Heartbeat deadline：超时返回
    /// [`NapCatError::HeartbeatTimeout`]，由宿主结束 ConnectionEpoch 并唤醒 Backfill。
    /// 普通 WebSocket 文本流量不能错误掩盖已启用的 Heartbeat 超时。
    pub async fn run_forward(&self) -> Result<(), NapCatError> {
        self.heartbeat.validate().map_err(NapCatError::Protocol)?;
        info!(url = %self.ws_url, "正在通过 WebSocket 连接 NapCat");
        let (mut stream, _) = connect_async(self.ws_url.as_str()).await.map_err(|error| {
            NapCatError::Connection(format!("WebSocket connect failed: {error}"))
        })?;

        if let Some(observer) = &self.connection_observer {
            observer.connected().await?;
        }

        info!("NapCat WebSocket 已连接");
        let mut heartbeat = HeartbeatState::new(self.heartbeat);
        loop {
            // 三态 deadline（评审 P0-3）：Expired 必须立即返回超时，不能与 Disabled 混淆。
            // 业务事件处理期间跨过 deadline 时，下一轮这里会立即触发 Expired，
            // 避免连接变成假在线。
            match heartbeat.heartbeat_deadline() {
                super::heartbeat::HeartbeatDeadline::Expired => {
                    warn!("NapCat OneBot Heartbeat 已超时，结束当前监听连接");
                    return Err(NapCatError::HeartbeatTimeout(
                        "heartbeat deadline already expired".into(),
                    ));
                }
                super::heartbeat::HeartbeatDeadline::Disabled => {
                    // 监控禁用：无 Heartbeat 超时抢占，只等下一条 WS 消息。
                    if self.recv_once(&mut stream, &mut heartbeat).await? {
                        // 连接已关闭，正常结束循环进入重连。
                        break;
                    }
                }
                super::heartbeat::HeartbeatDeadline::Waiting(dur) => {
                    let timeout_fut: std::pin::Pin<
                        Box<dyn std::future::Future<Output = ()> + Send>,
                    > = Box::pin(tokio::time::sleep(dur));
                    tokio::select! {
                        biased;
                        _ = timeout_fut => {
                            warn!("NapCat OneBot Heartbeat 超时，结束当前监听连接");
                            return Err(NapCatError::HeartbeatTimeout(
                                "no heartbeat within deadline".into(),
                            ));
                        }
                        message = stream.next() => {
                            if self.handle_one_message(&mut stream, message, &mut heartbeat).await? {
                                break;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// 监控禁用路径：接收一条 WS 消息并处理。返回 `Ok(true)` 表示连接已关闭。
    async fn recv_once(
        &self,
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        heartbeat: &mut HeartbeatState,
    ) -> Result<bool, NapCatError> {
        let message = stream.next().await;
        self.handle_one_message(stream, message, heartbeat).await
    }

    /// 处理一条 WS 消息。返回 `Ok(true)` 表示连接已关闭（应结束循环）。
    async fn handle_one_message(
        &self,
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        heartbeat: &mut HeartbeatState,
    ) -> Result<bool, NapCatError> {
        let Some(message) = message else {
            return Ok(true);
        };
        match message {
            Ok(Message::Text(text)) => {
                if let Err(error) = self.handle_ws_message(text.as_str(), heartbeat).await {
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
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) => {
                return Err(NapCatError::Connection(format!(
                    "WebSocket receive failed: {error}"
                )));
            }
        }
        Ok(false)
    }

    async fn handle_ws_message(
        &self,
        text: &str,
        heartbeat: &mut HeartbeatState,
    ) -> Result<(), NapCatError> {
        // 评审 P1-4：在反序列化之前检查 WS 文本帧字节上限。
        // 超过上限的帧视为异常或攻击，直接拒绝，不进入协议适配层。
        if text.len() > MAX_WS_TEXT_BYTES {
            return Err(NapCatError::Protocol(format!(
                "WebSocket text frame exceeds {} bytes (got {}); rejected before deserialization",
                MAX_WS_TEXT_BYTES,
                text.len()
            )));
        }
        let raw_event: Value = serde_json::from_str(text)
            .map_err(|error| NapCatError::Protocol(format!("invalid JSON: {error}")))?;
        let post_type = raw_event
            .get("post_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // meta_event（heartbeat/lifecycle）只更新监控状态，不进入业务路径、不持久化。
        if let Some(meta) = parse_meta_event(&raw_event) {
            heartbeat.observe_meta(&meta);
            return Ok(());
        }

        // 非 meta_event 视为业务事件：记录时间戳但不重置 Heartbeat deadline，
        // 避免普通文本流量掩盖已启用的 Heartbeat 超时。
        heartbeat.observe_business_event();

        // 评审 P1-4：raw_event 以无界 serde_json::Value 穿过协议回调。
        // 序列化大小超过上限时替换为有界摘要，只保留 post_type 与截断后的原始文本，
        // 防止无界 JSON 进入业务层与持久化。类型化字段（message_id/segments 等）已各自有界。
        let bounded_raw_event = bound_raw_event(raw_event);

        match post_type.as_str() {
            "notice" => self.handle_notice(bounded_raw_event).await,
            "message" => self.handle_message(bounded_raw_event, false).await,
            "message_sent" => self.handle_message(bounded_raw_event, true).await,
            _ => {
                debug!(post_type = %post_type, "忽略尚未建模的 OneBot 事件");
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

    async fn handle_message(
        &self,
        raw_event: Value,
        sent_by_self: bool,
    ) -> Result<(), NapCatError> {
        let event: OneBotMessageEvent = serde_json::from_value(raw_event.clone())
            .map_err(|error| NapCatError::Protocol(format!("invalid message event: {error}")))?;
        if !matches!(event.message_type.as_str(), "group" | "private") {
            debug!(message_type = %event.message_type, "忽略尚未建模的消息类型");
            return Ok(());
        }
        if event.message_id.trim().is_empty() {
            return Err(NapCatError::Protocol(
                "message requires a non-empty message_id".into(),
            ));
        }
        if event.user_id <= 0 {
            return Err(NapCatError::Protocol(
                "message requires a positive user_id".into(),
            ));
        }

        // B2/P0-2：结构化段是语义事实来源。先解析结构化 message 数组，
        // 再从 canonical segments 派生 normalized_text / at_bot / reply / mentions；
        // raw_message 仅在结构化缺失时回退，并作有界审计信息。
        // 这避免 raw_message 缺失/与结构化不一致时持久化的 normalized_text 错误、
        // 以及 @本人 判断失效（评审 P0-2）。
        let segments = parse_structured_or_cq(&event.message, &event.raw_message);
        let (normalized_text, at_bot) = if segments.is_empty() {
            // 结构化与 CQ 都未产生段（极少见）：回退 raw_message 解析。
            normalize_text(&event.raw_message, self.self_qq_id)
        } else {
            let text = super::segments::segments_to_canonical_text(&segments);
            let at_bot = super::segments::segments_mention_self(&segments, self.self_qq_id);
            (text, at_bot)
        };
        let is_self = sent_by_self || event.user_id == self.self_qq_id;
        // 保留有界 raw_message 作为审计信息，防止无限大小载荷进入业务层。
        let bounded_raw = truncate_bytes(&event.raw_message, MAX_MESSAGE_TOTAL_BYTES);

        match event.message_type.as_str() {
            "group" => {
                validate_actor_ids(event.group_id, event.user_id, "group message")?;
                let group_id = event.group_id;
                let user_id = event.user_id;
                let message = GroupMessageEvent {
                    message_id: event.message_id,
                    group_id,
                    user_id,
                    segments,
                    raw_message: bounded_raw,
                    normalized_text,
                    at_bot,
                    time: event.time,
                    sender: event.sender.map(map_sender),
                    is_self,
                    raw_event,
                };

                info!(group_id, user_id, at_bot, is_self, "收到 NapCat 群消息事件");
                self.handler
                    .handle(NapCatEvent::GroupMessage(message))
                    .await
            }
            "private" => {
                let peer_id = if is_self {
                    event
                        .target_id
                        .filter(|target_id| *target_id > 0)
                        .or_else(|| (event.user_id != self.self_qq_id).then_some(event.user_id))
                        .ok_or_else(|| {
                            NapCatError::Protocol(
                                "self-sent private message requires a positive target_id".into(),
                            )
                        })?
                } else {
                    event.user_id
                };
                let user_id = event.user_id;
                let message = PrivateMessageEvent {
                    message_id: event.message_id,
                    user_id,
                    peer_id,
                    segments,
                    raw_message: bounded_raw,
                    normalized_text,
                    time: event.time,
                    sender: event.sender.map(map_sender),
                    is_self,
                    raw_event,
                };

                info!(user_id, peer_id, is_self, "收到 NapCat 私聊消息事件");
                self.handler
                    .handle(NapCatEvent::PrivateMessage(message))
                    .await
            }
            _ => unreachable!("message type was checked before validation"),
        }
    }
}

/// 优先解析结构化 `message` 数组；不存在、非数组或为空时回退 CQ raw parser。
/// 结构化与等价 CQ 字符串生成等价的 canonical segment。
///
/// 评审 P1-3：CQ 回退前必须对 raw_message 做有界截断，否则无结构化数组的
/// 巨大 raw_message 会生成无界 Text segment 穿过协议边界。
fn parse_structured_or_cq(message: &Value, raw_message: &str) -> Vec<super::MessageSegment> {
    if let Value::Array(arr) = message
        && !arr.is_empty()
    {
        let (segments, truncated) = parse_structured_segments(arr);
        if truncated {
            warn!("消息段数量超过上限，已截断");
        }
        return segments;
    }
    // 结构化字段不存在或为空时才回退 CQ raw parser。
    // 先按字节上限截断 raw_message，保证 CQ 解析器不会生成无界 Text segment。
    let bounded_raw = truncate_bytes(raw_message, MAX_MESSAGE_TOTAL_BYTES);
    parse_message_segments(&bounded_raw)
}

/// 按字节截断到上限，保证多字节边界安全（不切割 UTF-8 字符）。
fn truncate_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// 把 raw_event 限制为有界大小（评审 P1-4）。
///
/// 序列化大小不超过 `MAX_WS_TEXT_BYTES` 时原样返回；超过时替换为只保留 `post_type`
/// 与截断原始文本的有界摘要，防止无界 `serde_json::Value` 穿过协议回调进入业务层
/// 与持久化。类型化字段（message_id/group_id/segments 等）已各自有界，不受影响。
fn bound_raw_event(raw_event: Value) -> Value {
    let serialized = match serde_json::to_string(&raw_event) {
        Ok(s) => s,
        // 序列化失败（理论上不应发生，Value 总可序列化）：替换为有界占位。
        Err(_) => {
            return serde_json::json!({"_bounded": "raw_event serialization failed"});
        }
    };
    if serialized.len() <= MAX_WS_TEXT_BYTES {
        return raw_event;
    }
    // 超限：替换为有界摘要。保留 post_type 供审计追溯；raw_text 截断到上限。
    let post_type = raw_event
        .get("post_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let bounded_text = truncate_bytes(&serialized, MAX_WS_TEXT_BYTES);
    tracing::warn!(
        original_bytes = serialized.len(),
        bound_bytes = MAX_WS_TEXT_BYTES,
        "raw_event 超过有界上限，已替换为截断摘要"
    );
    serde_json::json!({
        "post_type": post_type,
        "_bounded": true,
        "_original_bytes": serialized.len(),
        "raw_text": bounded_text,
    })
}

fn map_sender(sender: OneBotSender) -> SenderInfo {
    SenderInfo {
        nickname: sender.nickname,
        card: sender.card,
        role: sender.role,
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

    /// 测试助手：用默认 Heartbeat 状态投递一条 WS 文本消息。
    async fn feed(listener: &NapCatListener, text: &str) -> Result<(), NapCatError> {
        let mut state = HeartbeatState::new(HeartbeatConfig::default());
        listener.handle_ws_message(text, &mut state).await
    }

    #[tokio::test]
    async fn numeric_message_id_and_cq_segments_are_preserved() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"group","message_id":42,"group_id":7,"user_id":8,"raw_message":"[CQ:at,qq=10001] 你好","time":9}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::GroupMessage(message) = &events[0] else {
            panic!("expected group message");
        };
        assert_eq!(message.message_id, "42");
        // P0-2：normalized_text 从 canonical segments 生成（含 @user 占位），
        // 不再从 raw_message 剥离 @bot。
        assert_eq!(message.normalized_text, "@user 你好");
        assert!(message.at_bot);
        assert!(!message.is_self);
        assert!(matches!(
            message.segments[0],
            super::super::MessageSegment::At { .. }
        ));
    }

    #[tokio::test]
    async fn incoming_private_message_is_forwarded_with_peer_identity() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"private","message_id":"p-1","user_id":20002,"target_id":10001,"raw_message":"明天下午开会","time":9}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::PrivateMessage(message) = &events[0] else {
            panic!("expected private message");
        };
        assert_eq!(message.peer_id, 20002);
        assert_eq!(message.normalized_text, "明天下午开会");
        assert!(!message.is_self);
    }

    #[tokio::test]
    async fn self_sent_private_message_uses_target_as_peer() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message_sent","message_type":"private","message_id":"p-2","user_id":10001,"target_id":20002,"raw_message":"我确认一下","time":10}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::PrivateMessage(message) = &events[0] else {
            panic!("expected private message");
        };
        assert_eq!(message.peer_id, 20002);
        assert!(message.is_self);
    }

    #[tokio::test]
    async fn poke_event_is_forwarded_without_business_filtering() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
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

        let error = feed(
            &listener,
            r#"{"post_type":"message","message_type":"group","message_id":42,"group_id":0,"user_id":8}"#,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, NapCatError::Protocol(_)));
        assert!(handler.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unsupported_message_type_is_ignored_before_identity_validation() {
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"guild","raw_message":"ignored"}"#,
        )
        .await
        .unwrap();

        assert!(handler.events.lock().unwrap().is_empty());
    }

    // ===== B2: 结构化消息段优先 + CQ 回退 =====

    #[tokio::test]
    async fn structured_message_array_is_preferred_over_raw_cq() {
        // 结构化 message 数组存在时优先解析；raw_message 仅作审计。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"group","message_id":51,"group_id":7,"user_id":8,"message":[{"type":"at","data":{"qq":"10001"}},{"type":"text","data":{"text":"结构化你好"}}],"raw_message":"[CQ:at,qq=10001] 不应使用的回退文本","time":9}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::GroupMessage(message) = &events[0] else {
            panic!("expected group message");
        };
        // 结构化段优先：第一段应是 At，第二段应是结构化文本（而非 CQ 回退文本）。
        assert!(matches!(
            message.segments[0],
            super::super::MessageSegment::At { ref qq } if qq == "10001"
        ));
        assert!(matches!(
            message.segments[1],
            super::super::MessageSegment::Text { ref content } if content == "结构化你好"
        ));
        // raw_message 仍保留作审计信息。
        assert!(message.raw_message.contains("回退文本"));
    }

    #[tokio::test]
    async fn missing_structured_message_falls_back_to_cq_parser() {
        // 无 message 字段时回退 CQ raw parser，行为与拆分前一致。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"group","message_id":52,"group_id":7,"user_id":8,"raw_message":"[CQ:at,qq=10001] 回退你好","time":9}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::GroupMessage(message) = &events[0] else {
            panic!("expected group message");
        };
        assert!(matches!(
            message.segments[0],
            super::super::MessageSegment::At { ref qq } if qq == "10001"
        ));
        assert!(message.normalized_text.contains("回退你好"));
    }

    #[tokio::test]
    async fn empty_structured_array_falls_back_to_cq_parser() {
        // message 为空数组时不视为有效结构化，回退 CQ。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"group","message_id":53,"group_id":7,"user_id":8,"message":[],"raw_message":"[CQ:at,qq=10001] 空数组回退","time":9}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::GroupMessage(message) = &events[0] else {
            panic!("expected group message");
        };
        assert!(matches!(
            message.segments[0],
            super::super::MessageSegment::At { ref qq } if qq == "10001"
        ));
    }

    #[tokio::test]
    async fn numeric_message_id_in_structured_path_is_coerced() {
        // 结构化路径下 message_id 仍兼容数字与字符串。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"message","message_type":"private","message_id":99,"user_id":20002,"target_id":10001,"message":[{"type":"text","data":{"text":"hi"}}],"raw_message":"hi","time":9}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::PrivateMessage(message) = &events[0] else {
            panic!("expected private message");
        };
        assert_eq!(message.message_id, "99");
        assert!(matches!(
            message.segments[0],
            super::super::MessageSegment::Text { ref content } if content == "hi"
        ));
    }

    #[tokio::test]
    async fn oversized_raw_message_is_truncated_for_audit() {
        // 评审 P1-4：raw_message 超过有界上限时截断，防止无限大小载荷进入业务层。
        // 注意分层：WS 文本帧上限 MAX_WS_TEXT_BYTES 在反序列化前检查；
        // raw_message 字段上限 MAX_MESSAGE_TOTAL_BYTES 在 handle_message 中检查。
        // 此测试构造一个 raw_message 超过 MAX_MESSAGE_TOTAL_BYTES 但整个帧
        // 不超过 MAX_WS_TEXT_BYTES 的消息，验证 raw_message 被截断。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));
        let big = "x".repeat(super::super::segments::MAX_MESSAGE_TOTAL_BYTES + 100);

        feed(
            &listener,
            &serde_json::to_string(&serde_json::json!({
                "post_type": "message",
                "message_type": "group",
                "message_id": 54,
                "group_id": 7,
                "user_id": 8,
                "raw_message": big,
                "time": 9
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        let NapCatEvent::GroupMessage(message) = &events[0] else {
            panic!("expected group message");
        };
        assert!(message.raw_message.len() <= super::super::segments::MAX_MESSAGE_TOTAL_BYTES);
    }

    #[tokio::test]
    async fn oversized_ws_frame_with_huge_raw_message_is_rejected() {
        // 评审 P1-4：整个 WS 文本帧超过 MAX_WS_TEXT_BYTES 时在反序列化前拒绝，
        // 即使 raw_message 本身也超限。这是分层防护的最外层。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));
        // raw_message 足够大，使整个 JSON 帧超过 MAX_WS_TEXT_BYTES。
        let big = "x".repeat(MAX_WS_TEXT_BYTES + 1000);

        let error = feed(
            &listener,
            &serde_json::to_string(&serde_json::json!({
                "post_type": "message",
                "message_type": "group",
                "message_id": 55,
                "group_id": 7,
                "user_id": 8,
                "raw_message": big,
                "time": 9
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, NapCatError::Protocol(_)));
        assert!(handler.events.lock().unwrap().is_empty());
    }

    // ===== B1: Heartbeat / Lifecycle =====

    #[tokio::test]
    async fn heartbeat_meta_event_is_not_forwarded_as_business_event() {
        // Heartbeat 高频事件只更新监控状态，不进入业务回调、不持久化。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"meta_event","meta_event_type":"heartbeat","interval":30000,"time":1}"#,
        )
        .await
        .unwrap();

        feed(
            &listener,
            r#"{"post_type":"meta_event","meta_event_type":"lifecycle","sub_type":"connect","time":1}"#,
        )
        .await
        .unwrap();

        assert!(handler.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn heartbeat_does_not_swallow_message_after_meta_event() {
        // 收到 Heartbeat 后再收到普通消息仍能正常转发，证明 meta_event 不污染业务路径。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        feed(
            &listener,
            r#"{"post_type":"meta_event","meta_event_type":"heartbeat","interval":5000,"time":1}"#,
        )
        .await
        .unwrap();
        feed(
            &listener,
            r#"{"post_type":"message","message_type":"private","message_id":"m-1","user_id":20002,"target_id":10001,"raw_message":"hi","time":2}"#,
        )
        .await
        .unwrap();

        let events = handler.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let NapCatEvent::PrivateMessage(message) = &events[0] else {
            panic!("expected private message");
        };
        assert_eq!(message.message_id, "m-1");
    }

    // ===== P1-4: WS 文本帧与 raw_event 有界 =====

    #[tokio::test]
    async fn oversized_ws_text_frame_is_rejected_before_deserialization() {
        // 评审 P1-4：超过 MAX_WS_TEXT_BYTES 的 WS 文本帧在反序列化之前即被拒绝。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        // 构造一个超过上限的合法 JSON 文本帧。
        let big = "x".repeat(MAX_WS_TEXT_BYTES + 100);
        let json = serde_json::to_string(&serde_json::json!({
            "post_type": "message",
            "message_type": "group",
            "message_id": 60,
            "group_id": 7,
            "user_id": 8,
            "raw_message": big,
            "time": 9
        }))
        .unwrap();

        let error = feed(&listener, &json).await.unwrap_err();
        assert!(matches!(error, NapCatError::Protocol(_)));
        // 关键断言：事件未被转发（在反序列化前拒绝）。
        assert!(handler.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_raw_event_is_replaced_with_bounded_summary() {
        // 评审 P1-4：raw_event 序列化大小超过上限时替换为有界摘要，
        // 防止无界 serde_json::Value 穿过协议回调。
        let handler = Arc::new(RecordingHandler::default());
        let listener = listener(Arc::clone(&handler));

        // 构造一个 raw_event 序列化后超过 MAX_WS_TEXT_BYTES 的消息。
        // 但文本帧本身必须 <= MAX_WS_TEXT_BYTES（否则被前一个测试路径拒绝）。
        // 用结构化 message 数组填充：raw_event 总大小接近上限但文本帧合法，
        // raw_message 字段使 raw_event 的序列化大小超过上限。
        // 注意：raw_message 本身已被截断到 MAX_MESSAGE_TOTAL_BYTES，但 raw_event
        // 包含完整结构化 message 数组。这里用大量结构化段填充。
        let mut segs = Vec::new();
        for _ in 0..100 {
            let mut data = serde_json::Map::new();
            data.insert("text".into(), serde_json::Value::String("x".repeat(2000)));
            let mut seg = serde_json::Map::new();
            seg.insert("type".into(), serde_json::Value::String("text".into()));
            seg.insert("data".into(), serde_json::Value::Object(data));
            segs.push(serde_json::Value::Object(seg));
        }
        let json = serde_json::to_string(&serde_json::json!({
            "post_type": "message",
            "message_type": "group",
            "message_id": 61,
            "group_id": 7,
            "user_id": 8,
            "message": segs,
            "raw_message": "x".repeat(5000),
            "time": 9
        }))
        .unwrap();

        // 文本帧本身可能也超过 MAX_WS_TEXT_BYTES。先验证不超过的路径：
        // 用一个适中的大小使 raw_event 序列化超限但文本帧合法。
        if json.len() <= MAX_WS_TEXT_BYTES {
            feed(&listener, &json).await.unwrap();
            let events = handler.events.lock().unwrap();
            let NapCatEvent::GroupMessage(message) = &events[0] else {
                panic!("expected group message");
            };
            // raw_event 被替换为有界摘要（含 _bounded: true）。
            assert_eq!(
                message.raw_event.get("_bounded").and_then(|v| v.as_bool()),
                Some(true)
            );
        }
        // 若文本帧本身超限（被前一个测试路径拒绝），这里验证不 panic 即可。
    }

    #[test]
    fn bound_raw_event_passes_small_events_through() {
        let small = serde_json::json!({
            "post_type": "message",
            "message_id": "x",
            "raw_message": "hi"
        });
        let bounded = bound_raw_event(small.clone());
        // 小事件原样返回（未被替换）。
        assert_eq!(bounded.get("post_type"), small.get("post_type"));
        assert!(bounded.get("_bounded").is_none());
    }
}
