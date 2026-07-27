//! 通知事件映射：OneBot notice 事件 DTO 解析与 `NapCatEvent::*` 构造。
//!
//! 覆盖 `group_increase`/`group_decrease`/`notify::poke`/`group_recall`/`friend_recall`。
//! 撤回通知携带 `message_id`，用于关联被撤回的原消息。

use serde_json::Value;
use tracing::debug;

use super::super::{
    FriendRecallEvent, GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupRecallEvent,
    NapCatError, NapCatEvent, NapCatEventHandler, PokeEvent,
};
use super::bounds::validate_actor_ids;
use super::message_event::deserialize_message_id;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct OneBotNoticeEvent {
    #[serde(default)]
    pub(crate) notice_type: String,
    #[serde(default)]
    pub(crate) group_id: i64,
    #[serde(default)]
    pub(crate) user_id: i64,
    #[serde(default)]
    pub(crate) sub_type: String,
    #[serde(default)]
    pub(crate) operator_id: Option<i64>,
    #[serde(default)]
    pub(crate) target_id: Option<i64>,
    /// 撤回通知携带的 message_id。兼容字符串与数字（任务七-2）。
    /// 非 撤回通知通常无此字段，反序列化为空字符串。
    #[serde(default, deserialize_with = "deserialize_message_id")]
    pub(crate) message_id: String,
    #[serde(default)]
    pub(crate) time: i64,
}

/// 处理一条通知事件，构造对应的 `NapCatEvent` 并交给回调。
///
/// 撤回通知（`group_recall`/`friend_recall`）的 `message_id` 是被撤回原消息的平台 ID。
/// `friend_recall` 合法地无 `group_id`，因此对 friend recall 放宽 `validate_actor_ids` 校验。
pub(crate) async fn handle_notice(
    handler: &dyn NapCatEventHandler,
    raw_event: Value,
) -> Result<(), NapCatError> {
    let event: OneBotNoticeEvent = serde_json::from_value(raw_event.clone())
        .map_err(|error| NapCatError::Protocol(format!("invalid notice event: {error}")))?;

    let event = match (event.notice_type.as_str(), event.sub_type.as_str()) {
        ("group_increase", _) => {
            validate_actor_ids(event.group_id, event.user_id, "group_increase notice")?;
            NapCatEvent::GroupMemberIncrease(GroupMemberIncreaseEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                operator_id: event.operator_id,
                time: event.time,
                raw_event,
            })
        }
        ("group_decrease", _) => {
            validate_actor_ids(event.group_id, event.user_id, "group_decrease notice")?;
            NapCatEvent::GroupMemberDecrease(GroupMemberDecreaseEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                operator_id: event.operator_id,
                sub_type: event.sub_type,
                time: event.time,
                raw_event,
            })
        }
        ("notify", "poke") => {
            validate_actor_ids(event.group_id, event.user_id, "poke notice")?;
            NapCatEvent::Poke(PokeEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                target_id: event.target_id,
                time: event.time,
                raw_event,
            })
        }
        ("group_recall", _) => {
            // 群撤回：group_id 和 user_id（被撤回消息的发送者）必须为正。
            // message_id 是被撤回原消息的平台 ID，用于关联。
            validate_actor_ids(event.group_id, event.user_id, "group_recall notice")?;
            if event.message_id.trim().is_empty() {
                return Err(NapCatError::Protocol(
                    "group_recall notice requires a non-empty message_id".into(),
                ));
            }
            NapCatEvent::GroupRecall(GroupRecallEvent {
                group_id: event.group_id,
                user_id: event.user_id,
                operator_id: event.operator_id,
                message_id: event.message_id,
                time: event.time,
                raw_event,
            })
        }
        ("friend_recall", _) => {
            // 好友撤回：合法地无 group_id（私聊场景）。
            // 只校验 user_id（撤回消息的好友）必须为正。
            if event.user_id <= 0 {
                return Err(NapCatError::Protocol(
                    "friend_recall notice requires a positive user_id".into(),
                ));
            }
            if event.message_id.trim().is_empty() {
                return Err(NapCatError::Protocol(
                    "friend_recall notice requires a non-empty message_id".into(),
                ));
            }
            NapCatEvent::FriendRecall(FriendRecallEvent {
                user_id: event.user_id,
                message_id: event.message_id,
                time: event.time,
                raw_event,
            })
        }
        _ => {
            debug!(notice_type = %event.notice_type, "忽略尚未建模的群通知");
            return Ok(());
        }
    };

    handler.handle(event).await
}
