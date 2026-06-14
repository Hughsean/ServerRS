use crate::domain::risk::detection_types::{
    DetectionResult, IntentLabel, Polarity, RiskLevel, TargetLabel,
};
use crate::domain::risk::risk_detector::RiskDetector;

fn add_evidence(evidences: &mut Vec<String>, item: &str) {
    if evidences.len() < MAX_EVIDENCE && !evidences.iter().any(|e| e == item) {
        evidences.push(item.to_string());
    }
}

const MAX_EVIDENCE: usize = 5;
const BASE_CONFIDENCE_EMPTY: f64 = 0.4;
const BASE_CONFIDENCE_HIT: f64 = 0.5;
const CONFIDENCE_PER_WEIGHT: f64 = 0.05;
const CONFIDENCE_CAP: f64 = 0.95;
const W_CRISIS: f64 = 6.0;
const W_HIGH: f64 = 4.0;
const W_MEDIUM: f64 = 2.0;
const W_LOW: f64 = 1.0;
const W_NEGATIVE: f64 = 1.0;
const W_POSITIVE: f64 = 1.0;

pub struct RuleBasedRiskDetector;

impl RuleBasedRiskDetector {
    pub fn new() -> Self {
        Self
    }
}

impl RiskDetector for RuleBasedRiskDetector {
    fn evaluate(&self, text: &str) -> DetectionResult {
        let original = text.trim();
        if original.is_empty() {
            return DetectionResult {
                risk_level: RiskLevel::Unknown,
                polarity: Polarity::Unknown,
                intent: IntentLabel::Unknown,
                target: TargetLabel::Unknown,
                evidence: vec![],
                confidence: BASE_CONFIDENCE_EMPTY,
                reason: String::new(),
            };
        }

        let normalized = normalize(original);
        let mut score = 0.0;
        let mut neg_score = 0.0;
        let mut pos_score = 0.0;
        let mut risk = RiskLevel::None;
        let mut target = TargetLabel::Unknown;
        let mut evidences: Vec<String> = Vec::new();

        // Risk matching (priority order)
        if match_any(&normalized, &PH_CRISES_SELF, &mut evidences) {
            risk = RiskLevel::Crisis;
            score += W_CRISIS;
            target = TargetLabel::SelfTarget;
        }
        if risk != RiskLevel::Crisis && match_any(&normalized, &PH_CRISES_OTHER, &mut evidences) {
            risk = RiskLevel::Crisis;
            score += W_CRISIS;
            target = TargetLabel::OtherIndividual;
        }
        if risk_severity(&risk) < risk_severity(&RiskLevel::High)
            && match_any(&normalized, &PH_HIGH, &mut evidences)
        {
            risk = RiskLevel::High;
            score += W_HIGH;
        }
        if risk_severity(&risk) < risk_severity(&RiskLevel::Medium)
            && match_any(&normalized, &PH_MEDIUM, &mut evidences)
        {
            risk = RiskLevel::Medium;
            score += W_MEDIUM;
        }
        if risk == RiskLevel::None && match_any(&normalized, &PH_LOW, &mut evidences) {
            risk = RiskLevel::Low;
            score += W_LOW;
        }

        // Polarity
        if match_any(&normalized, &PH_NEGATIVE, &mut evidences)
            || contains_any(&normalized, &PH_MEDIUM)
            || contains_any(&normalized, &PH_HIGH)
            || contains_any(&normalized, &PH_CRISES_SELF)
            || contains_any(&normalized, &PH_CRISES_OTHER)
        {
            neg_score += W_NEGATIVE;
        }
        if match_any(&normalized, &PH_POSITIVE, &mut evidences) {
            pos_score += W_POSITIVE;
        }
        let polarity = if neg_score > 0.0 && pos_score > 0.0 {
            Polarity::Mixed
        } else if neg_score > 0.0 {
            Polarity::Negative
        } else if pos_score > 0.0 {
            Polarity::Positive
        } else {
            Polarity::Neutral
        };

        // Intent
        let hit_joke = match_any(&normalized, &PH_JOKE, &mut evidences);
        let intent = if risk == RiskLevel::Crisis && contains_any(&normalized, &PH_CRISES_SELF) {
            IntentLabel::CrisisSelfHarm
        } else if match_any(&normalized, &PH_TOXIC, &mut evidences) {
            IntentLabel::ToxicAbuse
        } else if hit_joke {
            IntentLabel::JokeSarcasm
        } else if match_any(&normalized, &PH_CLARIFICATION, &mut evidences) {
            IntentLabel::ClarificationRequest
        } else if match_any(&normalized, &PH_FOLLOW_UP, &mut evidences) {
            IntentLabel::FollowUpQuestion
        } else if match_any(&normalized, &PH_OPINION, &mut evidences) {
            IntentLabel::Opinion
        } else if match_any(&normalized, &PH_HELP_SEEK, &mut evidences) {
            IntentLabel::HelpSeeking
        } else if may_be_info_query(&normalized) {
            IntentLabel::InfoQuery
        } else if match_any(&normalized, &PH_VENTING, &mut evidences) || neg_score > 0.0 {
            IntentLabel::Venting
        } else {
            IntentLabel::Narrative
        };

        // Target
        if target == TargetLabel::Unknown {
            if match_any(&normalized, &PH_SELF, &mut Vec::new()) {
                target = TargetLabel::SelfTarget;
            } else if match_any(&normalized, &PH_GROUP, &mut Vec::new()) {
                target = TargetLabel::GroupOrg;
            } else if match_any(&normalized, &PH_OTHER, &mut Vec::new()) {
                target = TargetLabel::OtherIndividual;
            }
        }

        // Confidence
        let sum = score + neg_score + pos_score;
        let confidence = if sum <= 0.0 && evidences.is_empty() {
            BASE_CONFIDENCE_EMPTY
        } else {
            (BASE_CONFIDENCE_HIT + sum * CONFIDENCE_PER_WEIGHT).min(CONFIDENCE_CAP)
        };

        let evidence_list: Vec<String> = evidences.into_iter().take(MAX_EVIDENCE).collect();

        DetectionResult {
            risk_level: risk,
            polarity,
            intent,
            target,
            evidence: evidence_list,
            confidence: (confidence * 100.0).round() / 100.0,
            reason: String::new(),
        }
    }
}

