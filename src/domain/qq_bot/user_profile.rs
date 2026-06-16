use serde::{Deserialize, Serialize};

/// QQ 用户画像 - 由 ProfileBuilder 后台维护
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// QQ 号（主键，关联 qq_external_users）
    pub qq_user_id: i64,
    /// 兴趣标签，如 ["编程", "二次元", "音乐"]
    pub interest_tags: Option<Vec<String>>,
    /// 活跃时段分布，如 {"早":[7,9], "晚":[20,23]}
    pub active_hours: Option<serde_json::Value>,
    /// 说话风格分类
    pub speaking_style: Option<String>,
    /// 话题频率统计，如 {"游戏":15, "学习":3}
    pub topic_frequency: Option<serde_json::Value>,
    /// 消息总数
    pub total_messages: u32,
    /// 平均消息长度（字符数）
    pub avg_message_length: f64,
    /// 表情使用率（0.0 ~ 1.0）
    pub emoji_usage_rate: f64,
    /// 首次发现时间戳
    pub first_seen_at: Option<i64>,
    /// 上次画像更新时间戳
    pub last_summary_at: Option<i64>,
    /// LLM 生成的完整画像文本（原始）
    pub raw_profile: Option<String>,
}
