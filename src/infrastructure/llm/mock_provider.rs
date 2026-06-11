use async_trait::async_trait;

use crate::domain::llm::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingProvider, LlmError, LlmProvider,
    ToolDefinition,
};

/// Mock LLM provider for testing.
///
/// Returns a fixed JSON response for every chat request.
pub struct MockLlmProvider {
    /// The fixed JSON string returned as the assistant content.
    fixed_response: String,
}

impl MockLlmProvider {
    pub fn new(fixed_response: impl Into<String>) -> Self {
        Self {
            fixed_response: fixed_response.into(),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn chat(&self, _request: ChatCompletionRequest) -> Result<ChatCompletionResponse, LlmError> {
        Ok(ChatCompletionResponse {
            content: self.fixed_response.clone(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: None,
        })
    }

    async fn chat_with_tools(
        &self,
        _request: ChatCompletionRequest,
        _tools: Vec<ToolDefinition>,
    ) -> Result<ChatCompletionResponse, LlmError> {
        // Same fixed response when tools are provided.
        Ok(ChatCompletionResponse {
            content: self.fixed_response.clone(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: None,
        })
    }
}

/// Mock embedding provider for testing.
///
/// Returns random vectors of a fixed dimension for every embed call.
pub struct MockEmbeddingProvider {
    dimension: usize,
}

impl MockEmbeddingProvider {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        let embeddings: Vec<Vec<f32>> = texts
            .iter()
            .map(|_| vec![0.0_f32; self.dimension])
            .collect();
        Ok(embeddings)
    }
}
