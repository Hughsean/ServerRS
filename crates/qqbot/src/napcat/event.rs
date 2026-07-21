use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::NapCatError;

/// OneBot 消息中的单个协议段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageSegment {
    Text { content: String },
    Face { id: i32, text: Option<String> },
    Image { file: String, url: Option<String> },
    At { qq: String },
    Reply { id: String },
    Record { file: String },
    Video { file: String },
    File { file: String, name: Option<String> },
    Unknown { raw: String },
}

/// OneBot 上报的发送者资料；字段缺失时保留默认值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SenderInfo {
    pub nickname: String,
    pub card: Option<String>,
    pub role: Option<String>,
}

/// 已解析但尚未进入任何业务流程的群消息事件。
#[derive(Debug, Clone)]
pub struct GroupMessageEvent {
    pub message_id: String,
    pub group_id: i64,
    pub user_id: i64,
    pub raw_message: String,
    pub normalized_text: String,
    pub segments: Vec<MessageSegment>,
    pub at_bot: bool,
    pub time: i64,
    pub sender: Option<SenderInfo>,
    /// 保留完整协议载荷，供未来业务按需解析未建模字段。
    pub raw_event: Value,
}

#[derive(Debug, Clone)]
pub struct GroupMemberIncreaseEvent {
    pub group_id: i64,
    pub user_id: i64,
    pub operator_id: Option<i64>,
    pub time: i64,
    pub raw_event: Value,
}

#[derive(Debug, Clone)]
pub struct GroupMemberDecreaseEvent {
    pub group_id: i64,
    pub user_id: i64,
    pub operator_id: Option<i64>,
    pub sub_type: String,
    pub time: i64,
    pub raw_event: Value,
}

#[derive(Debug, Clone)]
pub struct PokeEvent {
    pub group_id: i64,
    pub user_id: i64,
    pub target_id: Option<i64>,
    pub time: i64,
    pub raw_event: Value,
}

/// NapCat 适配器向未来业务层暴露的协议事件集合。
#[derive(Debug, Clone)]
pub enum NapCatEvent {
    GroupMessage(GroupMessageEvent),
    GroupMemberIncrease(GroupMemberIncreaseEvent),
    GroupMemberDecrease(GroupMemberDecreaseEvent),
    Poke(PokeEvent),
}

/// 未来 QQBot 业务接入 NapCat 的唯一回调边界。
#[async_trait]
pub trait NapCatEventHandler: Send + Sync {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError>;
}
