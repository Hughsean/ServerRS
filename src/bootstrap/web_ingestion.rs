//! Bootstrap the web ingestion subsystem.
//!
//! Responsibilities (task-book §4.1): dependency assembly, master-switch gate,
//! scheduler/dispatcher initialisation. NO business logic — that lives in
//! `application::web_ingestion`.

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tracing::info;

use crate::application::web_ingestion::pipeline_context::PipelineContext;
use crate::application::web_ingestion::{dispatcher, scheduler};
use crate::bootstrap::tasks::BackgroundTasks;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::RAGRepository;
use crate::domain::vector_store::VectorStore;
use crate::infrastructure::web_ingestion::fetcher::WebFetcher;
use crate::infrastructure::web_ingestion::repositories::*;
use crate::shared::config::AppConfig;
use crate::shared::error::AppError;

pub async fn init_web_ingestion(
    config: &AppConfig,
    db: &DatabaseConnection,
    vector_store: &Option<Arc<dyn VectorStore>>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    rag_repo: &Arc<dyn RAGRepository>,
    background: &mut BackgroundTasks,
) -> Result<(), AppError> {
    let wc = &config.web_ingestion;

    // ── Master switch (§5.1) ───────────────────────────────────────────────
    // Even if scheduler_enabled / dispatcher_enabled are true, nothing starts
    // unless web_ingestion.enabled is true. Defence-in-depth: main.rs also
    // gates on this, but the gate is enforced here too so it is directly
    // testable and impossible to bypass.
    let gate = WorkerGate::from_config(wc);
    if !gate.any() {
        info!(
            enabled = wc.enabled,
            "web ingestion: no workers to start (master switch off or both workers disabled)"
        );
        return Ok(());
    }

    let fetcher = Arc::new(
        WebFetcher::new(wc)
            .map_err(|e| AppError::internal(format!("web ingestion fetcher init: {e}")))?,
    );

    let ctx = PipelineContext {
        source_repo: Arc::new(SeaOrmWebSourceRepository::new(db.clone())),
        source_url_repo: Arc::new(SeaOrmWebSourceUrlRepository::new(db.clone())),
        crawl_job_repo: Arc::new(SeaOrmWebCrawlJobRepository::new(db.clone())),
        page_repo: Arc::new(SeaOrmWebPageRepository::new(db.clone())),
        run_repo: Arc::new(SeaOrmIngestionRunRepository::new(db.clone())),
        publish_repo: Arc::new(SeaOrmPublishRecordRepository::new(db.clone())),
        chunk_manifest_repo: Arc::new(SeaOrmChunkManifestRepository::new(db.clone())),
        vector_manifest_repo: Arc::new(SeaOrmVectorManifestRepository::new(db.clone())),
        outbox_repo: Arc::new(SeaOrmOutboxRepository::new(db.clone())),
        audit_repo: Arc::new(SeaOrmAuditLogRepository::new(db.clone())),
        rag_repo: Arc::clone(rag_repo),
        fetcher,
        embedding_provider: Arc::clone(embedding_provider),
        vector_store: vector_store.as_ref().map(Arc::clone),
        config: wc.clone(),
        embedding: config.embedding.clone(),
    };

    info!(
        enabled = wc.enabled,
        scheduler = wc.scheduler_enabled,
        dispatcher = wc.dispatcher_enabled,
        auto_publish = wc.auto_publish,
        "web ingestion infrastructure initialised"
    );

    // ── Scheduler loop ──────────────────────────────────────────────────────
    if gate.scheduler {
        let source_repo = Arc::clone(&ctx.source_repo);
        let crawl_job_repo = Arc::clone(&ctx.crawl_job_repo);
        let outbox_repo = Arc::clone(&ctx.outbox_repo);
        let pipeline_version = wc.pipeline_version.clone();
        let sched_interval = wc.scheduler_interval_secs;

        background.spawn(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(sched_interval));
            loop {
                interval.tick().await;
                if let Err(e) = scheduler::run_tick(
                    &source_repo,
                    &crawl_job_repo,
                    &outbox_repo,
                    &pipeline_version,
                )
                .await
                {
                    tracing::warn!(error = %e, "web ingestion scheduler tick failed");
                }
            }
        }));
        info!("web ingestion scheduler started");
    }

    // ── Dispatcher loop ──────────────────────────────────────────────────────
    if gate.dispatcher {
        let ctx = ctx.clone();
        let disp_interval = wc.dispatcher_interval_secs;
        background.spawn(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(disp_interval));
            loop {
                interval.tick().await;
                if let Err(e) = dispatcher::run_tick(&ctx).await {
                    tracing::warn!(error = %e, "web ingestion dispatcher tick failed");
                }
            }
        }));
        info!("web ingestion outbox dispatcher started");
    }

    Ok(())
}

/// Resolved decision of which workers may start, after applying the master
/// switch (§5.1). A worker only runs when BOTH the master switch is on AND that
/// worker's own flag is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerGate {
    pub scheduler: bool,
    pub dispatcher: bool,
}

impl WorkerGate {
    pub fn from_config(wc: &crate::shared::config::WebIngestionConfig) -> Self {
        // Master switch gates everything (§5.1): if enabled is false, no worker
        // starts regardless of scheduler_enabled / dispatcher_enabled.
        if !wc.enabled {
            return Self {
                scheduler: false,
                dispatcher: false,
            };
        }
        Self {
            scheduler: wc.scheduler_enabled,
            dispatcher: wc.dispatcher_enabled,
        }
    }

    pub fn any(&self) -> bool {
        self.scheduler || self.dispatcher
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerGate;
    use crate::shared::config::WebIngestionConfig;

    #[test]
    fn master_switch_off_starts_nothing_even_if_workers_enabled() {
        // §16.1 #2: scheduler_enabled=true but enabled=false → still nothing.
        let wc = WebIngestionConfig {
            enabled: false,
            scheduler_enabled: true,
            dispatcher_enabled: true,
            ..WebIngestionConfig::default()
        };
        let gate = WorkerGate::from_config(&wc);
        assert!(!gate.scheduler, "scheduler must not start when master off");
        assert!(
            !gate.dispatcher,
            "dispatcher must not start when master off"
        );
        assert!(!gate.any());
    }

    #[test]
    fn default_config_is_fully_disabled() {
        // §16.1 #1 + §5.1 defaults: everything off by default.
        let wc = WebIngestionConfig::default();
        assert!(!wc.enabled);
        assert!(!wc.scheduler_enabled);
        assert!(!wc.dispatcher_enabled);
        assert!(!wc.auto_publish);
        let gate = WorkerGate::from_config(&wc);
        assert!(!gate.any());
    }

    #[test]
    fn master_on_respects_individual_worker_flags() {
        let wc = WebIngestionConfig {
            enabled: true,
            scheduler_enabled: true,
            dispatcher_enabled: false,
            ..WebIngestionConfig::default()
        };
        let gate = WorkerGate::from_config(&wc);
        assert!(gate.scheduler);
        assert!(!gate.dispatcher);
        assert!(gate.any());
    }

    #[test]
    fn master_on_but_both_workers_off_starts_nothing() {
        let wc = WebIngestionConfig {
            enabled: true,
            scheduler_enabled: false,
            dispatcher_enabled: false,
            ..WebIngestionConfig::default()
        };
        let gate = WorkerGate::from_config(&wc);
        assert!(!gate.any());
    }
}
