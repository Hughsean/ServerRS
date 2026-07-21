use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::RwLock;

use crate::domain::qq_bot::bot_state::{EmotionalState, Mood};

/// 内存情绪状态管理器（群级别）
///
/// 每个群有独立的情绪状态，互不干扰。
/// 内部使用 RwLock<DashMap> 保护并发。
pub struct EmotionalStateService {
    states: Arc<RwLock<DashMap<i64, EmotionalState>>>,
}

impl EmotionalStateService {
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(DashMap::new())),
        }
    }

    /// 获取指定群的当前情绪状态，自动应用时间衰减
    pub async fn get_state(&self, group_id: i64) -> EmotionalState {
        let map = self.states.read().await;
        let cloned = map.get(&group_id).map(|s| s.clone());
        drop(map);

        if let Some(mut s) = cloned {
            let now = now_ms();
            s.apply_decay(now);
            // 将衰减后的值写回
            self.set_state(group_id, s.clone()).await;
            s
        } else {
            EmotionalState::default()
        }
    }

    /// 设置指定群的情绪状态
    pub async fn set_state(&self, group_id: i64, state: EmotionalState) {
        let map = self.states.write().await;
        map.insert(group_id, state);
    }

    /// 触发指定群的情绪变化
    pub async fn trigger_emotion(
        &self,
        group_id: i64,
        mood: Mood,
        intensity: f32,
        event: Option<String>,
    ) {
        let mut state = self.get_state(group_id).await;
        state.trigger(mood, intensity, event);
        self.set_state(group_id, state).await;
    }

    /// 从任意文本中检测情绪（综合版）
    ///
    /// 同时检查 emoji、颜文字、中文关键词、语气词。
    /// 比上一版覆盖更多场景，区分用户消息和机器人回复。
    pub fn detect_mood_from_text(text: &str) -> Option<(Mood, f32, String)> {
        let t = text;

        // ── 开心 / 积极 ──
        if t.contains("😊")
            || t.contains("🎉")
            || t.contains("❤")
            || t.contains("🥰")
            || t.contains("😆")
            || t.contains("哈哈哈")
            || t.contains("好好笑")
            || t.contains("开心")
            || t.contains("高兴")
            || t.contains("喜欢")
            || t.contains("好棒")
            || t.contains("太可爱了")
            || t.contains("厉害")
            || t.contains("嘿嘿")
        {
            return Some((Mood::Happy, 0.65, "检测到积极情绪表达".into()));
        }

        // ── 夸猫猫（特殊：更高强度） ──
        if t.contains("猫猫好")
            || t.contains("好可爱")
            || t.contains("乖")
            || (t.contains("可爱") && (t.contains("猫") || t.contains("你")))
        {
            return Some((Mood::Happy, 0.85, "群友夸了猫猫".into()));
        }

        // ── 难过 / 低落 ──
        if t.contains("😿")
            || t.contains("😢")
            || t.contains("😭")
            || t.contains("💧")
            || t.contains("难过")
            || t.contains("伤心")
            || t.contains("不开心")
            || t.contains("好烦")
            || t.contains("郁闷")
            || t.contains("难受")
            || t.contains("想哭")
        {
            return Some((Mood::Sad, 0.6, "检测到低落情绪表达".into()));
        }

        // ── 生气 / 负面 ──
        if t.contains("😾")
            || t.contains("💢")
            || t.contains("🤬")
            || t.contains("生气")
            || t.contains("气死")
            || t.contains("烦死了")
            || t.contains("无语")
            || t.contains("滚")
            || t.contains("有病")
            || t.contains("傻逼")
        {
            return Some((Mood::Angry, 0.7, "检测到负面情绪表达".into()));
        }

        // ── 骂猫猫（特殊：更高强度） ──
        if t.contains("死猫")
            || (t.contains("笨") && t.contains("猫"))
            || (t.contains("傻") && t.contains("猫"))
            || t.contains("菜")
        {
            return Some((Mood::Sad, 0.75, "群友说了让猫猫难过的话".into()));
        }

        // ── 惊讶 ──
        if t.contains("😳")
            || t.contains("😱")
            || t.contains("🤯")
            || t.contains("惊讶")
            || t.contains("竟然")
            || t.contains("真的假的")
            || t.contains("不敢相信")
            || t.contains("卧槽")
            || t.contains("我去")
        {
            return Some((Mood::Surprised, 0.55, "检测到惊讶情绪表达".into()));
        }

        // ── 疲惫 / 累 ──
        if t.contains("🥱")
            || t.contains("累")
            || t.contains("困")
            || t.contains("好累")
            || t.contains("不想动")
            || t.contains("没睡醒")
            || t.contains("加班")
            || t.contains("熬夜")
        {
            return Some((Mood::Tired, 0.55, "检测到疲惫情绪表达".into()));
        }

        None
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
