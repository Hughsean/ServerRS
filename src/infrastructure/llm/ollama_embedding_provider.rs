use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::domain::llm::{EmbeddingProvider, LlmError};

/// Dedicated embedding provider backed by Ollama (or any OpenAI-compatible
/// `/embeddings` endpoint).
///
/// Separate from `OllamaProvider` (chat) so that embedding uses its own
/// model & endpoint config.
#[derive(Clone)]
pub struct OllamaEmbeddingProvider {
    base_url: String,
    model: String,
    client: reqwest::Client,
    /// Expected embedding dimension (validated on first call).
    expected_dimension: usize,
}

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

impl OllamaEmbeddingProvider {
    pub fn new(base_url: String, model: String, expected_dimension: usize) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            expected_dimension,
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest Client should build"),
        }
    }

    fn embed_url(&self) -> String {
        format!("{}/embeddings", self.base_url)
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = self.embed_url();
        let body = EmbedRequest {
            model: self.model.clone(),
            input: texts.to_vec(),
        };

        debug!("OllamaEmbeddingProvider.embed -> {url} ({} texts)", texts.len());

        let response = self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout(format!("embedding request to {url} timed out"))
                } else if e.is_connect() {
                    LlmError::Connection(format!("cannot connect to {url}: {e}"))
                } else {
                    LlmError::ProviderError(format!("embedding request to {url} failed: {e}"))
                }
            })?;

        let status = response.status();
        let body_bytes = response.bytes().await.map_err(|e| {
            LlmError::InvalidResponse(format!("failed to read embedding response: {e}"))
        })?;

        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&body_bytes);
            return Err(LlmError::ProviderError(format!(
                "embedding returned {status}: {body_text}"
            )));
        }

        let embed_response: EmbedResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                LlmError::InvalidResponse(format!("failed to parse embedding response: {e}"))
            })?;

        if embed_response.data.len() != texts.len() {
            return Err(LlmError::EmbeddingError(format!(
                "expected {} embeddings but got {}",
                texts.len(),
                embed_response.data.len()
            )));
        }

        // Validate dimension on first non-empty result
        for data in &embed_response.data {
            let dim = data.embedding.len();
            if dim != self.expected_dimension && self.expected_dimension != 0 {
                return Err(LlmError::EmbeddingError(format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.expected_dimension, dim
                )));
            }
        }

        Ok(embed_response
            .data
            .into_iter()
            .map(|d| d.embedding)
            .collect())
    }
}
