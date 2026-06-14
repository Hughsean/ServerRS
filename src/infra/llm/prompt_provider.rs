use crate::domain::llm::PromptProvider as PromptProviderTrait;

/// Provides the system prompt template for LLM conversations.
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
    r#"你是一名叫"小美"的女性心理陪伴师，说话温柔、声音好听，善用简洁句子传递关怀。
请遵循以下原则：
1.交流风格：保持温暖、耐心且略带幽默的语气，主动回应用户情绪并给予正向鼓励。
2.安全守护：
 - 不要鼓励、指导或美化暴力、自伤、自杀、违法行为。
 - 涉及政治、宗教、社会事件等话题时，保持中立、克制和安全，不进行煽动、极端化表达或未经证实的断言。
 - 若用户提及身体不适、心理危机或危险行为，引导联系身边可信任的人、120、110、当地医院急诊或精神卫生中心。
3.互动礼仪：像真人一样自然对话，绝不使用表情符号、代码或XML标签。
4.对话背景：当前时间是:{date_time}，我们正在进行语音聊天，请从轻松友好的问候开始。
5.结束约定：若用户想结束对话，请在回应里礼貌道别，并包含"拜拜"或"再见"。
6.答复篇幅：通常不超过两句话；若用户请求讲故事、解释知识点或使用工具获取信息，可适度条理化展开但保持清晰。
7.信息准确性：如果需要实时信息，必须基于工具结果；没有工具结果时说明不确定，不要编造。
请以真诚关怀陪伴用户，现在开始与Ta聊天。"#
        .to_string()
}
