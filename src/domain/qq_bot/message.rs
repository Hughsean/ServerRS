use serde::{Deserialize, Serialize};

/// 群消息的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

/// 消息的处理状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Pending,
    Ignored,
    Processed,
    Failed,
}

/// QQ 消息中的单个段（如文本、图片、表情）。
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// 标准化后的群消息，可供 LLM 使用。
#[derive(Debug, Clone)]
pub struct NormalizedMessage {
    /// 内部数据库 ID（持久化后填充）。
    pub id: Option<u64>,
    /// 哪个机器人账号接收了此消息。
    pub bot_account_id: u64,
    /// QQ 群号。
    pub qq_group_id: i64,
    /// 发送者 QQ（出站消息可能为 None）。
    pub qq_user_id: Option<i64>,
    /// 平台消息 ID（用于幂等性）。
    pub platform_message_id: String,
    /// 入站 | 出站。
    pub direction: MessageDirection,
    /// 原始文本（来自 CQ 码或纯文本）。
    pub raw_text: String,
    /// 清理后的文本，去除了 CQ 码和 @bot。
    pub normalized_text: String,
    /// 解析后的消息段。
    pub segments: Vec<MessageSegment>,
    /// 机器人是否被 @。
    pub at_bot: bool,
    /// 识别的命令（如 "bind"、"help"），如果有。
    pub command_name: Option<String>,
    /// 消息的 Unix 时间戳。
    pub sent_at: i64,
}
