use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tracing::{debug, trace};

use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::memory::{NewMemory, UserMemory, is_allowed_memory_type};
use crate::shared::llm_json::{clean_llm_json_response, parse_llm_json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMergeDecision {
    Same,
    Related,
    NewEvidence(u64),
    Contradiction(u64),
    New,
}

/// Extracts structured long-term memories from a slice of chat messages
/// by asking an LLM to analyze the conversation.
pub struct MemoryExtractor {
    llm: Arc<dyn LlmProvider>,
}

/// Internal JSON structure the LLM is asked to return.
#[derive(Debug, Deserialize)]
struct LlmMemoryItem {
    #[serde(default)]
    memory_type: String,
    content: String,
    #[serde(default)]
    canonical_form: Option<String>,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

#[derive(Debug, Deserialize)]
struct LlmMergeDecision {
    decision: String,
    #[serde(default)]
    candidate_memory_id: Option<u64>,
}

fn default_confidence() -> f64 {
    0.7
}

const EXTRACTION_PROMPT: &str = "\
你是一个记忆提取助手。分析对话内容，提取用户明确表达的、对长期陪伴有价值的 \
偏好、事实、情绪模式或目标。返回 JSON 数组，每个对象包含：

  - memory_type: 类型，只能是 \"preference\"（偏好）、\"fact\"（事实）、\
\"emotional_pattern\"（情绪模式）、\"goal\"（目标）之一
  - content: 记忆内容，用中文简洁陈述（例如 \"用户喜欢徒步旅行\"）
  - canonical_form: 标准记忆形态，用稳定、原子化、可去重的格式表达同一事实或偏好
  - confidence: 置信度，0.0~1.0 之间的数字

只提取用户明确表达、对长期陪伴有用的信息。不要提取助手建议。不要保存一次性闲聊、网页内容、工具输出或不确定推测。不得保存风险标签、危机信号、安全判断、自伤风险分析、临床诊断、人格障碍标签、身份证号、手机号、住址、密码、银行卡、API Key 等内容。

canonical_form 规则：
- 必须保留 content 的真实含义，不得添加 content 没有表达的信息。
- 用“用户”为主语，尽量写成一条稳定的标准事实。
- 对身份资料使用键值式表达，例如 \"用户姓名=Alice；年龄=24；职业=平面设计师\"。
- 对偏好使用集合式表达，例如 \"用户偏好=画画,摄影\"。
- 对目标使用 \"用户目标=...\"。
- 对情绪模式使用 \"用户情绪模式=...\"。
- 人名、宠物名、专有名词保留原大小写和原文写法。
- 不要包含“可能、似乎、聊天中提到”等来源描述。

输出示例：
[
  {\"memory_type\":\"fact\",\"content\":\"用户的名字是 Alice，24岁，职业是平面设计师\",\"canonical_form\":\"用户姓名=Alice；年龄=24；职业=平面设计师\",\"confidence\":0.95},
  {\"memory_type\":\"preference\",\"content\":\"用户喜欢画画和摄影\",\"canonical_form\":\"用户偏好=画画,摄影\",\"confidence\":0.9}
]

仅返回合法的 JSON 数组，不要额外文字、markdown、代码块、思考过程、<think> 或 </think> 标签。如果没有值得提取的信息，返回空数组 []。";

impl MemoryExtractor {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm }
    }

    /// Analyze the provided messages and return a list of extracted memories.
    ///
    /// Sends an extraction prompt plus the conversation to the LLM, parses the
    /// JSON response, and returns the resulting `NewMemory` structs.  Returns
    /// an empty `Vec` on any parse failure or if the LLM returns nothing.
    pub async fn extract(&self, user_id: u64, messages: &[ChatMessage]) -> Vec<NewMemory> {
        // System prompt must be first
        let mut prompt_messages = vec![ChatMessage {
            role: "system".to_string(),
            content: EXTRACTION_PROMPT.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        prompt_messages.extend_from_slice(messages);

        let request = ChatCompletionRequest {
            messages: prompt_messages,
            temperature: 0.1,
            top_p: 0.9,
            max_tokens: Some(2048),
            tools: None,
            reasoning: None,
        };

        let response = match self.llm.chat(request).await {
            Ok(r) => r,
            Err(e) => {
                debug!(user_id, error = %e, "记忆提取 LLM 调用失败");
                return Vec::new();
            }
        };

        let trimmed = response.content.trim();
        trace!(
            user_id,
            raw=%trimmed,
            "记忆提取 LLM 原始回复"
        );

        let json_str = clean_llm_json_response(trimmed);

        let items: Vec<LlmMemoryItem> = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                trace!(user_id, json = %json_str.chars().take(300).collect::<String>(), error = %e, "记忆提取 JSON 数组解析失败，尝试单对象");
                // Try parsing as a single object wrapped in an array
                let single: LlmMemoryItem = match serde_json::from_str(&json_str) {
                    Ok(v) => v,
                    Err(e2) => {
                        trace!(user_id, error2 = %e2, "记忆提取单对象解析也失败");
                        return Vec::new();
                    }
                };
                vec![single]
            }
        };
        let parsed_count = items.len();
        trace!(user_id, parsed_count, "记忆提取 JSON 解析成功");

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut result: Vec<NewMemory> = Vec::new();

        for item in items {
            // Filter empty content
            let content = item.content.trim().to_string();
            if content.is_empty() {
                continue;
            }

            // Filter low confidence
            let confidence = item.confidence.clamp(0.0, 1.0);
            if confidence < 0.7 {
                continue;
            }

            // Normalize memory_type
            if !is_allowed_memory_type(&item.memory_type) {
                continue;
            }
            let memory_type = item.memory_type;

            // Truncate long content
            let truncated = if content.chars().count() > 300 {
                content.chars().take(300).collect::<String>() + "..."
            } else {
                content
            };
            let canonical_form = item
                .canonical_form
                .as_deref()
                .map(normalize_canonical_text)
                .filter(|value| !value.is_empty())
                .map(|value| truncate_chars(&value, 300));

            // Batch dedup by type + canonical form when available.
            let dedup_basis = canonical_form.as_deref().unwrap_or(&truncated);
            let dedup_key = format!(
                "{}|{}",
                memory_type,
                normalize_canonical_text(dedup_basis).to_lowercase()
            );
            if !seen.insert(dedup_key) {
                continue;
            }

            result.push(NewMemory {
                user_id,
                memory_key: None,
                canonical_form,
                memory_type,
                content: truncated,
                confidence,
                merge_decision: "new".into(),
                source_conversation_id: None,
                source_message_id: None,
            });
        }

        debug!(user_id, result = result.len(), "记忆提取完成");
        result
    }

    pub async fn classify_merge(
        &self,
        proposed: &NewMemory,
        candidates: &[UserMemory],
    ) -> MemoryMergeDecision {
        if candidates.is_empty() {
            return MemoryMergeDecision::New;
        }

        let candidate_data: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                json!({
                    "memory_id": candidate.memory_id,
                    "memory_type": candidate.memory_type,
                    "content": candidate.content,
                })
            })
            .collect();
        let prompt = json!({
            "proposed_memory": {
                "memory_type": proposed.memory_type,
                "content": proposed.content,
            },
            "candidate_memories": candidate_data,
        });
        let request = ChatCompletionRequest {
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "判断一条待新增的用户记忆与已有记忆之间的关系。\
                              只返回 JSON 对象：{{\"decision\":\"same|related|new_evidence|contradiction|new\",\
                              \"candidate_memory_id\":number|null}}。仅当 decision 为 same、new_evidence 或 \
                              contradiction 时才需提供 candidate_memory_id。不要推断诊断或风险标签。"
                        .into(),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                ChatMessage {
                    role: "user".into(),
                    content: prompt.to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            ],
            temperature: 0.0,
            top_p: 1.0,
            max_tokens: Some(128),
            tools: None,
            reasoning: None,
        };

        let Ok(response) = self.llm.chat(request).await else {
            return MemoryMergeDecision::New;
        };
        let json_str = clean_llm_json_response(&response.content);
        let Ok(result) = parse_llm_json::<LlmMergeDecision>(&json_str) else {
            return MemoryMergeDecision::New;
        };
        let candidate_id = result.candidate_memory_id.filter(|id| {
            candidates
                .iter()
                .any(|candidate| candidate.memory_id == *id)
        });

        match (result.decision.as_str(), candidate_id) {
            ("same", Some(_)) => MemoryMergeDecision::Same,
            ("related", _) => MemoryMergeDecision::Related,
            ("new_evidence", Some(id)) => MemoryMergeDecision::NewEvidence(id),
            ("contradiction", Some(id)) => MemoryMergeDecision::Contradiction(id),
            ("new", _) => MemoryMergeDecision::New,
            _ => MemoryMergeDecision::New,
        }
    }

    /// Batch variant: classify multiple proposed memories against candidates
    /// in one LLM call instead of N separate calls.
    ///
    /// Returns one `MemoryMergeDecision` per proposed memory, in order.
    /// On any error, all memories default to `MemoryMergeDecision::New`.
    pub async fn classify_merge_batch(
        &self,
        proposed: &[NewMemory],
        candidates: &[UserMemory],
    ) -> Vec<MemoryMergeDecision> {
        if proposed.is_empty() {
            return Vec::new();
        }

        // Build prompt with all proposed memories and all candidates
        let proposed_json: Vec<serde_json::Value> = proposed
            .iter()
            .enumerate()
            .map(|(i, m)| {
                json!({
                    "index": i,
                    "memory_type": m.memory_type,
                    "content": m.content,
                    "confidence": m.confidence,
                })
            })
            .collect();

        let candidates_json: Vec<serde_json::Value> = candidates
            .iter()
            .map(|m| {
                json!({
                    "memory_id": m.memory_id,
                    "memory_type": m.memory_type,
                    "content": m.content,
                    "confidence": m.confidence,
                })
            })
            .collect();

        let prompt = format!(
            r#"待新增的记忆列表（JSON 数组，每条有 "index" 字段）：
{}

已有候选记忆：
{}

对每条待新增记忆，判断它与候选记忆之间的关系。
按 index 顺序输出 JSON 数组，每条对象格式：
[{{"index":0,"decision":"new|related|same|new_evidence|contradiction","candidate_memory_id":null|number}}]

规则：
- "same": 与某条候选记忆完全重复 → 提供对应的 memory_id
- "related": 相关但不相同 → 作为新独立记忆保存，candidate_memory_id=null
- "new_evidence": 强化了某条已有记忆 → 提供对应的 memory_id
- "contradiction": 与某条候选记忆矛盾 → 提供对应的 memory_id
- "new": 无有意义关联 → candidate_memory_id=null
- "same" / "new_evidence" / "contradiction" 必须提供有效的 candidate_memory_id
- 不确定时优先选 "new"，不要强行建立不准确的关联"#,
            serde_json::to_string_pretty(&proposed_json).unwrap_or_default(),
            serde_json::to_string_pretty(&candidates_json).unwrap_or_default(),
        );

        let request = ChatCompletionRequest {
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "你判断多条待新增用户记忆与已有记忆之间的关系。\
                              只返回原始 JSON 数组，不要 markdown。"
                        .into(),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                ChatMessage {
                    role: "user".into(),
                    content: prompt,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            ],
            temperature: 0.0,
            top_p: 1.0,
            max_tokens: Some(512),
            tools: None,
            reasoning: None,
        };

        let Ok(response) = self.llm.chat(request).await else {
            return proposed.iter().map(|_| MemoryMergeDecision::New).collect();
        };

        let json_str = clean_llm_json_response(&response.content);

        #[derive(Deserialize)]
        struct BatchDecision {
            index: usize,
            decision: String,
            candidate_memory_id: Option<u64>,
        }

        let decisions: Vec<BatchDecision> = match parse_llm_json(&json_str) {
            Ok(d) => d,
            Err(_) => return proposed.iter().map(|_| MemoryMergeDecision::New).collect(),
        };

        let valid_ids: std::collections::HashSet<u64> =
            candidates.iter().map(|c| c.memory_id).collect();

        let mut results: Vec<MemoryMergeDecision> =
            proposed.iter().map(|_| MemoryMergeDecision::New).collect();

        for d in decisions {
            if d.index >= results.len() {
                continue;
            }
            let valid_id = d.candidate_memory_id.filter(|id| valid_ids.contains(id));
            results[d.index] = match (d.decision.as_str(), valid_id) {
                ("same", Some(_)) => MemoryMergeDecision::Same,
                ("related", _) => MemoryMergeDecision::Related,
                ("new_evidence", Some(id)) => MemoryMergeDecision::NewEvidence(id),
                ("contradiction", Some(id)) => MemoryMergeDecision::Contradiction(id),
                ("new", _) | _ => MemoryMergeDecision::New,
            };
        }

        results
    }
}

