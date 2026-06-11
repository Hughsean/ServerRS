use std::sync::Arc;

use serde::Deserialize;

use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::memory::NewMemory;

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

fn default_confidence() -> f64 {
    0.7
}

const EXTRACTION_PROMPT: &str = "\
You are a memory-extraction assistant. Analyze the conversation above and extract \
any important personal information, preferences, goals, or safety-relevant facts \
about the user. Return a JSON array of objects with these fields:
  - memory_type: one of \"preference\", \"profile\", \"fact\", \"emotional_pattern\", \"goal\", \"safety_note\"
  - content: a concise statement of the memory (e.g. \"user mentioned they enjoy hiking\")
  - confidence: a number between 0.0 and 1.0 indicating how certain you are

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
        let mut prompt_messages = messages.to_vec();
        prompt_messages.push(ChatMessage {
            role: "system".to_string(),
            content: EXTRACTION_PROMPT.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

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

        items
            .into_iter()
            .map(|item| NewMemory {
                user_id,
                memory_type: if item.memory_type.is_empty() {
                    "fact".to_string()
                } else {
                    item.memory_type
                },
                content: item.content,
                confidence: item.confidence.clamp(0.0, 1.0),
                source_conversation_id: None,
                source_message_id: None,
            })
            .collect()
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
        assert_eq!(memories.len(), 2);
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
}
