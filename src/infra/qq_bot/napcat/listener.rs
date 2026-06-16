use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::domain::qq_bot::QqBotError;
use crate::domain::qq_bot::message::{
    MessageDirection, NormalizedMessage,
};

use super::message_parser::{ParsedEvent, normalize_text, parse_message_segments};

/// Raw OneBot group message event.
///
/// All fields use `#[serde(default)]` so a single unknown field never causes the
/// whole event to be dropped — this is critical because NapCat may emit fields
/// (e.g. `message` as an array, extra metadata) that we don't model.
#[derive(Debug, serde::Deserialize)]
struct OneBotGroupMessageEvent {
    #[serde(default)]
    post_type: String,
    #[serde(default)]
    message_type: String,
    #[allow(dead_code)]
    #[serde(default)]
    sub_type: String,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_message_id")]
    message_id: String,
    #[serde(default)]
    group_id: i64,
    #[serde(default)]
    user_id: i64,
    /// NapCat may send this as a string (CQ codes) or an array (message segments).
    /// We don't use it, so we accept any form and discard it.
    #[allow(dead_code)]
    #[serde(default)]
    message: serde_json::Value,
    #[serde(default)]
    raw_message: String,
    #[serde(default)]
    time: i64,
    #[allow(dead_code)]
    #[serde(default)]
    sender: Option<OneBotSender>,
}

#[derive(Debug, serde::Deserialize)]
struct OneBotSender {
    #[allow(dead_code)]
    #[serde(default)]
    nickname: String,
    #[allow(dead_code)]
    #[serde(default)]
    card: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    role: Option<String>,
}

/// Deserialize message_id which can be either a number or string in OneBot v11.
fn deserialize_message_id<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<String, D::Error> {
    use serde::de;
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a number or string")
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }
    }
    d.deserialize_any(V)
}

/// Raw OneBot group notice event (member join/leave, poke, etc.).
#[derive(Debug, serde::Deserialize)]
struct OneBotNoticeEvent {
    #[serde(default)]
    post_type: String,
    #[serde(default)]
    notice_type: String,
    #[serde(default)]
    group_id: i64,
    #[serde(default)]
    user_id: i64,
    #[serde(default)]
    sub_type: String,
    #[allow(dead_code)]
    #[serde(default)]
    operator_id: Option<i64>,
    /// Who was poked (for notify/poke events).
    #[serde(default)]
    target_id: Option<i64>,
    #[serde(default)]
    time: i64,
}

/// Event handler trait for processed group messages.
#[async_trait::async_trait]
pub trait GroupMessageHandler: Send + Sync {
    async fn handle_group_message(&self, msg: NormalizedMessage, raw_json: Value);
}

/// Event handler trait for group notice events (member join/leave).
#[async_trait::async_trait]
pub trait GroupNoticeHandler: Send + Sync {
    async fn handle_group_increase(
        &self,
        group_id: i64,
        user_id: i64,
        operator_id: Option<i64>,
    ) -> Result<(), QqBotError>;

    async fn handle_group_decrease(
        &self,
        group_id: i64,
        user_id: i64,
        sub_type: &str,
    ) -> Result<(), QqBotError>;
}

/// NapCat WebSocket listener that connects via **forward WebSocket**.
/// We connect to NapCat and process both message and notice events.
pub struct NapCatListener {
    /// WebSocket URL (e.g. "ws://127.0.0.1:6700" for forward WS).
    ws_url: String,
    /// Bot's QQ number for identifying @-mentions.
    self_qq_id: i64,
    /// Token channel for sending API requests via the same WS connection.
    api_tx: Option<mpsc::UnboundedSender<Value>>,
    /// Handler for processed messages.
    handler: Arc<dyn GroupMessageHandler>,
    /// Handler for notice events (group join/leave).
    notice_handler: Option<Arc<dyn GroupNoticeHandler>>,
}

impl NapCatListener {
    pub fn new(ws_url: String, self_qq_id: i64, handler: Arc<dyn GroupMessageHandler>) -> Self {
        Self {
            ws_url,
            self_qq_id,
            api_tx: None,
            handler,
            notice_handler: None,
        }
    }

    /// Set the notice handler after construction.
    pub fn with_notice_handler(mut self, notice_handler: Arc<dyn GroupNoticeHandler>) -> Self {
        self.notice_handler = Some(notice_handler);
        self
    }