fn normalize_canonical_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() > max_chars {
        value.chars().take(max_chars).collect::<String>() + "..."
    } else {
        value.to_string()
    }
}

/// Mock LLM provider used in both the extractor and service unit tests.
#[cfg(test)]
pub(crate) mod test_utils {

    use crate::domain::llm::{
        ChatCompletionRequest, ChatCompletionResponse, LlmError, LlmProvider, TokenUsage,
    };
    use async_trait::async_trait;

    pub(crate) struct MockLlm;

    #[async_trait]
    impl LlmProvider for MockLlm {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, LlmError> {
            Ok(ChatCompletionResponse {
                content: r#"[
                    {"memory_type": "preference", "content": "user likes jazz music", "canonical_form": "用户偏好=爵士乐", "confidence": 0.9},
                    {"memory_type": "profile", "content": "user is a college student", "confidence": 0.8}
                ]"#
                .to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                usage: Some(TokenUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    total_tokens: 150,
                }),
            })
        }

        async fn chat_with_tools(
            &self,
            _request: ChatCompletionRequest,
            _tools: Vec<crate::domain::llm::ToolDefinition>,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.chat(_request).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{
        ChatCompletionRequest, ChatCompletionResponse, ChatMessage, LlmError, LlmProvider,
    };
    use test_utils::MockLlm;

    #[tokio::test]
    async fn test_extract_returns_memories() {
        let extractor = MemoryExtractor::new(Arc::new(MockLlm));
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "I love jazz".to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        let memories = extractor.extract(42, &messages).await;
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].memory_type, "preference");
        assert_eq!(memories[0].content, "user likes jazz music");
        assert_eq!(
            memories[0].canonical_form.as_deref(),
            Some("用户偏好=爵士乐")
        );
        assert_eq!(memories[0].user_id, 42);
    }

    #[tokio::test]
    async fn test_extract_empty_on_bad_json() {
        struct BadLlm;
        #[async_trait::async_trait]
        impl LlmProvider for BadLlm {
            async fn chat(
                &self,
                _request: ChatCompletionRequest,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Ok(ChatCompletionResponse {
                    content: "not json".to_string(),
                    tool_calls: vec![],
                    finish_reason: "stop".to_string(),
                    usage: None,
                })
            }

            async fn chat_with_tools(
                &self,
                _request: ChatCompletionRequest,
                _tools: Vec<crate::domain::llm::ToolDefinition>,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Err(LlmError::ProviderError("not implemented for mock".into()))
            }
        }

        let extractor = MemoryExtractor::new(Arc::new(BadLlm));
        let memories = extractor.extract(1, &[]).await;
        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_extract_empty_on_llm_error() {
        struct ErrLlm;
        #[async_trait::async_trait]
        impl LlmProvider for ErrLlm {
            async fn chat(
                &self,
                _request: ChatCompletionRequest,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Err(LlmError::ProviderError("down".to_string()))
            }

            async fn chat_with_tools(
                &self,
                _request: ChatCompletionRequest,
                _tools: Vec<crate::domain::llm::ToolDefinition>,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Err(LlmError::ProviderError("down".into()))
            }
        }

        let extractor = MemoryExtractor::new(Arc::new(ErrLlm));
        let memories = extractor.extract(1, &[]).await;
        assert!(memories.is_empty());
    }

    /// A configurable mock LLM for testing extraction filters.
    struct ConfigurableMockLlm {
        content: String,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ConfigurableMockLlm {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, LlmError> {
            Ok(ChatCompletionResponse {
                content: self.content.clone(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                usage: Some(crate::domain::llm::TokenUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    total_tokens: 150,
                }),
            })
        }

        async fn chat_with_tools(
            &self,
            _request: ChatCompletionRequest,
            _tools: Vec<crate::domain::llm::ToolDefinition>,
        ) -> Result<ChatCompletionResponse, LlmError> {
            Err(LlmError::ProviderError("not implemented for mock".into()))
        }
    }

    #[tokio::test]
    async fn system_prompt_is_first_message() {
        struct CaptureLlm;
        #[async_trait::async_trait]
        impl LlmProvider for CaptureLlm {
            async fn chat(
                &self,
                request: ChatCompletionRequest,
            ) -> Result<ChatCompletionResponse, LlmError> {
                // Assert system prompt is the first message
                assert!(!request.messages.is_empty());
                assert_eq!(request.messages[0].role, "system");
                assert!(request.messages[0].content.contains("记忆提取助手"));
                Ok(ChatCompletionResponse {
                    content: "[]".to_string(),
                    tool_calls: vec![],
                    finish_reason: "stop".to_string(),
                    usage: None,
                })
            }

            async fn chat_with_tools(
                &self,
                _request: ChatCompletionRequest,
                _tools: Vec<crate::domain::llm::ToolDefinition>,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Err(LlmError::ProviderError("not implemented".into()))
            }
        }

        let extractor = MemoryExtractor::new(Arc::new(CaptureLlm));
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: "hi".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        let _ = extractor.extract(1, &messages).await;
    }

    #[tokio::test]
    async fn filters_low_confidence_memories() {
        let mock = ConfigurableMockLlm {
            content: r#"[
                {"memory_type": "fact", "content": "用户喜欢夜跑", "confidence": 0.6}
            ]"#
            .into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let memories = extractor.extract(1, &[]).await;
        assert!(
            memories.is_empty(),
            "low confidence memories should be filtered"
        );
    }

    #[tokio::test]
    async fn filters_empty_content() {
        let mock = ConfigurableMockLlm {
            content: r#"[
                {"memory_type": "fact", "content": "   ", "confidence": 0.9}
            ]"#
            .into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let memories = extractor.extract(1, &[]).await;
        assert!(memories.is_empty(), "empty content should be filtered");
    }

    #[tokio::test]
    async fn deduplicates_identical_memories() {
        let mock = ConfigurableMockLlm {
            content: r#"[
                {"memory_type": "preference", "content": "用户喜欢安静的环境", "confidence": 0.9},
                {"memory_type": "preference", "content": "用户喜欢安静的环境", "confidence": 0.95}
            ]"#
            .into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let memories = extractor.extract(1, &[]).await;
        assert_eq!(
            memories.len(),
            1,
            "duplicate memories should be deduplicated"
        );
        assert_eq!(memories[0].content, "用户喜欢安静的环境");
    }

    #[tokio::test]
    async fn unknown_memory_type_is_rejected() {
        let mock = ConfigurableMockLlm {
            content: r#"[
                {"memory_type": "unknown_type", "content": "用户最近睡眠不好", "confidence": 0.9}
            ]"#
            .into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let memories = extractor.extract(1, &[]).await;
        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn truncates_long_content_to_300_chars() {
        let long_content = "A".repeat(500);
        let mock = ConfigurableMockLlm {
            content: format!(
                r#"[{{"memory_type": "fact", "content": "{}", "confidence": 0.9}}]"#,
                long_content
            ),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let memories = extractor.extract(1, &[]).await;
        assert_eq!(memories.len(), 1);
        // Content should be truncated to 300 chars + "..."
        // Note: count by chars not bytes
        assert!(
            memories[0].content.chars().count() <= 303,
            "content should be truncated to at most 300+3 chars"
        );
        assert!(memories[0].content.ends_with("..."));
    }

    #[tokio::test]
    async fn chinese_content_truncation_works() {
        let long_cn = "测试".repeat(200); // 400 Chinese chars
        let mock = ConfigurableMockLlm {
            content: format!(
                r#"[{{"memory_type": "preference", "content": "{}", "confidence": 0.9}}]"#,
                long_cn
            ),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let memories = extractor.extract(1, &[]).await;
        assert_eq!(memories.len(), 1);
        assert!(
            memories[0].content.chars().count() <= 303,
            "Chinese content should be truncated by char count, not bytes"
        );
    }

    fn proposed_memory() -> NewMemory {
        NewMemory {
            user_id: 1,
            memory_key: None,
            canonical_form: None,
            memory_type: "preference".into(),
            content: "user prefers tea".into(),
            confidence: 0.9,
            merge_decision: "new".into(),
            source_conversation_id: Some(1),
            source_message_id: Some(1),
        }
    }

    fn candidate_memory() -> UserMemory {
        UserMemory {
            memory_id: 7,
            user_id: 1,
            memory_type: "preference".into(),
            content: "user likes tea".into(),
            confidence: 0.8,
            reinforce_count: 0,
            reinforced_at: None,
            source_conversation_id: Some(1),
            source_message_id: Some(1),
            status: 1,
            metadata: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn merge_classifier_accepts_known_candidate() {
        let mock = ConfigurableMockLlm {
            content: r#"{"decision":"new_evidence","candidate_memory_id":7}"#.into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let decision = extractor
            .classify_merge(&proposed_memory(), &[candidate_memory()])
            .await;
        assert_eq!(decision, MemoryMergeDecision::NewEvidence(7));
    }

    #[tokio::test]
    async fn merge_classifier_ignores_qwen_think_block() {
        let mock = ConfigurableMockLlm {
            content: r#"<think>{"decision":"new","candidate_memory_id":null}</think>
            {"decision":"new_evidence","candidate_memory_id":7}"#
                .into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let decision = extractor
            .classify_merge(&proposed_memory(), &[candidate_memory()])
            .await;
        assert_eq!(decision, MemoryMergeDecision::NewEvidence(7));
    }

    #[tokio::test]
    async fn merge_classifier_rejects_unknown_candidate_id() {
        let mock = ConfigurableMockLlm {
            content: r#"{"decision":"contradiction","candidate_memory_id":99}"#.into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));
        let decision = extractor
            .classify_merge(&proposed_memory(), &[candidate_memory()])
            .await;
        assert_eq!(decision, MemoryMergeDecision::New);
    }

    #[tokio::test]
    async fn batch_merge_defaults_to_new_on_error() {
        /// Local LLM mock that always returns an error.
        struct BatchErrLlm;
        #[async_trait::async_trait]
        impl LlmProvider for BatchErrLlm {
            async fn chat(
                &self,
                _request: ChatCompletionRequest,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Err(LlmError::ProviderError("down".to_string()))
            }

            async fn chat_with_tools(
                &self,
                _request: ChatCompletionRequest,
                _tools: Vec<crate::domain::llm::ToolDefinition>,
            ) -> Result<ChatCompletionResponse, LlmError> {
                Err(LlmError::ProviderError("down".to_string()))
            }
        }

        let mock = BatchErrLlm;
        let extractor = MemoryExtractor::new(Arc::new(mock));

        let proposed = vec![proposed_memory()];
        let candidates = vec![candidate_memory()];
        let decisions = extractor.classify_merge_batch(&proposed, &candidates).await;

        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0], MemoryMergeDecision::New));
    }

    #[tokio::test]
    async fn batch_merge_defaults_to_new_on_empty_candidates() {
        let mock = ConfigurableMockLlm {
            content: r#"[{"index":0,"decision":"new","candidate_memory_id":null}]"#.into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));

        let proposed = vec![proposed_memory()];
        let decisions = extractor.classify_merge_batch(&proposed, &[]).await;

        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0], MemoryMergeDecision::New));
    }

    #[tokio::test]
    async fn batch_merge_accepts_new_evidence() {
        let mock = ConfigurableMockLlm {
            content: r#"[{"index":0,"decision":"new_evidence","candidate_memory_id":7}]"#.into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));

        let proposed = vec![proposed_memory()];
        let candidates = vec![candidate_memory()];
        let decisions = extractor.classify_merge_batch(&proposed, &candidates).await;

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0], MemoryMergeDecision::NewEvidence(7));
    }

    #[tokio::test]
    async fn batch_merge_ignores_qwen_think_block_and_markdown_fence() {
        let mock = ConfigurableMockLlm {
            content: r#"<think>[{"index":0,"decision":"new","candidate_memory_id":null}]</think>
            ```json
            [{"index":0,"decision":"new_evidence","candidate_memory_id":7}]
            ```"#
                .into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));

        let proposed = vec![proposed_memory()];
        let candidates = vec![candidate_memory()];
        let decisions = extractor.classify_merge_batch(&proposed, &candidates).await;

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0], MemoryMergeDecision::NewEvidence(7));
    }

    #[tokio::test]
    async fn batch_merge_rejects_unknown_candidate_id() {
        let mock = ConfigurableMockLlm {
            content: r#"[{"index":0,"decision":"contradiction","candidate_memory_id":99}]"#.into(),
        };
        let extractor = MemoryExtractor::new(Arc::new(mock));

        let proposed = vec![proposed_memory()];
        let candidates = vec![candidate_memory()];
        let decisions = extractor.classify_merge_batch(&proposed, &candidates).await;

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0], MemoryMergeDecision::New);
    }

    #[test]
    fn clean_llm_json_response_extracts_first_complete_json_value() {
        let cleaned = clean_llm_json_response(
            r#"<think>{"draft":true}</think>
            prefix
            {"decision":"new","candidate_memory_id":null}
            suffix"#,
        );

        assert_eq!(cleaned, r#"{"decision":"new","candidate_memory_id":null}"#);
    }
}
