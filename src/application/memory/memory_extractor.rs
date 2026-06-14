use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::memory::{NewMemory, UserMemory, is_allowed_memory_type};

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
You are a memory-extraction assistant. Analyze the conversation and extract \
only durable user-stated preferences, facts, emotional patterns, or goals \
about the user. Return a JSON array of objects with these fields:
  - memory_type: one of \"preference\", \"fact\", \"emotional_pattern\", \"goal\"
  - content: a concise statement of the memory (e.g. \"user mentioned they enjoy hiking\")
  - confidence: a number between 0.0 and 1.0 indicating how certain you are

只提取用户明确表达、对长期陪伴有用的信息。不要提取助手建议。不要保存一次性闲聊、网页内容、工具输出或不确定推测。不得保存风险标签、危机信号、安全判断、自伤风险分析、临床诊断、人格障碍标签、身份证号、手机号、住址、密码、银行卡、API Key 等内容。

Return ONLY a valid JSON array with no additional text or markdown. \
If nothing noteworthy is found, return an empty array [].";

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
        };

        let response = match self.llm.chat(request).await {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let trimmed = response.content.trim();
        // Strip markdown fences if present
        let json_str = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim();

        let items: Vec<LlmMemoryItem> = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => {
                // Try parsing as a single object wrapped in an array
                let single: LlmMemoryItem = match serde_json::from_str(json_str) {
                    Ok(v) => v,
                    Err(_) => return Vec::new(),
                };
                vec![single]
            }
        };

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

            // Batch dedup by type + content
            let dedup_key = format!("{memory_type}|{truncated}");
            if !seen.insert(dedup_key) {
                continue;
            }

            result.push(NewMemory {
                user_id,
                memory_key: None,
                canonical_form: None,
                memory_type,
                content: truncated,
                confidence,
                merge_decision: "new".into(),
                source_conversation_id: None,
                source_message_id: None,
            });
        }

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
                    content: "Classify how a proposed user memory relates to existing memories. \
                              Return only JSON: {\"decision\":\"same|related|new_evidence|contradiction|new\",\
                              \"candidate_memory_id\":number|null}. Use a candidate id only for \
                              same, new_evidence, or contradiction. Do not infer diagnoses or risk labels."
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
        };

        let Ok(response) = self.llm.chat(request).await else {
            return MemoryMergeDecision::New;
        };
        let trimmed = response.content.trim();
        let json_str = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .and_then(|value| value.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim();
        let Ok(result) = serde_json::from_str::<LlmMergeDecision>(json_str) else {
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
                    {"memory_type": "preference", "content": "user likes jazz music", "confidence": 0.9},
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
                assert!(
                    request.messages[0]
                        .content
                        .contains("memory-extraction assistant")
                );
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
}
