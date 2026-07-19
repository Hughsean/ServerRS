//! 启动 Fresh Context 短期上下文子系统。
//!
//! 这里只做依赖装配和后台循环启动；采集、蒸馏和发布策略留在应用层服务内。

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

use crate::app::fresh_context::collector::FreshCollectorService;
use crate::app::fresh_context::config::FreshContextUseCaseConfig;
use crate::app::fresh_context::indexer::{FreshIndexStats, FreshIndexerService};
use crate::app::fresh_context::pipeline::{FreshPipelineService, FreshPipelineStats};
use crate::app::fresh_context::topic_clusterer::{
    FreshTopicClusterStats, FreshTopicClustererService,
};
use crate::bootstrap::tasks::BackgroundTasks;
use crate::domain::fresh_context::FreshContextRepoT;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_store::VectorStoreT;
use crate::infra::fresh_context::config::FreshContextAdapterConfig;
use crate::infra::fresh_context::distiller::OpenAiFreshContextDistiller;
use crate::infra::fresh_context::fetcher::FreshContextWebFetcher;
use crate::infra::repo::seaorm_impl::fresh_context::FreshContextRepo;
use crate::shared::config::{AppConfig, FreshContextConfig};
use crate::shared::error::AppError;

pub async fn init_fresh_context(
    config: &AppConfig,
    db: &DatabaseConnection,
    vector_store: &Option<Arc<dyn VectorStoreT>>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    background: &mut BackgroundTasks,
) -> Result<(), AppError> {
    let fc = &config.fresh_context;
    let use_case_config = FreshContextUseCaseConfig::from(fc);
    let adapter_config = FreshContextAdapterConfig::from(fc);
    let gate = WorkerGate::from_config(fc);
    if !gate.any() {
        info!(
            enabled = fc.enabled,
            "Fresh Context：没有要启动的 Worker（主开关关闭或两个 Worker 均禁用）"
        );
        return Ok(());
    }

    let repo: Arc<dyn FreshContextRepoT> = Arc::new(FreshContextRepo::new(db.clone()));

    info!(
        enabled = fc.enabled,
        scheduler = fc.scheduler_enabled,
        dispatcher = fc.dispatcher_enabled,
        scheduler_interval_secs = fc.scheduler_interval_secs,
        dispatcher_interval_secs = fc.dispatcher_interval_secs,
        max_sources_per_tick = fc.max_sources_per_tick,
        max_items_per_source = fc.max_items_per_source,
        max_pipeline_items_per_tick = fc.max_pipeline_items_per_tick,
        max_indexable_chunks_per_tick = fc.max_indexable_chunks_per_tick,
        vector_index_name = %adapter_config.vector_index_name,
        proxy_enabled = !fc.fetch_proxy_url.trim().is_empty(),
        "Fresh Context 基础设施初始化完成"
    );

    if gate.scheduler {
        let collector = Arc::new(FreshCollectorService::new(
            Arc::clone(&repo),
            Arc::new(FreshContextWebFetcher::new(&adapter_config)?),
            use_case_config.clone(),
        ));
        let sched_interval = fc.scheduler_interval_secs.max(1);
        background.spawn(tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(sched_interval));
            loop {
                ticker.tick().await;
                match collector.collect_tick().await {
                    Ok(stats) => {
                        if stats.items_inserted > 0 || stats.sources_failed > 0 {
                            info!(
                                sources_seen = stats.sources_seen,
                                sources_collected = stats.sources_collected,
                                sources_failed = stats.sources_failed,
                                items_seen = stats.items_seen,
                                items_inserted = stats.items_inserted,
                                items_duplicated = stats.items_duplicated,
                                items_skipped_short = stats.items_skipped_short,
                                "Fresh Context 采集周期完成"
                            );
                        } else {
                            debug!(
                                sources_seen = stats.sources_seen,
                                sources_collected = stats.sources_collected,
                                "Fresh Context 采集周期完成，无新增 item"
                            );
                        }
                    }
                    Err(error) => warn!(error = %error, "Fresh Context 采集周期执行失败"),
                }
            }
        }));
        info!("Fresh Context collector 已启动");
    }

    if gate.dispatcher {
        let pipeline = Arc::new(FreshPipelineService::new(
            Arc::clone(&repo),
            Arc::new(OpenAiFreshContextDistiller::new(
                adapter_config.distill_llm.clone(),
            )?),
            use_case_config.clone(),
        ));
        let disp_interval = fc.dispatcher_interval_secs.max(1);
        background.spawn(tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(disp_interval));
            loop {
                ticker.tick().await;
                match pipeline.run_tick().await {
                    Ok(stats) => log_pipeline_stats(stats),
                    Err(error) => warn!(error = %error, "Fresh Context pipeline 周期执行失败"),
                }
            }
        }));
        info!("Fresh Context pipeline dispatcher 已启动");

        let topic_clusterer = Arc::new(FreshTopicClustererService::new(
            Arc::clone(&repo),
            use_case_config.clone(),
        ));
        let topic_interval = fc.dispatcher_interval_secs.max(1);
        background.spawn(tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(topic_interval));
            loop {
                ticker.tick().await;
                match topic_clusterer.run_tick().await {
                    Ok(stats) => log_topic_stats(stats),
                    Err(error) => warn!(error = %error, "Fresh Context topic 聚合周期执行失败"),
                }
            }
        }));
        info!("Fresh Context topic clusterer 已启动");

        if let Some(vector_store) = vector_store.as_ref() {
            let indexer = Arc::new(FreshIndexerService::new(
                Arc::clone(&repo),
                Arc::clone(vector_store),
                Arc::clone(embedding_provider),
                use_case_config,
                adapter_config.vector_index_name,
                config.embedding.provider.clone(),
                config.embedding.model.clone(),
            ));
            indexer.ensure_collection().await?;
            let index_interval = fc.dispatcher_interval_secs.max(1);
            background.spawn(tokio::spawn(async move {
                let mut ticker = interval(Duration::from_secs(index_interval));
                loop {
                    ticker.tick().await;
                    match indexer.run_tick().await {
                        Ok(stats) => log_index_stats(stats),
                        Err(error) => {
                            warn!(error = %error, "Fresh Context indexer 周期执行失败")
                        }
                    }
                }
            }));
            info!("Fresh Context vector indexer 已启动");
        } else {
            warn!(
                "Fresh Context dispatcher 已启用，但 vector_store 未启用，fresh_chunks 不会写入向量库"
            );
        }
    }

    Ok(())
}

