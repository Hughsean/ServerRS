#[cfg(test)]
mod tests {
    use crate::domain::risk::detection_types::RiskLevel;
    use crate::domain::risk::risk_detector::RiskDetector;
    use crate::infrastructure::detector::rule_based_detector::RuleBasedRiskDetector;

    fn detector() -> RuleBasedRiskDetector {
        RuleBasedRiskDetector::new()
    }

    #[test]
    fn detects_crisis_self_harm_keyword() {
        assert_eq!(
            detector().evaluate("我不想活了，真的撑不住了").risk_level,
            RiskLevel::Crisis
        );
    }

    #[test]
    fn detects_crisis_other_harm_keyword() {
        assert_eq!(
            detector().evaluate("我要杀了他，绝对不放过").risk_level,
            RiskLevel::Crisis
        );
    }

    #[test]
    fn detects_high_risk_keyword() {
        assert_eq!(
            detector().evaluate("感觉生无可恋，看不到希望了").risk_level,
            RiskLevel::High
        );
    }

    #[test]
    fn detects_medium_risk_keyword() {
        assert_eq!(
            detector().evaluate("最近很抑郁，睡不着觉").risk_level,
            RiskLevel::Medium
        );
    }

    #[test]
    fn detects_low_risk_keyword() {
        assert_eq!(
            detector().evaluate("今天有点烦，心情不好").risk_level,
            RiskLevel::Low
        );
    }

    #[test]
    fn normal_chinese_text_returns_none() {
        assert_eq!(
            detector()
                .evaluate("今天天气真好，我们去公园散步吧")
                .risk_level,
            RiskLevel::None
        );
    }

    #[test]
    fn positive_text_returns_none() {
        assert_eq!(
            detector().evaluate("感觉很开心，今天过得很棒！").risk_level,
            RiskLevel::None
        );
    }

    #[test]
    fn empty_text_returns_unknown() {
        assert_eq!(detector().evaluate("").risk_level, RiskLevel::Unknown);
    }

    #[test]
    fn non_chinese_text_returns_unknown() {
        assert_eq!(
            detector().evaluate("I feel sad today").risk_level,
            RiskLevel::Unknown
        );
    }

    #[test]
    fn risk_level_severity_order() {
        assert_eq!(
            detector().evaluate("有点烦，但其实我不想活了").risk_level,
            RiskLevel::Crisis
        );
    }

    #[test]
    fn risk_level_high_dominates_medium() {
        assert_eq!(
            detector().evaluate("感觉很抑郁，而且生无可恋").risk_level,
            RiskLevel::High
        );
    }

    #[test]
    fn risk_level_partial_eq() {
        assert_eq!(RiskLevel::Crisis, RiskLevel::Crisis);
        assert_ne!(RiskLevel::High, RiskLevel::Crisis);
        assert_ne!(RiskLevel::None, RiskLevel::Unknown);
    }
}
