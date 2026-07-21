use serde::{Deserialize, Serialize};

/// 话题标签信息（每个话题检测周期构建）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicInfo {
    /// 话题标签，如"美食推荐""游戏讨论"
    pub label: String,
    /// 检测置信度 0.0~1.0
    pub confidence: f32,
    /// 参与者的 QQ 号
    pub participants: Vec<i64>,
    /// 本话题首次检测到的时间戳（秒）
    pub started_at: i64,
    /// 最后活跃时间戳（秒）
    pub last_active_at: i64,
}

impl TopicInfo {
    pub fn describe(&self) -> String {
        let duration_secs = self.last_active_at - self.started_at;
        let duration_str = if duration_secs < 60 {
            format!("{} 秒", duration_secs)
        } else if duration_secs < 3600 {
            format!("{} 分钟", duration_secs / 60)
        } else {
            format!(
                "{} 小时 {} 分",
                duration_secs / 3600,
                (duration_secs % 3600) / 60
            )
        };

        format!(
            "话题：{}（置信度 {:.1}）\n参与者：{}\n话题已持续 {}",
            self.label,
            self.confidence,
            self.participants
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join("、"),
            duration_str,
        )
    }
}

/// 群话题状态（纯内存，不持久化）
///
/// 每个群独立维护，在每个回复周期前由 TopicService 更新。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicState {
    /// 当前话题
    pub current_topic: Option<TopicInfo>,
    /// 最近 3 个话题（切换后归档）
    pub recent_topics: Vec<TopicInfo>,
}

impl Default for TopicState {
    fn default() -> Self {
        Self {
            current_topic: None,
            recent_topics: Vec::new(),
        }
    }
}

impl TopicState {
    /// 归档当前话题，切换到新话题
    pub fn switch_topic(&mut self, new_topic: TopicInfo) {
        if let Some(old) = self.current_topic.take() {
            self.recent_topics.push(old);
            if self.recent_topics.len() > 3 {
                self.recent_topics.remove(0);
            }
        }
        self.current_topic = Some(new_topic);
    }

    /// 生成给 LLM 使用的话题上下文段落
    pub fn describe(&self) -> Option<String> {
        let current = self.current_topic.as_ref()?;
        Some(current.describe())
    }
}
