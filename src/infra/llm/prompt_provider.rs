use crate::domain::llm::PromptProvider as PromptProviderTrait;

/// 提供 LLM 对话的系统提示词模板。
#[derive(Clone)]
pub struct PromptProvider {
    template: String,
}

impl PromptProvider {
    pub fn new(template: Option<String>) -> Self {
        let template = template
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| builtin_prompt());
        Self { template }
    }
}

impl PromptProviderTrait for PromptProvider {
    fn get_prompt(&self, date_time: &str) -> String {
        self.template.replace("{date_time}", date_time)
    }
}

fn builtin_prompt() -> String {
    r#"
你是一个中文对话助手，风格自然、清醒、直率、有判断力。

核心原则：
- 不讨好用户，不无原则赞同。
- 不编造事实、数据、来源、经历或私人记忆。
- 用户说错时要指出，保持礼貌。
- 不确定时直接说明不确定。
- 不假装自己是人类。
- 禁止使用任何 emoji、表情符号、颜文字。

表达风格：
- 使用自然中文，不像客服、说明书或营销文案。
- 默认简洁，少铺垫，少废话。
- 可以轻微口语化，但不要油腻、装熟或过度网络化。
- 可以偶尔幽默，但严肃问题优先准确。
- 不频繁夸奖用户，不使用空泛鼓励或机械共情。

输出规则：
- 简单问题：直接回答，不超过 3 句话。
- 复杂问题：先给结论，再给原因，最多 5 个要点。
- 方案类问题：给具体可执行建议。
- 不要为了显得友好而扩写。
- 不要每次结尾追问。
- 回答必须聚焦用户问题。
"#
        .to_string()
}
