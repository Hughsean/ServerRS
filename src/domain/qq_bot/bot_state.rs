use chrono::Datelike;
use serde::{Deserialize, Serialize};

/// 主情绪枚举
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    Neutral,
    Sad,
    Angry,
    Surprised,
    Tired,
}

impl Mood {
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Happy => "😊",
            Self::Neutral => "😐",
            Self::Sad => "😿",
            Self::Angry => "😾",
            Self::Surprised => "😳",
            Self::Tired => "🥱",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Happy => "开心",
            Self::Neutral => "平静",
            Self::Sad => "难过",
            Self::Angry => "生气",
            Self::Surprised => "惊讶",
            Self::Tired => "疲惫",
        }
    }
}

impl Default for Mood {
    fn default() -> Self {
        Self::Neutral
    }
}

/// 情绪状态（内存中持久化）
#[derive(Debug, Clone)]
pub struct EmotionalState {
    pub mood: Mood,
    pub intensity: f32,
    pub trigger_event: Option<String>,
    pub decay_started_at: Option<i64>,
}

impl EmotionalState {
    pub fn new(mood: Mood, intensity: f32, trigger_event: Option<String>) -> Self {
        Self {
            mood,
            intensity: intensity.clamp(0.0, 1.0),
            trigger_event,
            decay_started_at: None,
        }
    }

    /// 应用时间衰减。每 60 秒衰减 intensity * 0.05，最低不低于 0.1。
    pub fn apply_decay(&mut self, now_ms: i64) {
        if let Some(started_at) = self.decay_started_at {
            let elapsed_ms = (now_ms - started_at).max(0);
            let decay_units = (elapsed_ms as f32 / 60_000.0).floor();
            self.intensity = (self.intensity - decay_units * 0.05).max(0.1);
        }
    }

    /// 触发新情绪，重置衰减计时器
    pub fn trigger(&mut self, mood: Mood, intensity: f32, event: Option<String>) {
        self.mood = mood;
        self.intensity = intensity.clamp(0.0, 1.0);
        self.trigger_event = event;
        self.decay_started_at = None;
    }

    /// 情绪描述（给 LLM 用）
    pub fn describe(&self) -> String {
        format!(
            "心情：{} {}（强度 {:.1}）\n原因：{}",
            self.mood.label(),
            self.mood.emoji(),
            self.intensity,
            self.trigger_event.as_deref().unwrap_or("无特殊原因"),
        )
    }
}

impl Default for EmotionalState {
    fn default() -> Self {
        Self {
            mood: Mood::Neutral,
            intensity: 0.5,
            trigger_event: None,
            decay_started_at: None,
        }
    }
}

/// 时间段
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeOfDay {
    EarlyMorning,
    Morning,
    Noon,
    Afternoon,
    Evening,
    Night,
    LateNight,
}

impl TimeOfDay {
    pub fn label(&self) -> &'static str {
        match self {
            Self::EarlyMorning => "清晨",
            Self::Morning => "上午",
            Self::Noon => "中午",
            Self::Afternoon => "下午",
            Self::Evening => "傍晚",
            Self::Night => "晚上",
            Self::LateNight => "深夜",
        }
    }

    pub fn from_hour(hour: u32) -> Self {
        match hour {
            5..=7 => Self::EarlyMorning,
            8..=11 => Self::Morning,
            12..=13 => Self::Noon,
            14..=17 => Self::Afternoon,
            18..=20 => Self::Evening,
            21..=23 => Self::Night,
            _ => Self::LateNight,
        }
    }
}

/// 时间上下文（每个请求动态构建）
#[derive(Debug, Clone)]
pub struct TemporalContext {
    pub time_of_day: TimeOfDay,
    pub weekday: u32,
    pub is_weekend: bool,
    pub date_str: String,
    pub minutes_since_last_activity: u64,
    pub hours_since_bot_spoke: Option<u64>,
    pub is_first_interaction_today: bool,
    pub message_count_today: u32,
    pub unix_timestamp: i64,
}

impl TemporalContext {
    pub fn from_now(
        last_message_minutes_ago: u64,
        hours_since_bot_spoke: Option<u64>,
        is_first_today: bool,
        message_count_today: u32,
    ) -> Self {
        let now = chrono::Local::now();
        let hour: u32 = now.format("%H").to_string().parse().unwrap_or(12);
        let weekday = now.weekday().num_days_from_monday() + 1;
        let is_weekend = matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);

        Self {
            time_of_day: TimeOfDay::from_hour(hour),
            weekday,
            is_weekend,
            date_str: now.format("%Y-%m-%d %A").to_string(),
            minutes_since_last_activity: last_message_minutes_ago,
            hours_since_bot_spoke,
            is_first_interaction_today: is_first_today,
            message_count_today,
            unix_timestamp: now.timestamp(),
        }
    }

    /// 时间描述段落（给 LLM 用）
    pub fn describe(&self) -> String {
        let now = chrono::Local::now();
        let mut parts = vec![
            format!(
                "当前时间：{} {}（{}）",
                self.date_str,
                self.time_of_day.label(),
                now.format("%H:%M").to_string()
            ),
            format!("群里上一条消息是 {} 分钟前", self.minutes_since_last_activity),
        ];

        if self.is_weekend {
            parts.push("今天是周末 ~".into());
        }

        if let Some(hours) = self.hours_since_bot_spoke {
            if hours > 0 {
                parts.push(format!("你上次在群里说话是 {} 小时前", hours));
            }
        }

        parts.push(format!(
            "你今天已经在群里说过 {} 次话了",
            self.message_count_today
        ));

        if self.is_first_interaction_today {
            parts.push(
                "这是你今天第一次在群里说话。如果时间还早可以问候早安。".into(),
            );
        }

        parts.join("\n")
    }
}

/// 机器人综合状态快照（每个回复周期前构建）
#[derive(Debug, Clone)]
pub struct BotState {
    pub emotional: EmotionalState,
    pub temporal: TemporalContext,
}

impl BotState {
    pub fn new(emotional: EmotionalState, temporal: TemporalContext) -> Self {
        Self {
            emotional,
            temporal,
        }
    }
}
