use std::fmt;

use super::AppConfig;
use super::ssh::TunnelDirection;

/// 暴露部分配置用于诊断目的。
/// 不记录密钥（API 密钥、密码、JWT 密钥）。
impl fmt::Display for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "server   → {}:{}", self.server.host, self.server.port)?;

        writeln!(
            f,
            "database → {} (max {})",
            redact_url_credentials(&self.database.url),
            self.database.max_connections
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
            self.llm.provider, self.llm.chat_model, self.llm.embedding_model, self.llm.timeout_secs
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
            "fresh   → enabled={}, scheduler={}, dispatcher={}, scheduler_interval={}s, dispatcher_interval={}s, collection={}, topic_items={}, expired_vectors={}, retrieval_chunks={}, distill_model={}",
            self.fresh_context.enabled,
            self.fresh_context.scheduler_enabled,
            self.fresh_context.dispatcher_enabled,
            self.fresh_context.scheduler_interval_secs,
            self.fresh_context.dispatcher_interval_secs,
            self.fresh_context.qdrant_collection,
            self.fresh_context.max_topic_items_per_tick,
            self.fresh_context.max_expired_vectors_per_tick,
            self.fresh_context.max_retrieval_chunks,
            self.fresh_context.distill_llm.chat_model
        )?;

        writeln!(
            f,
            "semantic→ enabled={}, provider={}, taxonomies={}",
            self.semantic_classification.enabled,
            self.semantic_classification.provider,
            self.semantic_classification.taxonomies.len()
        )?;

        writeln!(
            f,
            "routing → enabled={}, taxonomy={}, margin={}",
            self.context_routing.enabled,
            self.context_routing.taxonomy,
            self.context_routing.margin
        )?;

        writeln!(
            f,
            "tts      → provider={}, voice={}, encoding={}",
            self.tts.provider, self.tts.default_voice, self.tts.default_encoding
        )?;

        if !self.ssh_tunnels.is_empty() {
            for (name, tunnel) in &self.ssh_tunnels {
                let direction_label = match tunnel.direction {
                    TunnelDirection::Local => "-L",
                    TunnelDirection::Remote => "-R",
                };
                match &tunnel.user {
                    Some(user) => writeln!(
                        f,
                        "ssh      → {}: ({}) {}@{}:{} → 127.0.0.1:{} (bind={})",
                        name,
                        direction_label,
                        user,
                        tunnel.host,
                        tunnel.remote_port,
                        tunnel.local_port,
                        tunnel.bind_address.as_deref().unwrap_or("127.0.0.1"),
                    )?,
                    None => writeln!(
                        f,
                        "ssh      → {}: ({}) {}:{} → 127.0.0.1:{} (bind={})",
                        name,
                        direction_label,
                        tunnel.host,
                        tunnel.remote_port,
                        tunnel.local_port,
                        tunnel.bind_address.as_deref().unwrap_or("127.0.0.1"),
                    )?,
                }
            }
        }
        Ok(())
    }
}

fn redact_url_credentials(raw_url: &str) -> String {
    let raw_url = raw_url.trim();
    if raw_url.is_empty() {
        return "<empty>".into();
    }

    let Ok(mut url) = reqwest::Url::parse(raw_url) else {
        return "<invalid-url>".into();
    };

    if url.password().is_some() {
        let _ = url.set_password(Some("***"));
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use crate::shared::config::AppConfig;

    #[test]
    fn display_redacts_database_password() {
        let mut config = AppConfig::default();
        config.database.url = "mysql://root:very-secret@127.0.0.1:3306/digital_companion".into();

        let rendered = config.to_string();

        assert!(!rendered.contains("very-secret"));
        assert!(rendered.contains("mysql://root:***@127.0.0.1:3306/digital_companion"));
    }

    #[test]
    fn display_does_not_echo_invalid_database_url() {
        let mut config = AppConfig::default();
        config.database.url = "not a url with secret".into();

        let rendered = config.to_string();

        assert!(!rendered.contains("not a url with secret"));
        assert!(rendered.contains("database → <invalid-url>"));
    }
}
