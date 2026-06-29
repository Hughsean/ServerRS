use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use tracing::warn;

use crate::domain::fresh_context::{
    FreshContextDistiller, FreshContextRepoT, FreshDistillInput, FreshDistilledItem, FreshItem,
    FreshItemDistillUpdate, FreshSource, fresh_status, risk_policy,
};
use crate::shared::config::FreshContextConfig;
use crate::shared::error::AppError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreshPipelineStats {
    pub expired_items: u64,
    pub fetched_seen: usize,
    pub distilled: usize,
    pub published: usize,
    pub rejected: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub struct FreshPipelineService {
    repo: Arc<dyn FreshContextRepoT>,
    distiller: Arc<dyn FreshContextDistiller>,
    config: FreshContextConfig,
}

impl FreshPipelineService {
    pub fn new(
        repo: Arc<dyn FreshContextRepoT>,
        distiller: Arc<dyn FreshContextDistiller>,
        config: FreshContextConfig,
    ) -> Self {
        Self {
            repo,
            distiller,
            config,
        }
    }

    pub async fn run_tick(&self) -> Result<FreshPipelineStats, AppError> {
        let now = Utc::now();
        let mut stats = FreshPipelineStats {
            expired_items: self.repo.expire_items(now).await?,
            ..FreshPipelineStats::default()
        };

        let items = self
            .repo
            .list_items_by_status(
                fresh_status::FETCHED,
                now,
                self.config.max_pipeline_items_per_tick as u64,
            )
            .await?;
        stats.fetched_seen = items.len();

        for item in items {
            match self.process_item(item, now).await {
                Ok(outcome) => stats.apply(outcome),
                Err(error) => {
                    stats.failed += 1;
                    warn!(error = %error, "Fresh Context pipeline item failed");
                }
            }
        }

        Ok(stats)
    }

    async fn process_item(
        &self,
        item: FreshItem,
        now: DateTime<Utc>,
    ) -> Result<FreshPipelineOutcome, AppError> {
        if item.expires_at <= now {
            let applied = self
                .repo
                .update_item_status_if_current(
                    item.id,
                    fresh_status::FETCHED,
                    fresh_status::EXPIRED,
                    Some(json!({
                        "pipeline": "fresh_context",
                        "reason": "expired_before_distill",
                        "expired_at": now.to_rfc3339(),
                    })),
                )
                .await?;
            return Ok(if applied {
                FreshPipelineOutcome::Skipped
            } else {
                FreshPipelineOutcome::NotApplied
            });
        }

        let Some(source) = self.repo.find_source_by_id(item.source_id).await? else {
            return self.reject_item(item.id, "source_not_found", None).await;
        };
        if !source_is_processable(&source) {
            return self
                .reject_item(item.id, "source_disabled_or_deleted", None)
                .await;
        }

        let Some(clean_text) = item
            .clean_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
        else {
            return self.reject_item(item.id, "missing_clean_text", None).await;
        };

        let input = FreshDistillInput {
            source_name: source.name.clone(),
            source_kind: source.source_kind.clone(),
            trust_level: source.trust_level.clone(),
            url: item.canonical_url.clone().or_else(|| item.url.clone()),
            title: item.title.clone(),
            clean_text,
            published_at: item.published_at,
            fetched_at: item.fetched_at,
        };
        let result = self.distiller.distill(&input).await?;
        let distilled = result.distilled;

        if !distilled.accept {
            return self
                .reject_item(item.id, "distiller_rejected", Some(&distilled))
                .await;
        }

        let publishable = should_publish(&source, &distilled);
        let update = build_distill_update(&item, &distilled, &source, publishable);
        let applied = self
            .repo
            .update_item_distill_result_if_current(
                item.id,
                fresh_status::FETCHED,
                fresh_status::DISTILLED,
                update,
            )
            .await?;
        if !applied {
            return Ok(FreshPipelineOutcome::NotApplied);
        }

        if publishable {
            let applied = self
                .repo
                .update_item_status_if_current(
                    item.id,
                    fresh_status::DISTILLED,
                    fresh_status::PUBLISHED,
                    Some(json!({
                        "pipeline": "fresh_context",
                        "published_by": "fresh_pipeline",
                        "distilled": distilled,
                    })),
                )
                .await?;
            if applied {
                Ok(FreshPipelineOutcome::Published)
            } else {
                Ok(FreshPipelineOutcome::Distilled)
            }
        } else {
            Ok(FreshPipelineOutcome::Distilled)
        }
    }

    async fn reject_item(
        &self,
        item_id: u64,
        reason: &str,
        distilled: Option<&FreshDistilledItem>,
    ) -> Result<FreshPipelineOutcome, AppError> {
        let applied = self
            .repo
            .update_item_status_if_current(
                item_id,
                fresh_status::FETCHED,
                fresh_status::REJECTED,
                Some(json!({
                    "pipeline": "fresh_context",
                    "reject_reason": reason,
                    "distilled": distilled,
                })),
            )
            .await?;
        Ok(if applied {
            FreshPipelineOutcome::Rejected
        } else {
            FreshPipelineOutcome::NotApplied
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreshPipelineOutcome {
    Distilled,
    Published,
    Rejected,
    Skipped,
    NotApplied,
}

impl FreshPipelineStats {
    fn apply(&mut self, outcome: FreshPipelineOutcome) {
        match outcome {
            FreshPipelineOutcome::Distilled => self.distilled += 1,
            FreshPipelineOutcome::Published => self.published += 1,
            FreshPipelineOutcome::Rejected => self.rejected += 1,
            FreshPipelineOutcome::Skipped | FreshPipelineOutcome::NotApplied => self.skipped += 1,
        }
    }
}

fn source_is_processable(source: &FreshSource) -> bool {
    source.enabled == 1 && source.deleted_at.is_none()
}

fn should_publish(source: &FreshSource, distilled: &FreshDistilledItem) -> bool {
    if !distilled.should_publish {
        return false;
    }
    if source.risk_policy == risk_policy::MANUAL_REVIEW {
        return false;
    }
    if source.risk_policy == risk_policy::STRICT && !distilled.risk_flags.is_empty() {
        return false;
    }
    !distilled
        .risk_flags
        .iter()
        .any(|flag| is_blocking_risk(flag))
}

fn is_blocking_risk(flag: &str) -> bool {
    matches!(
        flag,
        "privacy_sensitive"
            | "defamation_risk"
            | "minor_involved"
            | "medical_claim"
            | "legal_claim"
            | "financial_claim"
            | "self_harm_crisis"
            | "explicit_content"
            | "political_sensitive"
    )
}

fn build_distill_update(
    item: &FreshItem,
    distilled: &FreshDistilledItem,
    source: &FreshSource,
    publishable: bool,
) -> FreshItemDistillUpdate {
    FreshItemDistillUpdate {
        title: non_empty(distilled.title.as_str()).or_else(|| item.title.clone()),
        summary: non_empty(distilled.summary.as_str()),
        published_at: parsed_published_at(distilled).or(item.published_at),
        freshness_score: clamp_score(distilled.freshness_score),
        heat_score: clamp_score(distilled.heat_score),
        rumor_level: distilled.rumor_level.clone(),
        risk_flags: Some(json!(distilled.risk_flags)),
        metadata: Some(json!({
            "pipeline": "fresh_context",
            "distilled": distilled,
            "source": {
                "id": source.id,
                "name": source.name,
                "kind": source.source_kind,
                "trust_level": source.trust_level,
                "risk_policy": source.risk_policy,
            },
            "publishable": publishable,
            "collector_metadata": item.metadata,
        })),
    }
}

fn parsed_published_at(distilled: &FreshDistilledItem) -> Option<DateTime<Utc>> {
    let raw = distilled.published_at.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&Utc))
        .ok()
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn clamp_score(score: f64) -> f64 {
    if score.is_finite() {
        score.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use super::*;
    use crate::domain::fresh_context::{
        FreshChunk, FreshDistillResult, FreshTopic, FreshTopicEvidence, NewFreshChunk,
        NewFreshItem, NewFreshSource, NewFreshTopic, NewFreshTopicEvidence, rumor_level,
        source_kind,
    };

    #[test]
    fn blocking_risk_prevents_publish() {
        let mut item = accepted_distilled_item();
        item.risk_flags = vec!["privacy_sensitive".into()];
        assert!(!should_publish(&test_source(), &item));
    }

    #[tokio::test]
    async fn pipeline_publishes_accepted_items() {
        let repo = Arc::new(MockPipelineRepo::new(
            vec![test_fresh_item()],
            test_source(),
        ));
        let distiller = Arc::new(MockDistiller::new(accepted_distilled_item()));
        let pipeline =
            FreshPipelineService::new(repo.clone(), distiller, FreshContextConfig::default());

        let stats = pipeline.run_tick().await.unwrap();
        assert_eq!(stats.fetched_seen, 1);
        assert_eq!(stats.published, 1);
        assert_eq!(repo.item_status(1).await, fresh_status::PUBLISHED);
    }

    #[tokio::test]
    async fn pipeline_rejects_distiller_rejections() {
        let mut rejected = accepted_distilled_item();
        rejected.accept = false;
        rejected.should_publish = false;
        rejected.reject_reason = Some("no useful content".into());
        let repo = Arc::new(MockPipelineRepo::new(
            vec![test_fresh_item()],
            test_source(),
        ));
        let distiller = Arc::new(MockDistiller::new(rejected));
        let pipeline =
            FreshPipelineService::new(repo.clone(), distiller, FreshContextConfig::default());

        let stats = pipeline.run_tick().await.unwrap();
        assert_eq!(stats.rejected, 1);
        assert_eq!(repo.item_status(1).await, fresh_status::REJECTED);
    }

    fn accepted_distilled_item() -> FreshDistilledItem {
        FreshDistilledItem {
            accept: true,
            reject_reason: None,
            title: "新鲜标题".into(),
            language: "zh".into(),
            content_type: "news".into(),
            summary: "这是一条新鲜上下文摘要。".into(),
            claims: Vec::new(),
            entities: Vec::new(),
            keywords: vec!["关键词".into()],
            published_at: Some("2026-06-28T10:00:00Z".into()),
            topic_key_hint: "fresh-topic".into(),
            rumor_level: rumor_level::REPORTED.into(),
            risk_flags: Vec::new(),
            freshness_score: 0.8,
            heat_score: 0.4,
            ttl_hint: "news".into(),
            should_publish: true,
        }
    }

    fn test_source() -> FreshSource {
        let now = Utc::now();
        FreshSource {
            id: 1,
            name: "测试源".into(),
            source_kind: source_kind::RSS.into(),
            base_url: Some("https://example.com/rss.xml".into()),
            allowed_domains: None,
            trust_level: "normal".into(),
            reliability_score: 0.8,
            crawl_interval_secs: 1800,
            default_ttl_secs: 86_400,
            risk_policy: risk_policy::NORMAL.into(),
            enabled: 1,
            metadata: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn test_fresh_item() -> FreshItem {
        let now = Utc::now();
        FreshItem {
            id: 1,
            source_id: 1,
            url: Some("https://example.com/a".into()),
            canonical_url: Some("https://example.com/a".into()),
            url_hash: Some("hash".into()),
            title: Some("原始标题".into()),
            raw_text: Some("原文".into()),
            clean_text: Some("这是一段足够长的新闻正文，用来测试 Fresh Pipeline。".into()),
            summary: None,
            published_at: None,
            fetched_at: now,
            expires_at: now + chrono::Duration::hours(1),
            content_hash: "content_hash".into(),
            status: fresh_status::FETCHED.into(),
            reliability_score: 0.8,
            freshness_score: 0.5,
            heat_score: 0.0,
            rumor_level: rumor_level::REPORTED.into(),
            risk_flags: None,
            metadata: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    struct MockDistiller {
        result: FreshDistilledItem,
    }

    impl MockDistiller {
        fn new(result: FreshDistilledItem) -> Self {
            Self { result }
        }
    }

    #[async_trait]
    impl FreshContextDistiller for MockDistiller {
        async fn distill(
            &self,
            _input: &FreshDistillInput,
        ) -> Result<FreshDistillResult, AppError> {
            Ok(FreshDistillResult {
                distilled: self.result.clone(),
                llm_input_tokens: Some(10),
                llm_output_tokens: Some(20),
            })
        }
    }

    struct MockPipelineRepo {
        items: Mutex<Vec<FreshItem>>,
        source: FreshSource,
    }

    impl MockPipelineRepo {
        fn new(items: Vec<FreshItem>, source: FreshSource) -> Self {
            Self {
                items: Mutex::new(items),
                source,
            }
        }

        async fn item_status(&self, item_id: u64) -> String {
            self.items
                .lock()
                .await
                .iter()
                .find(|item| item.id == item_id)
                .map(|item| item.status.clone())
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl FreshContextRepoT for MockPipelineRepo {
        async fn insert_source(&self, _source: NewFreshSource) -> Result<FreshSource, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn list_enabled_sources(&self, _limit: u64) -> Result<Vec<FreshSource>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn find_source_by_id(&self, source_id: u64) -> Result<Option<FreshSource>, AppError> {
            Ok((self.source.id == source_id).then(|| self.source.clone()))
        }

        async fn insert_item(&self, _item: NewFreshItem) -> Result<FreshItem, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn find_item_by_source_content(
            &self,
            _source_id: u64,
            _content_hash: &str,
        ) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn find_item_by_id(&self, _item_id: u64) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn list_active_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn list_chunkable_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn list_items_by_status(
            &self,
            status: &str,
            now: DateTime<Utc>,
            limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            Ok(self
                .items
                .lock()
                .await
                .iter()
                .filter(|item| item.status == status && item.expires_at > now)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn expire_items(&self, now: DateTime<Utc>) -> Result<u64, AppError> {
            let mut items = self.items.lock().await;
            let mut count = 0;
            for item in &mut *items {
                if item.expires_at <= now && item.status != fresh_status::EXPIRED {
                    item.status = fresh_status::EXPIRED.into();
                    count += 1;
                }
            }
            Ok(count)
        }

        async fn update_item_status_if_current(
            &self,
            item_id: u64,
            expected_status: &str,
            new_status: &str,
            metadata: Option<serde_json::Value>,
        ) -> Result<bool, AppError> {
            let mut items = self.items.lock().await;
            let Some(item) = items
                .iter_mut()
                .find(|item| item.id == item_id && item.status == expected_status)
            else {
                return Ok(false);
            };
            item.status = new_status.into();
            item.metadata = metadata;
            Ok(true)
        }

        async fn update_item_distill_result_if_current(
            &self,
            item_id: u64,
            expected_status: &str,
            new_status: &str,
            update: FreshItemDistillUpdate,
        ) -> Result<bool, AppError> {
            let mut items = self.items.lock().await;
            let Some(item) = items
                .iter_mut()
                .find(|item| item.id == item_id && item.status == expected_status)
            else {
                return Ok(false);
            };
            item.status = new_status.into();
            item.title = update.title;
            item.summary = update.summary;
            item.published_at = update.published_at;
            item.freshness_score = update.freshness_score;
            item.heat_score = update.heat_score;
            item.rumor_level = update.rumor_level;
            item.risk_flags = update.risk_flags;
            item.metadata = update.metadata;
            Ok(true)
        }

        async fn insert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn upsert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn find_topic_by_key(
            &self,
            _topic_key: &str,
        ) -> Result<Option<FreshTopic>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn link_topic_evidence(
            &self,
            _evidence: NewFreshTopicEvidence,
        ) -> Result<FreshTopicEvidence, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn assign_topic_to_item_chunks(
            &self,
            _item_id: u64,
            _topic_id: u64,
        ) -> Result<u64, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn insert_chunks(
            &self,
            _chunks: &[NewFreshChunk],
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn find_chunk_by_id(&self, _chunk_id: u64) -> Result<Option<FreshChunk>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn find_chunks_by_item(&self, _item_id: u64) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn list_indexable_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn mark_chunk_indexed(
            &self,
            _chunk_id: u64,
            _vector_id: String,
            _embedding_provider: String,
            _embedding_model: String,
            _embedding_dimension: u32,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn list_expired_indexed_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by pipeline tests")
        }

        async fn mark_chunk_vector_deleted(
            &self,
            _chunk_id: u64,
            _vector_id: &str,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by pipeline tests")
        }
    }
}
