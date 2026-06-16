use serde::{Deserialize, Serialize};

/// 亲密度等级
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RapportLevel {
    /// 友善的
    Friendly,
    /// 中立的
    Neutral,
    /// 尴尬的
    Awkward,
    /// 爱开玩笑的
    Playful,
    /// 尊敬的
    Respectful,
}

impl Default for RapportLevel {
    fn default() -> Self {
        Self::Neutral
    }
}

impl RapportLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Friendly => "友善",
            Self::Neutral => "普通",
            Self::Awkward => "尴尬",
            Self::Playful => "玩闹",
            Self::Respectful => "尊敬",
        }
    }

    /// 根据熟悉度推断默认的亲密度等级
    pub fn from_familiarity(f: f32) -> Self {
        if f >= 0.8 {
            Self::Playful
        } else if f >= 0.5 {
            Self::Friendly
        } else if f >= 0.3 {
            Self::Neutral
        } else {
            Self::Respectful // 对不熟的人保持礼貌
        }
    }
}

/// 对某个群友的关系状态（持久化到数据库）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipState {
    /// 自增 ID（数据库生成）
    pub id: Option<u64>,
    /// 群号
    pub qq_group_id: i64,
    /// 群友 QQ 号
    pub qq_user_id: i64,
    /// 熟悉度 0.0(陌生人) ~ 1.0(死党)
    pub familiarity: f32,
    /// 累计互动次数
    pub interaction_count: u32,
    /// 上次互动时间戳
    pub last_interaction_at: Option<i64>,
    /// 亲密度等级
    pub rapport: RapportLevel,
    /// 偏好的称呼
    pub nickname_preference: Option<String>,
    /// 已知兴趣
    pub known_interests: Vec<String>,
    /// 应避免的话题
    pub known_avoid_topics: Vec<String>,
}

impl RelationshipState {
    /// 创建一个全新的关系记录
    pub fn new(qq_group_id: i64, qq_user_id: i64, now_ts: Option<i64>) -> Self {
        Self {
            id: None,
            qq_group_id,
            qq_user_id,
            familiarity: 0.1,
            interaction_count: 1,
            last_interaction_at: now_ts,
            rapport: RapportLevel::Neutral,
            nickname_preference: None,
            known_interests: Vec::new(),
            known_avoid_topics: Vec::new(),
        }
    }

    /// 递增互动次数并重新计算熟悉度
    pub fn increment_interaction(&mut self, now_ts: Option<i64>) {
        self.interaction_count = self.interaction_count.saturating_add(1);
        // familiarity = min(1.0, 0.1 + interaction_count * 0.015)
        self.familiarity = (0.1 + self.interaction_count as f32 * 0.015).min(1.0);
        if let Some(ts) = now_ts {
            self.last_interaction_at = Some(ts);
        }
        // 根据熟悉度更新亲密度等级
        self.rapport = RapportLevel::from_familiarity(self.familiarity);
    }

    /// 生成给 LLM 使用的关系描述文本
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();

        let rapport_label = self.rapport.label();
        let nickname = self.nickname_preference.as_deref().unwrap_or("");

        let header = if nickname.is_empty() {
            format!(
                "用户[{}]：熟悉度 {:.2}（{}）",
                self.qq_user_id, self.familiarity, rapport_label,
            )
        } else {
            format!(
                "用户[{}]（你叫他\"{}\"）：熟悉度 {:.2}（{}）",
                self.qq_user_id, nickname, self.familiarity, rapport_label,
            )
        };
        parts.push(header);

        if !self.known_interests.is_empty() {
            parts.push(format!("- 兴趣：{}", self.known_interests.join("、")));
        }

        if !self.known_avoid_topics.is_empty() {
            parts.push(format!("- 应避免：{}", self.known_avoid_topics.join("、")));
        }

        parts.join("\n")
    }
}
