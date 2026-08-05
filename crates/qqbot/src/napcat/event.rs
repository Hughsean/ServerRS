use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::NapCatError;

/// 富消息 envelope 类型。只保存有限描述，不保存完整载荷（B2 约束）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RichKind {
    Json,
    Xml,
    Card,
    Other,
}

/// OneBot 消息中的单个协议段。
///
/// 变体与 NapCat.Onebot.yaml 段类型对齐。未知段保留类型名与有界原始 JSON，
/// 不静默删除；不允许无限大小未知 JSON 进入业务层（B2 约束）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageSegment {
    Text {
        content: String,
    },
    Face {
        id: i32,
        text: Option<String>,
    },
    Image {
        file: String,
        url: Option<String>,
    },
    At {
        qq: String,
    },
    Reply {
        id: String,
    },
    Record {
        file: String,
    },
    Video {
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    File {
        file: String,
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    /// 合并转发引用：只保留转发 ID，不下载全部内容。
    Forward {
        id: String,
    },
    /// JSON/XML/卡片等富消息的有界 envelope：只保留类型与有限元数据，不存全文。
    Rich {
        kind: RichKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        /// SHA-256 of the complete, untruncated structured payload, domain-separated by kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_sha256: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// 未知段：保留类型名与有界原始 JSON 片段，不静默删除。
    Unknown {
        seg_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<String>,
    },
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
    /// 该消息是否由当前登录的个人 QQ 账号发出。
    pub is_self: bool,
    /// 保留完整协议载荷，供未来业务按需解析未建模字段。
    pub raw_event: Value,
}

/// 已解析但尚未进入任何业务流程的私聊消息事件。
#[derive(Debug, Clone)]
pub struct PrivateMessageEvent {
    pub message_id: String,
    /// 协议载荷中的发送者 ID。
    pub user_id: i64,
    /// 当前个人账号正在与谁对话；对本人发出的消息优先取 target_id。
    pub peer_id: i64,
    pub raw_message: String,
    pub normalized_text: String,
    pub segments: Vec<MessageSegment>,
    pub time: i64,
    pub sender: Option<SenderInfo>,
    pub is_self: bool,
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

/// 群消息撤回通知。`operator_id` 是执行撤回的人（群主/管理员或发送者本人），
/// `user_id` 是被撤回消息的发送者，`message_id` 是被撤回消息的平台 ID。
#[derive(Debug, Clone)]
pub struct GroupRecallEvent {
    pub group_id: i64,
    pub user_id: i64,
    pub operator_id: Option<i64>,
    pub message_id: String,
    pub time: i64,
    pub raw_event: Value,
}

/// 好友消息撤回通知。无 `group_id`；`user_id` 是撤回消息的好友。
#[derive(Debug, Clone)]
pub struct FriendRecallEvent {
    pub user_id: i64,
    pub message_id: String,
    pub time: i64,
    pub raw_event: Value,
}

/// NapCat 适配器向未来业务层暴露的协议事件集合。
#[derive(Debug, Clone)]
pub enum NapCatEvent {
    GroupMessage(GroupMessageEvent),
    PrivateMessage(PrivateMessageEvent),
    GroupMemberIncrease(GroupMemberIncreaseEvent),
    GroupMemberDecrease(GroupMemberDecreaseEvent),
    Poke(PokeEvent),
    GroupRecall(GroupRecallEvent),
    FriendRecall(FriendRecallEvent),
}

/// 未来 QQBot 业务接入 NapCat 的唯一回调边界。
#[async_trait]
pub trait NapCatEventHandler: Send + Sync {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError>;
}

/// 传输层在 WebSocket 握手完成后通知宿主；宿主可在接收事件前持久化连接状态。
#[async_trait]
pub trait NapCatConnectionObserver: Send + Sync {
    async fn connected(&self) -> Result<(), NapCatError>;
}