    /// Start listening via forward WebSocket (we connect to NapCat).
    pub async fn run_forward(&mut self) -> Result<(), QqBotError> {
        info!(url = %self.ws_url, "connecting to NapCat via WebSocket");
        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .map_err(|e| QqBotError::Connection(format!("WebSocket connect failed: {e}")))?;

        info!("NapCat WebSocket connected");
        let (mut write, mut read) = ws_stream.split();

        let (api_tx, mut api_rx) = mpsc::unbounded_channel::<Value>();
        self.api_tx = Some(api_tx);

        // Spawn a task to forward API requests through WS
        let write_handle = tokio::spawn(async move {
            while let Some(msg) = api_rx.recv().await {
                let text = serde_json::to_string(&msg).unwrap_or_default();
                if let Err(e) = write.send(Message::Text(text.into())).await {
                    error!("failed to send WS message: {e}");
                    break;
                }
            }
        });

        // Read loop
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_ws_message(&text).await {
                        warn!("error handling WS message: {e}");
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("NapCat WebSocket closed");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    // pong is handled automatically by tungstenite
                    let _ = data;
                }
                Err(e) => {
                    error!("NapCat WebSocket error: {e}");
                    break;
                }
                _ => {}
            }
        }

        write_handle.abort();
        info!("NapCat listener stopped");
        Ok(())
    }

    /// Handle a raw message from the WebSocket.
    async fn handle_ws_message(&self, text: &str) -> Result<(), QqBotError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| QqBotError::MessageProcessing(format!("invalid JSON: {e}")))?;

        // Try parsing as a notice event first (member join/leave)
        if let Ok(event) = serde_json::from_value::<OneBotNoticeEvent>(value.clone()) {
            if event.post_type == "notice" {
                match event.notice_type.as_str() {
                    "group_increase" => {
                        if let Some(ref handler) = self.notice_handler {
                            if let Err(e) = handler
                                .handle_group_increase(
                                    event.group_id,
                                    event.user_id,
                                    event.operator_id,
                                )
                                .await
                            {
                                warn!(
                                    group_id = event.group_id,
                                    user_id = event.user_id,
                                    error = %e,
                                    "handle_group_increase failed"
                                );
                            }
                        }
	                }
                    "group_decrease" => {
                        if let Some(ref handler) = self.notice_handler {
                            if let Err(e) = handler
                                .handle_group_decrease(
                                    event.group_id,
                                    event.user_id,
                                    &event.sub_type,
                                )
                                .await
                            {
                                warn!(
                                    group_id = event.group_id,
                                    user_id = event.user_id,
                                    sub_type = %event.sub_type,
                                    error = %e,
                                    "handle_group_decrease failed"
                                );
                            }
                        }
                    }
                    "notify" if event.sub_type == "poke" => {
                        // Only respond if the bot itself was poked
                        if event.target_id == Some(self.self_qq_id) {
                            let synthetic_msg = NormalizedMessage {
                                id: None,
                                bot_account_id: 0,
                                qq_group_id: event.group_id,
                                qq_user_id: Some(event.user_id),
                                platform_message_id: format!(
                                    "poke_{}_{}_{}",
                                    event.group_id, event.user_id, event.time
                                ),
                                direction: MessageDirection::Inbound,
                                raw_text: format!(
                                    "用户{}戳了戳你",
                                    event.user_id
                                ),
                                normalized_text: format!(
                                    "用户{}戳了戳你",
                                    event.user_id
                                ),
                                segments: vec![],
                                at_bot: true,
                                command_name: Some("poke".to_string()),
                                sent_at: event.time,
                            };
                            self.handler
                                .handle_group_message(synthetic_msg, value.clone())
                                .await;
                        }
                    }
                    _ => {
                        debug!(
                            notice_type = %event.notice_type,
                            "unhandled notice event"
                        );
                    }
                }
                return Ok(());
            }
        }

        // Check if it's a group message event
        match serde_json::from_value::<OneBotGroupMessageEvent>(value.clone()) {
            Ok(event) if event.post_type == "message" && event.message_type == "group" => {
                let parsed = self.parse_event(&event);
                let raw_text = event.raw_message.clone();

                let (normalized_text, at_bot, command_name) =
                    normalize_text(&raw_text, self.self_qq_id);

                // Diagnostic: detect @bot directly from raw before normalize_text
                let direct_at_bot = {
                    let pat1 = format!("[CQ:at,qq={}]", self.self_qq_id);
                    let pat2 = format!("[CQ:at,qq={}", self.self_qq_id); // match even if extra params
                    raw_text.contains(&pat1) || raw_text.contains(&pat2)
                };
                info!(
                    self_qq_id = self.self_qq_id,
                    raw_len = raw_text.len(),
                    at_bot_detected = at_bot,
                    direct_at_bot,
                    "WS: at-bot detection diagnostic"
                );

                let segments = parse_message_segments(&raw_text);

                let msg = NormalizedMessage {
                    id: None,
                    bot_account_id: 0, // to be filled by caller
                    qq_group_id: parsed.qq_group_id,
                    qq_user_id: Some(parsed.qq_user_id),
                    platform_message_id: parsed.platform_message_id,
                    direction: crate::domain::qq_bot::message::MessageDirection::Inbound,
                    raw_text,
                    normalized_text,
                    segments,
                    at_bot,
                    command_name,
                    sent_at: parsed.sent_at,
                };

                info!(
                    group_id = parsed.qq_group_id,
                    user_id = parsed.qq_user_id,
                    at_bot,
                    raw = %event.raw_message,
                    "WS: received group message event"
                );

                self.handler.handle_group_message(msg, value).await;
                return Ok(());
            }
            Ok(other) => {
                debug!(
                    post_type = %other.post_type,
                    message_type = %other.message_type,
                    "WS: non-group-message event ignored"
                );
            }
            Err(e) => {
                // Parse failed — log full payload to diagnose schema mismatch
                warn!(
                    error = %e,
                    payload = %value,
                    "WS: failed to parse as OneBotGroupMessageEvent"
                );
            }
        }

        Ok(())
    }

    fn parse_event(&self, event: &OneBotGroupMessageEvent) -> ParsedEvent {
        ParsedEvent {
            qq_group_id: event.group_id,
            qq_user_id: event.user_id,
            raw_text: event.raw_message.clone(),
            normalized_text: String::new(),
            segments: Vec::new(),
            at_bot: false,
            command_name: None,
            platform_message_id: event.message_id.clone(),
            sent_at: event.time,
        }
    }
}
