//! 消息事件映射：OneBot 消息事件 DTO 解析与 `NapCatEvent::Group/PrivateMessage` 构造。
//!
//! 仅负责类型化协议解析，不依赖数据库、个人秘书业务规则或 LLM。
//! `message_id` 兼容字符串与数字；结构化段优先，CQ raw 回退；raw_message 有界审计。

use serde_json::Value;
use tracing::info;

use super::super::message_parser::normalize_text;
use super::super::segments::MAX_MESSAGE_TOTAL_BYTES;
use super::super::{
    GroupMessageEvent, NapCatError, NapCatEvent, NapCatEventHandler, PrivateMessageEvent,
    SenderInfo,
};
use super::bounds::{parse_structured_or_cq, truncate_bytes, validate_actor_ids};

/// NapCat 上报的原始消息。
///
/// 所有非关键字段均允许缺失，以兼容 OneBot 实现之间的载荷差异；在转成公开事件前
/// 会按照群聊或私聊语义验证关键身份字段。
#[derive(Debug, serde::Deserialize)]
pub(crate) struct OneBotMessageEvent {
    #[serde(default)]
    pub(crate) message_type: String,
    #[serde(default, rename = "sub_type")]
    pub(crate) _sub_type: String,
    #[serde(default, deserialize_with = "deserialize_message_id")]
    pub(crate) message_id: String,
    #[serde(default)]
    pub(crate) group_id: i64,
    #[serde(default)]
    pub(crate) user_id: i64,
    #[serde(default)]
    pub(crate) target_id: Option<i64>,
    /// 结构化消息段数组。优先解析；不存在或非数组时回退 CQ raw parser（B2）。
    #[serde(default)]
    pub(crate) message: Value,
    #[serde(default)]
    pub(crate) raw_message: String,
    #[serde(default)]
    pub(crate) time: i64,
    #[serde(default)]
    pub(crate) sender: Option<OneBotSender>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct OneBotSender {
    #[serde(default)]
    pub(crate) nickname: String,
    #[serde(default)]
    pub(crate) card: Option<String>,
    #[serde(default)]
    pub(crate) role: Option<String>,
}

/// OneBot 的 message_id 在不同实现中可能是数字、字符串或 null。
pub(crate) fn deserialize_message_id<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<String, D::Error> {
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

pub(crate) fn map_sender(sender: OneBotSender) -> SenderInfo {
    SenderInfo {
        nickname: sender.nickname,
        card: sender.card,
        role: sender.role,
    }
}

/// 处理一条消息事件，构造 `NapCatEvent::Group/PrivateMessage` 并交给回调。
///
/// `sent_by_self` 表示这是 `message_sent`（本人发送）上报，需用 `target_id` 推断对端。
pub(crate) async fn handle_message(
    handler: &dyn NapCatEventHandler,
    self_qq_id: i64,
    raw_event: Value,
    sent_by_self: bool,
) -> Result<(), NapCatError> {
    let event: OneBotMessageEvent = serde_json::from_value(raw_event.clone())
        .map_err(|error| NapCatError::Protocol(format!("invalid message event: {error}")))?;
    if !matches!(event.message_type.as_str(), "group" | "private") {
        tracing::debug!(message_type = %event.message_type, "忽略尚未建模的消息类型");
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
        normalize_text(&event.raw_message, self_qq_id)
    } else {
        let text = super::super::segments::segments_to_canonical_text(&segments);
        let at_bot = super::super::segments::segments_mention_self(&segments, self_qq_id);
        (text, at_bot)
    };
    let is_self = sent_by_self || event.user_id == self_qq_id;
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
            handler.handle(NapCatEvent::GroupMessage(message)).await
        }
        "private" => {
            let peer_id = if is_self {
                event
                    .target_id
                    .filter(|target_id| *target_id > 0)
                    .or_else(|| (event.user_id != self_qq_id).then_some(event.user_id))
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
            handler.handle(NapCatEvent::PrivateMessage(message)).await
        }
        _ => unreachable!("message type was checked before validation"),
    }
}
