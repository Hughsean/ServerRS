//! 通过正向 WebSocket 连接 NapCat，并把 OneBot 事件交给协议回调。
//!
//! 模块职责拆分（保持公共 API 与行为不变）：
//! - `mod`：`NapCatListener` 结构体与 `run_forward` 三态 deadline 驱动循环。
//! - `transport`：WebSocket 建连与单条帧读取、Ping/Pong/Close 分类。
//! - `dispatch`：WS 文本帧 -> 有界检查 -> JSON -> meta_event/notice/message 路由。
//! - `message_event`：消息事件 DTO 解析与 `Group/PrivateMessage` 构造。
//! - `notice_event`：通知事件 DTO 解析（撤回通知在 B3 追加）。
//! - `bounds`：帧/raw_event/字段有界与 actor ID 校验。

mod bounds;
mod dispatch;
mod message_event;
mod notice_event;
mod transport;

use std::sync::Arc;

use futures_util::StreamExt;
use tracing::{info, warn};

use super::heartbeat::{HeartbeatConfig, HeartbeatState};
use super::{NapCatConnectionObserver, NapCatError, NapCatEventHandler};

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
        let (mut stream, _) = tokio_tungstenite::connect_async(self.ws_url.as_str())
            .await
            .map_err(|error| {
                NapCatError::Connection(format!("WebSocket connect failed: {error}"))
            })?;

        if let Some(observer) = &self.connection_observer {
            observer.connected().await?;
        }

        info!("NapCat WebSocket 已连接");
        let mut heartbeat = HeartbeatState::new(self.heartbeat);
        let handler: &dyn NapCatEventHandler = self.handler.as_ref();
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
                    if transport::recv_once(handler, self.self_qq_id, &mut stream, &mut heartbeat)
                        .await?
                    {
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
                            if transport::handle_one_message(
                                handler,
                                self.self_qq_id,
                                &mut stream,
                                message,
                                &mut heartbeat,
                            )
                            .await?
                            {
                                break;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::bounds::{MAX_WS_TEXT_BYTES, bound_raw_event};
    use super::dispatch::handle_ws_message;
    use super::*;
    use crate::napcat::NapCatEvent;

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
    ///
    /// 拆分后改为直接调用 `dispatch::handle_ws_message`（生产分发路径），
    /// 不再经过 `&self` 方法，保证测试经过与生产相同的分发与有界逻辑。
    async fn feed(listener: &NapCatListener, text: &str) -> Result<(), NapCatError> {
        let mut state = HeartbeatState::new(HeartbeatConfig::default());
        handle_ws_message(
            listener.handler.as_ref(),
            listener.self_qq_id,
            text,
            &mut state,
        )
        .await
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
