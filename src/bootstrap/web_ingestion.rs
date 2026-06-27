//! 启动网页知识摄取子系统。
//!
//! 职责（task-book §4.1）：依赖组装、主开关、调度器/分发器初始化。不含业务逻辑——业务逻辑在
//! `application::web_ingestion` 中。

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tracing::info;

use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::review_service::KnowledgeReviewService;
use crate::app::web_ingestion::{dispatcher, scheduler};
use crate::bootstrap::tasks::BackgroundTasks;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::RAGRepoT;
use crate::domain::vector_store::VectorStoreT;
use crate::infra::web_ingestion::distiller::OpenAiKnowledgeDistiller;
use crate::infra::web_ingestion::fetcher::WebFetcher;
use crate::infra::web_ingestion::repo::*;
use crate::infra::web_ingestion::review_repository::SeaOrmKnowledgeReviewRepository;
use crate::shared::config::AppConfig;
use crate::shared::error::AppError;

pub async fn init_web_ingestion(
    config: &AppConfig,
    db: &DatabaseConnection,
    vector_store: &Option<Arc<dyn VectorStoreT>>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    rag_repo: &Arc<dyn RAGRepoT>,
    background: &mut BackgroundTasks,
) -> Result<Arc<KnowledgeReviewService>, AppError> {
    let wc = &config.web_ingestion;
    let review_service = Arc::new(KnowledgeReviewService::new(
        Arc::new(SeaOrmKnowledgeReviewRepository::new(db.clone())),
        wc.enabled && wc.dispatcher_enabled,
    ));

    // ── 主开关（§5.1）───────────────────────────────────────────────
    // 即使 scheduler_enabled / dispatcher_enabled 为 true，除非
    // web_ingestion.enabled 为 true，否则什么也不启动。纵深防御：main.rs
    // 也设置了同样的门，但这里也强制执行，使得代码可直接测试且无法绕过。
    let gate = WorkerGate::from_config(wc);
    if !gate.any() {
        info!(
            enabled = wc.enabled,
            "网页知识摄取：没有要启动的 Worker（主开关关闭或两个 Worker 均禁用）"
        );
        return Ok(review_service);
    }

    let fetcher = Arc::new(
        WebFetcher::new(wc)
            .map_err(|e| AppError::internal(format!("web ingestion fetcher init: {e}")))?,
    );
    let distiller = Arc::new(
        OpenAiKnowledgeDistiller::new(wc.distill_llm.clone())
            .map_err(|e| AppError::internal(format!("web ingestion distiller init: {e}")))?,
    );

    let ctx = PipelineContext {
        source_repo: Arc::new(WebSourceRepo::new(db.clone())),
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
        distiller,
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
        proxy_enabled = !wc.fetch_proxy_url.trim().is_empty(),
        scheduler_interval_secs = wc.scheduler_interval_secs,
        dispatcher_interval_secs = wc.dispatcher_interval_secs,
        outbox_batch_size = wc.outbox_batch_size,
        dispatcher_parallelism = wc.dispatcher_parallelism,
        max_urls_per_source_per_job = wc.max_urls_per_source_per_job,
        "网页知识摄取基础设施初始化完成"
    );

    // ── 调度器循环 ──────────────────────────────────────────────────────
    if gate.scheduler {
        let source_repo = Arc::clone(&ctx.source_repo);
        let crawl_job_repo = Arc::clone(&ctx.crawl_job_repo);
        let outbox_repo = Arc::clone(&ctx.outbox_repo);
        let pipeline_version = wc.pipeline_version.clone();
        let sched_interval = wc.scheduler_interval_secs.max(1);

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
                    tracing::warn!(error = %e, "网页知识摄取调度器周期执行失败");
                }
            }
        }));
        info!("网页知识摄取调度器已启动");
    }

    // ── Dispatcher loop ──────────────────────────────────────────────────────
    if gate.dispatcher {
        let ctx = ctx.clone();
        let disp_interval = wc.dispatcher_interval_secs.max(1);
        background.spawn(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(disp_interval));
            loop {
                interval.tick().await;
                if let Err(e) = dispatcher::run_tick(&ctx).await {
                    tracing::warn!(error = %e, "网页知识摄取分发器周期执行失败");
                }
            }
        }));
        info!("web ingestion outbox dispatcher started");
    }

    Ok(review_service)
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