fn log_pipeline_stats(stats: FreshPipelineStats) {
    if stats.expired_items > 0
        || stats.distilled > 0
        || stats.published > 0
        || stats.rejected > 0
        || stats.failed > 0
    {
        info!(
            expired_items = stats.expired_items,
            fetched_seen = stats.fetched_seen,
            distilled = stats.distilled,
            published = stats.published,
            rejected = stats.rejected,
            skipped = stats.skipped,
            failed = stats.failed,
            "Fresh Context pipeline 周期完成"
        );
    } else {
        debug!(
            expired_items = stats.expired_items,
            fetched_seen = stats.fetched_seen,
            skipped = stats.skipped,
            "Fresh Context pipeline 周期完成，无待处理 item"
        );
    }
}

fn log_index_stats(stats: FreshIndexStats) {
    if stats.published_seen > 0
        || stats.chunks_created > 0
        || stats.indexable_seen > 0
        || stats.chunks_indexed > 0
        || stats.expired_vectors_seen > 0
        || stats.expired_vectors_deleted > 0
        || stats.failed > 0
    {
        info!(
            expired_vectors_seen = stats.expired_vectors_seen,
            expired_vectors_deleted = stats.expired_vectors_deleted,
            published_seen = stats.published_seen,
            chunks_created = stats.chunks_created,
            indexable_seen = stats.indexable_seen,
            chunks_indexed = stats.chunks_indexed,
            skipped = stats.skipped,
            failed = stats.failed,
            "Fresh Context indexer 周期完成"
        );
    } else {
        debug!("Fresh Context indexer 周期完成，无待索引 chunk");
    }
}

fn log_topic_stats(stats: FreshTopicClusterStats) {
    if stats.topics_upserted > 0
        || stats.evidences_linked > 0
        || stats.chunks_assigned > 0
        || stats.failed > 0
    {
        info!(
            active_seen = stats.active_seen,
            topics_upserted = stats.topics_upserted,
            evidences_linked = stats.evidences_linked,
            chunks_assigned = stats.chunks_assigned,
            skipped = stats.skipped,
            failed = stats.failed,
            "Fresh Context topic 聚合周期完成"
        );
    } else {
        debug!(
            active_seen = stats.active_seen,
            skipped = stats.skipped,
            "Fresh Context topic 聚合周期完成，无待聚合 item"
        );
    }
}

/// Fresh Context 的主开关和子 Worker 开关统一在启动层收敛。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerGate {
    pub scheduler: bool,
    pub dispatcher: bool,
}

impl WorkerGate {
    pub fn from_config(fc: &FreshContextConfig) -> Self {
        if !fc.enabled {
            return Self {
                scheduler: false,
                dispatcher: false,
            };
        }
        Self {
            scheduler: fc.scheduler_enabled,
            dispatcher: fc.dispatcher_enabled,
        }
    }

    pub fn any(&self) -> bool {
        self.scheduler || self.dispatcher
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerGate;
    use crate::shared::config::FreshContextConfig;

    #[test]
    fn master_switch_off_starts_nothing() {
        let fc = FreshContextConfig {
            enabled: false,
            scheduler_enabled: true,
            dispatcher_enabled: true,
            ..FreshContextConfig::default()
        };
        let gate = WorkerGate::from_config(&fc);
        assert!(!gate.scheduler);
        assert!(!gate.dispatcher);
        assert!(!gate.any());
    }

    #[test]
    fn default_config_is_fully_disabled() {
        let fc = FreshContextConfig::default();
        let gate = WorkerGate::from_config(&fc);
        assert!(!gate.any());
    }

    #[test]
    fn master_on_respects_worker_flags() {
        let fc = FreshContextConfig {
            enabled: true,
            scheduler_enabled: true,
            dispatcher_enabled: false,
            ..FreshContextConfig::default()
        };
        let gate = WorkerGate::from_config(&fc);
        assert!(gate.scheduler);
        assert!(!gate.dispatcher);
        assert!(gate.any());
    }
}
