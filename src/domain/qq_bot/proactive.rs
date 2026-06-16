use serde::{Deserialize, Serialize};

/// 主动行为类型
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveAction {
    /// 欢迎新人（暂未实现，保留枚举位）
    GreetNewcomer,
    /// 冷场救急 — 贴图/提问无人回应
    FillSilence,
    /// 长时间沉默后冒泡
    Bubble,
    /// 被拍后回应（暂未实现）
    RespondToPoke,
    /// 自发分享话题
    ShareInterest,
    /// 关心群友情绪
    ShowConcern,
}

impl ProactiveAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::GreetNewcomer => "欢迎新人",
            Self::FillSilence => "冷场救急",
            Self::Bubble => "沉默冒泡",
            Self::RespondToPoke => "回应拍一拍",
            Self::ShareInterest => "分享话题",
            Self::ShowConcern => "关心情绪",
        }
    }
}

/// 主动行为意图 — 由 Layer1 规则过滤 + Layer2 LLM 判断后生成
#[derive(Debug, Clone)]
pub struct ProactiveIntent {
    /// 目标群号
    pub group_id: i64,
    /// 行为类型
    pub action_type: ProactiveAction,
    /// 判断理由（Layer2 LLM 输出或 Layer1 规则描述）
    pub reasoning: String,
}