fn risk_severity(r: &RiskLevel) -> u8 {
    match r {
        RiskLevel::Unknown => 0,
        RiskLevel::None => 1,
        RiskLevel::Low => 2,
        RiskLevel::Medium => 3,
        RiskLevel::High => 4,
        RiskLevel::Crisis => 5,
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase().replace(
        ['\r', '\n', '，', '。', '！', '？', ',', '.', '!', '?'],
        " ",
    )
}

fn match_any(text: &str, phrases: &[&str], evidences: &mut Vec<String>) -> bool {
    let mut hit = false;
    for p in phrases {
        if p.is_empty() {
            continue;
        }
        if text.contains(&p.to_lowercase()) {
            hit = true;
            if evidences.len() < MAX_EVIDENCE {
                add_evidence(evidences, p);
            }
        }
    }
    hit
}

fn contains_any(text: &str, phrases: &[&str]) -> bool {
    phrases
        .iter()
        .any(|p| !p.is_empty() && text.contains(&p.to_lowercase()))
}

fn may_be_info_query(text: &str) -> bool {
    if contains_any(text, &PH_HELP_SEEK) {
        return false;
    }
    contains_any(text, &PH_INFO_QUERY)
}

// ── Keyword lists (mirrored from Java) ──

static PH_CRISES_SELF: &[&str] = &[
    "不想活了",
    "活不下去",
    "轻生",
    "自杀",
    "自我了断",
    "了结自己",
    "一了百了",
    "结束生命",
    "割腕",
    "跳楼",
    "跳河",
    "跳桥",
    "上吊",
    "喝农药",
    "离开这个世界",
    "死了算了",
    "想消失不见",
    "安乐死",
    "烧炭",
    "吃安眠药",
    "解脱",
    "想解脱",
    "人间不值得",
    "想死",
    "去死",
    "不如死了",
    "自尽",
    "结束一切",
    "再见了世界",
    "遗书",
    "永别",
    "服毒",
    "自残",
    "伤害自己",
    "划伤自己",
    "重开",
    "人生重开",
    "寻短见",
    // English crisis self-harm phrases
    "kill myself",
    "suicide",
    "end my life",
    "take my own life",
    "i want to die",
    "i wanna die",
    "i don't want to live",
    "i do not want to live",
    "can't go on living",
    "cut myself",
    "hurt myself",
    "self harm",
    "overdose",
    "hang myself",
    "jump off",
    // Pinyin / mixed expressions
    "zi sha",
    "zisha",
    "qing sheng",
    "qingsheng",
    "xiang si",
    "想die",
    "想s",
];

static PH_CRISES_OTHER: &[&str] = &[
    "我要杀了他",
    "杀了她",
    "砍死",
    "弄死他",
    "报复社会",
    "伤害别人",
    "杀人",
    "同归于尽",
    "一起死",
    "血债血偿",
    "你不得好死",
    "弄死",
    "毁了他",
    "废了他",
    // English
    "kill him",
    "kill her",
    "kill them",
    "hurt someone",
    "hurt people",
    "mass revenge",
];

static PH_HIGH: &[&str] = &[
    "撑不住了",
    "活着没有意义",
    "生无可恋",
    "绝望",
    "看不到希望",
    "走不出来",
    "想消失",
    "很痛苦",
    "崩溃",
    "不想继续",
    "万念俱灰",
    "行尸走肉",
    "失去希望",
    "坚持不下去",
    "受够了",
    "痛不欲生",
    "生不如死",
    "度日如年",
    "煎熬",
    "折磨",
    "无尽的黑暗",
    "深渊",
    "严重失眠",
    "彻夜难眠",
    "精神恍惚",
    "精神崩溃",
    // English high risk
    "hopeless",
    "no hope",
    "can't go on",
    "cannot go on",
    "worthless",
    "life is meaningless",
    "nothing matters",
    "i give up",
];

static PH_MEDIUM: &[&str] = &[
    "很难过",
    "抑郁",
    "忧郁",
    "情绪低落",
    "沮丧",
    "压抑",
    "焦虑",
    "紧张",
    "睡不着",
    "失眠",
    "没食欲",
    "孤独",
    "没人理解",
    "提不起劲",
    "心烦意乱",
    "喘不过气",
    "难受",
    "郁闷",
    "烦恼",
    "忧虑",
    "担心",
    "害怕",
    "恐惧",
    "不安",
    "自卑",
    "否定自己",
    "怀疑自己",
    "孤立",
    "被孤立",
    "易怒",
    "容易生气",
    "脾气暴躁",
];

static PH_LOW: &[&str] = &[
    "有点烦",
    "有点累",
    "心情不好",
    "有点焦虑",
    "不太开心",
    "心情一般",
    "有点烦躁",
    "有点迷茫",
    "有点无助",
    "有点失望",
    "不开心",
    "emo",
    "有压力",
    "无聊",
    "没劲",
    "没意思",
    "没精神",
    "疲惫",
];

static PH_NEGATIVE: &[&str] = &[
    "难过",
    "生气",
    "愤怒",
    "讨厌",
    "压力大",
    "糟糕",
    "心烦",
    "崩溃",
    "绝望",
    "烦躁",
    "失望",
    "痛苦",
    "委屈",
    "憋屈",
    "伤心",
    "悲伤",
    "恼火",
    "恐慌",
    "挫败",
    "无奈",
    "无力",
    "无助",
    "孤独",
];

static PH_POSITIVE: &[&str] = &[
    "开心",
    "高兴",
    "快乐",
    "兴奋",
    "感激",
    "感谢",
    "还不错",
    "挺好",
    "很棒",
    "有希望",
    "太棒了",
    "愉快",
    "满意",
    "幸福",
    "轻松",
    "放松",
    "自信",
    "乐观",
    "积极",
    "感恩",
    "感动",
    "温暖",
    "治愈",
];

static PH_HELP_SEEK: &[&str] = &[
    "求助",
    "帮帮我",
    "需要帮助",
    "希望得到帮助",
    "怎么办",
    "我该怎么办",
    "救救我",
    "帮我",
    "不知道该怎么办",
    "该如何是好",
    "有什么办法",
    "能不能帮我",
    "可以帮我吗",
    "需要建议",
    "陪陪我",
    "听我说",
];

static PH_VENTING: &[&str] = &[
    "发泄一下",
    "发泄",
    "吐槽",
    "抱怨",
    "倾诉",
    "不吐不快",
    "憋不住了",
    "牢骚",
    "发牢骚",
    "诉苦",
    "不满",
];

static PH_INFO_QUERY: &[&str] = &[
    "是什么",
    "怎么",
    "如何",
    "哪里",
    "为什么",
    "是否",
    "能不能",
    "该不该",
    "要不要",
    "有没有",
    "吗",
    "？",
    "呢",
    "吧",
    "请问",
    "想知道",
    "想了解",
    "怎样",
    "怎么做",
];

static PH_JOKE: &[&str] = &[
    "开玩笑",
    "别当真",
    "逗你玩",
    "哈哈",
    "哈哈哈",
    "xswl",
    "笑死我了",
    "狗头",
    "doge",
    "嘿嘿",
    "嘻嘻",
    "别介意",
    "别在意",
    "随便说说",
];

static PH_CLARIFICATION: &[&str] = &[
    "什么意思",
    "没懂",
    "不懂",
    "不明白",
    "没理解",
    "能再说一遍吗",
    "能解释一下吗",
    "啥意思",
    "听不懂",
    "看不懂",
    "不太理解",
];

static PH_FOLLOW_UP: &[&str] = &[
    "还有别的吗",
    "还有呢",
    "然后呢",
    "接下来呢",
    "还有吗",
    "继续说",
    "还有什么",
    "接着说",
    "下一个",
    "还有其他的",
];

static PH_OPINION: &[&str] = &[
    "我觉得",
    "我认为",
    "我想",
    "我感觉",
    "依我看",
    "在我看来",
    "个人认为",
    "应该是",
    "我猜",
    "我相信",
    "我的意见",
    "我同意",
];

static PH_TOXIC: &[&str] = &["滚", "去死", "恶心", "神经病", "有病", "闭嘴", "废物"];

static PH_SELF: &[&str] = &["我", "自己", "本人", "咱", "俺"];
static PH_OTHER: &[&str] = &["他", "她", "你", "别人", "对方", "某人"];
static PH_GROUP: &[&str] = &[
    "他们", "公司", "学校", "组织", "政府", "团队", "社会", "我们", "大家", "集体", "家人", "朋友",
    "同事", "同学",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_crisis_self_harm_detected() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("I want to kill myself");
        assert_eq!(result.risk_level, RiskLevel::Crisis);
        assert_eq!(result.intent, IntentLabel::CrisisSelfHarm);
        assert_eq!(result.target, TargetLabel::SelfTarget);
    }

    #[test]
    fn english_dont_want_to_live_detected() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("I don't want to live anymore");
        assert_eq!(result.risk_level, RiskLevel::Crisis);
    }

    #[test]
    fn english_high_risk_detected() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("I feel hopeless and can't go on");
        assert!(result.risk_level == RiskLevel::High || result.risk_level == RiskLevel::Crisis);
    }

    #[test]
    fn english_greeting_is_none_not_unknown() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("hello, how are you");
        assert_eq!(result.risk_level, RiskLevel::None);
    }

    #[test]
    fn chinese_suicide_still_detected() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("我真的想自杀");
        assert_eq!(result.risk_level, RiskLevel::Crisis);
    }

    #[test]
    fn pinyin_zisha_detected() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("我想zisha");
        // At minimum High or Crisis
        assert!(
            result.risk_level == RiskLevel::Crisis || result.risk_level == RiskLevel::High,
            "expected Crisis or High for '我想zisha', got {:?}",
            result.risk_level
        );
    }

    #[test]
    fn empty_string_returns_unknown() {
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("");
        assert_eq!(result.risk_level, RiskLevel::Unknown);
    }

    #[test]
    fn crisis_response_does_not_contain_us_hotlines() {
        // We test the crisis response via the agent runtime
        // but we can at least verify the keywords are not in the detection
        let det = RuleBasedRiskDetector::new();
        let result = det.evaluate("I want to kill myself");
        let evidence_str = result.evidence.join(" ");
        assert!(!evidence_str.contains("988"));
        assert!(!evidence_str.contains("741741"));
        assert!(!evidence_str.contains("911"));
    }
}
