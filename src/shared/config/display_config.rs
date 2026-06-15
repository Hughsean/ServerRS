use std::fmt;

use super::AppConfig;

/// Expose parts of the configuration for diagnostic purposes.
/// Does NOT log secrets (api keys, passwords, jwt secrets).
impl fmt::Display for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "server   → {}:{}",
            self.server.host, self.server.port
        )?;

        writeln!(
            f,
            "database → {} (max {})",
            self.database.url, self.database.max_connections
        )?;

        writeln!(
            f,
            "jwt      → access_ttl={}s, refresh_ttl={}s",
            self.jwt.access_ttl_secs, self.jwt.refresh_ttl_secs
        )?;

        writeln!(
            f,
            "auth     → max_login_attempts={}, lockout_duration={}s",
            self.auth.max_login_attempts, self.auth.lockout_duration_secs
        )?;

        writeln!(
            f,
            "session  → timeout={}s, cleanup={:?}",
            self.session.timeout_seconds,
            self.session
                .cleanup_interval_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| format!("{}s", self.session.cleanup_interval_secs))
        )?;

        writeln!(
            f,
            "storage  → backend={}, path={}, url={}",
            self.storage.backend, self.storage.base_path, self.storage.base_url
        )?;

        writeln!(
            f,
            "detector → context_window={}, threshold={}, llm_enabled={}, max_retries={}",
            self.detector.context_window_size,
            self.detector.confidence_threshold,
            self.detector.llm_enabled,
            self.detector.max_retries
        )?;

        writeln!(
            f,
            "ollama   → model={}, temperature={}",
            self.ollama.model, self.ollama.temperature
        )?;

        writeln!(
            f,
            "llm      → provider={}, model={}, embedding={}, timeout={}s",
            self.llm.provider,
            self.llm.chat_model,
            self.llm.embedding_model,
            self.llm.timeout_secs
        )?;

        writeln!(
            f,
            "agent    → enabled={}, memory={}, rag={}, summary={}",
            self.agent.enabled,
            self.agent.memory_enabled,
            self.agent.rag_enabled,
            self.agent.summary_enabled
        )?;

        writeln!(
            f,
            "rag      → chunk_size={}, overlap={}, top_k={}",
            self.rag.chunk_size, self.rag.chunk_overlap, self.rag.top_k
        )?;

        writeln!(
            f,
            "qdrant   → url={}",
            if self.qdrant.api_key.is_some() {
                format!("{} (api-key set)", self.qdrant.url)
            } else {
                self.qdrant.url.clone()
            }
        )?;

        writeln!(
            f,
            "embedding→ provider={}, model={}, dim={}, batch={}",
            self.embedding.provider,
            self.embedding.model,
            self.embedding.dimension,
            self.embedding.batch_size
        )?;

        writeln!(
            f,
            "web ing  → enabled={}, scheduler={}, dispatcher={}",
            self.web_ingestion.enabled,
            self.web_ingestion.scheduler_enabled,
            self.web_ingestion.dispatcher_enabled
        )?;

        writeln!(
            f,
            "tts      → provider={}, voice={}, encoding={}",
            self.tts.provider, self.tts.default_voice, self.tts.default_encoding
        )
    }
}
