use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::info;

use crate::domain::qq_bot::message::NormalizedMessage;
use crate::domain::qq_bot::topic_state::{TopicInfo, TopicState};

/// 中文停用词（轻量级内置列表）
const STOP_WORDS: &[&str] = &[
    "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一",
    "一个", "上", "也", "很", "到", "说", "要", "去", "你", "会", "着",
    "没有", "看", "好", "自己", "这", "他", "她", "它", "们", "那", "些",
    "吧", "吗", "啊", "呢", "哦", "嗯", "哈", "啦", "呀", "嘛", "唉", "哇",
    "的", "得", "地", "个", "把", "被", "让", "给", "对", "从", "向", "在",
    "什么", "怎么", "为什么", "这个", "那个", "哪个", "这些", "那些",
    "可以", "能", "可能", "应该", "要", "会", "想", "觉得", "知道",
    "没有", "就是", "不是", "但是", "而且", "虽然", "因为", "所以", "如果",
    "还是", "或者", "然后", "最后", "现在", "刚才", "以前", "以后", "已经",
    "这样", "那样", "这么", "那么", "非常", "很", "太", "更", "比较",
    "真的", "其实", "确实", "当然", "真的吗", "没事", "好的", "嗯嗯",
];

/// 话题服务（纯内存，每个群独立维护话题状态）
///
/// 通过词频分析检测群聊话题，零 LLM 调用成本。
pub struct TopicService {
    /// 群级别话题状态：group_id → TopicState
    states: Arc<RwLock<DashMap<i64, TopicState>>>,
}

impl TopicService {
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(DashMap::new())),
        }
    }

    /// 分析指定群的最近消息，更新话题状态
    ///
    /// 在每次收到新消息并 trigger 之前调用。
    pub async fn analyze(
        &self,
        group_id: i64,
        recent_messages: &[NormalizedMessage],
        now_ts: i64,
    ) {
        // 提取最近消息中的有效中文内容（跳过机器人自己发的）
        let user_texts: Vec<&str> = recent_messages
            .iter()
            .rev()
            .take(10)
            .filter(|m| {
                m.direction == crate::domain::qq_bot::message::MessageDirection::Inbound
                    && !m.normalized_text.is_empty()
            })
            .map(|m| m.normalized_text.as_str())
            .collect();

        if user_texts.is_empty() {
            return;
        }

        // 提取高频词作为话题标签
        let words = self.extract_keywords(&user_texts);

        if words.is_empty() {
            return;
        }

        let label = words.join("·");
        let participants = self.extract_participants(recent_messages);
        let confidence = (words.len() as f32).min(5.0) / 5.0; // 词越多置信度越高，最高 1.0

        let map = self.states.write().await;
        let mut state = map.entry(group_id).or_insert_with(TopicState::default);

        // 判断是否需要切换话题
        let should_switch = match &state.current_topic {
            Some(current) => {
                // 如果新标签与当前标签相似度高（共享关键词），则延续话题
                let overlap = self.word_overlap(&current.label, &label);
                if overlap > 0.3 {
                    // 延续当前话题，更新参与者和时间
                    false
                } else {
                    // 话题切换
                    true
                }
            }
            None => true,
        };

        if should_switch {
            let new_topic = TopicInfo {
                label,
                confidence,
                participants,
                started_at: now_ts,
                last_active_at: now_ts,
            };
            state.switch_topic(new_topic);
            info!(
                group_id,
                topic = %state.current_topic.as_ref().map(|t| t.label.as_str()).unwrap_or("none"),
                "topic switched"
            );
        } else if let Some(ref mut current) = state.current_topic {
            // 更新参与者合并
            for p in &participants {
                if !current.participants.contains(p) {
                    current.participants.push(*p);
                }
            }
            current.last_active_at = now_ts;
            // 微调置信度
            current.confidence = current.confidence.max(confidence);
        }
    }

    /// 获取当前话题上下文描述（用于注入 LLM prompt）
    pub async fn get_topic_context(&self, group_id: i64) -> Option<String> {
        let map = self.states.read().await;
        map.get(&group_id).and_then(|s| s.describe())
    }

    /// 获取当前话题标签（用于 trigger evaluator）
    pub async fn get_current_topic_label(&self, group_id: i64) -> Option<String> {
        let map = self.states.read().await;
        let entry = map.get(&group_id)?;
        entry.current_topic.as_ref().map(|t| t.label.clone())
    }

    // ── 内部方法 ──

    /// 从用户文本列表中提取关键词（去除停用词后按频率排序）
    fn extract_keywords(&self, texts: &[&str]) -> Vec<String> {
        use std::collections::HashMap;

        let mut freq: HashMap<String, u32> = HashMap::new();

        for text in texts {
            // 简单分词：按非中文字符切割，保留中文词
            let chars: Vec<char> = text.chars().collect();
            let mut current_word = String::new();
            for &c in &chars {
                if c >= '\u{4e00}' && c <= '\u{9fff}' {
                    current_word.push(c);
                } else {
                    if current_word.len() >= 2 {
                        *freq.entry(current_word.clone()).or_insert(0) += 1;
                    }
                    current_word.clear();
                }
            }
            if current_word.len() >= 2 {
                *freq.entry(current_word).or_insert(0) += 1;
            }
        }

        // 按频率排序，去除停用词，取前 3 个
        let mut words: Vec<(String, u32)> = freq
            .into_iter()
            .filter(|(w, _)| !STOP_WORDS.contains(&w.as_str()))
            .collect();

        words.sort_by(|a, b| b.1.cmp(&a.1));
        words.truncate(3);

        words.into_iter().map(|(w, _)| w).collect()
    }

    /// 提取最近消息中的参与者 QQ 号
    fn extract_participants(&self, recent_messages: &[NormalizedMessage]) -> Vec<i64> {
        let mut users: Vec<i64> = recent_messages
            .iter()
            .rev()
            .take(10)
            .filter(|m| m.direction == crate::domain::qq_bot::message::MessageDirection::Inbound)
            .filter_map(|m| m.qq_user_id)
            .collect();
        users.sort_unstable();
        users.dedup();
        users
    }

    /// 计算两个话题标签的词重叠率
    fn word_overlap(&self, label1: &str, label2: &str) -> f32 {
        let words1: Vec<&str> = label1.split('·').collect();
        let words2: Vec<&str> = label2.split('·').collect();

        if words1.is_empty() || words2.is_empty() {
            return 0.0;
        }

        let overlap = words1.iter().filter(|w| words2.contains(w)).count();
        overlap as f32 / words1.len().max(words2.len()) as f32
    }
}
