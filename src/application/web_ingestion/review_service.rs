use std::sync::Arc;

use crate::application::web_ingestion::event_types::{aggregate, event as ev};
use crate::application::web_ingestion::hash;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::review::{
    KnowledgeReviewDetail, KnowledgeReviewFilter, KnowledgeReviewPage, KnowledgeReviewRepository,
    NewReviewPublishRequest, ReviewPublishRequest,
};
use crate::domain::web_ingestion::status::publish_status;
use crate::shared::error::AppError;

pub struct KnowledgeReviewService {
    repository: Arc<dyn KnowledgeReviewRepository>,
    publish_dispatcher_enabled: bool,
}

impl KnowledgeReviewService {
    pub fn new(
        repository: Arc<dyn KnowledgeReviewRepository>,
        publish_dispatcher_enabled: bool,
    ) -> Self {
        Self {
            repository,
            publish_dispatcher_enabled,
        }
    }

    pub async fn list(
        &self,
        publish_status: Option<&str>,
        source_id: Option<u64>,
        page: u64,
        page_size: u64,
    ) -> Result<KnowledgeReviewPage, AppError> {
        let status = normalize_publish_status(publish_status)?;
        self.repository
            .list(KnowledgeReviewFilter {
                publish_status: status,
                source_id,
                page: page.max(1),
                page_size: page_size.clamp(1, 100),
            })
            .await
            .map_err(map_review_error)
    }

    pub async fn get(&self, publish_record_id: u64) -> Result<KnowledgeReviewDetail, AppError> {
        self.repository
            .find_detail_by_id(publish_record_id)
            .await
            .map_err(map_review_error)?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "knowledge publish record {publish_record_id} not found"
                ))
            })
    }

    pub async fn request_publish(
        &self,
        publish_record_id: u64,
        reviewer_user_id: u64,
        reviewer_username: String,
        notes: Option<String>,
    ) -> Result<ReviewPublishRequest, AppError> {
        if !self.publish_dispatcher_enabled {
            return Err(AppError::Conflict(
                "web ingestion dispatcher is disabled; publishing cannot be processed".into(),
            ));
        }

        let item = self
            .repository
            .find_item_by_id(publish_record_id)
            .await
            .map_err(map_review_error)?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "knowledge publish record {publish_record_id} not found"
                ))
            })?;
        if item.publish_status != publish_status::STAGED || item.active {
            return Err(AppError::Conflict(format!(
                "publish record {publish_record_id} is not awaiting review"
            )));
        }

        let notes = normalize_notes(notes)?;
        let event_key = hash::event_key(
            ev::KNOWLEDGE_PUBLISH_REQUESTED,
            aggregate::KNOWLEDGE_PUBLISH_RECORD,
            publish_record_id,
            item.run_id,
            &item.version_key,
        );

        self.repository
            .request_publish(NewReviewPublishRequest {
                publish_record_id,
                event_key,
                reviewer_user_id,
                reviewer_username,
                notes,
            })
            .await
            .map_err(map_review_error)
    }
}

fn normalize_publish_status(value: Option<&str>) -> Result<String, AppError> {
    let status = value.unwrap_or(publish_status::STAGED).trim();
    match status {
        "all"
        | publish_status::STAGED
        | publish_status::PUBLISHED
        | publish_status::SUPERSEDED
        | publish_status::ROLLED_BACK
        | publish_status::FAILED => Ok(status.to_string()),
        _ => Err(AppError::Validation(format!(
            "unsupported publish status '{status}'"
        ))),
    }
}

fn normalize_notes(notes: Option<String>) -> Result<Option<String>, AppError> {
    let notes = notes.map(|value| value.trim().to_string());
    let notes = notes.filter(|value| !value.is_empty());
    if notes
        .as_ref()
        .is_some_and(|value| value.chars().count() > 2_000)
    {
        return Err(AppError::Validation(
            "review notes must not exceed 2000 characters".into(),
        ));
    }
    Ok(notes)
}

fn map_review_error(error: WebIngestionError) -> AppError {
    match error {
        WebIngestionError::NotFound { entity, id } => {
            AppError::NotFound(format!("{entity} {id} not found"))
        }
        WebIngestionError::ReviewConflict { reason } => AppError::Conflict(reason),
        other => AppError::internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Mutex;

    use crate::domain::web_ingestion::review::{
        KnowledgeReviewAuditEntry, KnowledgeReviewDetail, KnowledgeReviewItem,
    };

    struct MockReviewRepository {
        detail: KnowledgeReviewDetail,
        requested: Mutex<Option<NewReviewPublishRequest>>,
    }

    #[async_trait]
    impl KnowledgeReviewRepository for MockReviewRepository {
        async fn list(
            &self,
            filter: KnowledgeReviewFilter,
        ) -> Result<KnowledgeReviewPage, WebIngestionError> {
            Ok(KnowledgeReviewPage {
                items: vec![self.detail.review.clone()],
                page: filter.page,
                page_size: filter.page_size,
                total: 1,
            })
        }

        async fn find_item_by_id(
            &self,
            publish_record_id: u64,
        ) -> Result<Option<KnowledgeReviewItem>, WebIngestionError> {
            Ok((publish_record_id == self.detail.review.publish_record_id)
                .then(|| self.detail.review.clone()))
        }

        async fn find_detail_by_id(
            &self,
            publish_record_id: u64,
        ) -> Result<Option<KnowledgeReviewDetail>, WebIngestionError> {
            Ok((publish_record_id == self.detail.review.publish_record_id)
                .then(|| self.detail.clone()))
        }

        async fn request_publish(
            &self,
            request: NewReviewPublishRequest,
        ) -> Result<ReviewPublishRequest, WebIngestionError> {
            *self.requested.lock().unwrap() = Some(request.clone());
            Ok(ReviewPublishRequest {
                publish_record_id: request.publish_record_id,
                event_id: 9,
                event_status: "pending".into(),
                already_requested: false,
            })
        }
    }

    fn review_detail() -> KnowledgeReviewDetail {
        KnowledgeReviewDetail {
            review: KnowledgeReviewItem {
                publish_record_id: 7,
                source_id: 2,
                source_name: "test-source".into(),
                page_id: 3,
                run_id: 5,
                document_id: 6,
                version_key: "version-key".into(),
                title: Some("title".into()),
                source_url: "https://example.com/article".into(),
                publish_status: publish_status::STAGED.into(),
                active: false,
                run_status: "staged".into(),
                run_stage: "staging".into(),
                quality_score: Some(0.9),
                quality_result: None,
                risk_flags: None,
                should_publish: Some(false),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            clean_text: Some("review text".into()),
            distilled_json: None,
            audit_logs: Vec::<KnowledgeReviewAuditEntry>::new(),
        }
    }

    #[test]
    fn status_filter_is_restricted() {
        assert_eq!(
            normalize_publish_status(None).unwrap(),
            publish_status::STAGED
        );
        assert_eq!(normalize_publish_status(Some("all")).unwrap(), "all");
        assert!(normalize_publish_status(Some("pending")).is_err());
    }

    #[test]
    fn review_notes_are_trimmed_and_bounded() {
        assert_eq!(
            normalize_notes(Some("  reviewed  ".into())).unwrap(),
            Some("reviewed".into())
        );
        assert_eq!(normalize_notes(Some(" ".into())).unwrap(), None);
        assert!(normalize_notes(Some("x".repeat(2_001))).is_err());
    }

    #[tokio::test]
    async fn publish_request_is_idempotently_keyed_and_records_reviewer() {
        let repository = Arc::new(MockReviewRepository {
            detail: review_detail(),
            requested: Mutex::new(None),
        });
        let service = KnowledgeReviewService::new(repository.clone(), true);

        let result = service
            .request_publish(7, 11, "admin".into(), Some(" checked ".into()))
            .await
            .unwrap();

        assert_eq!(result.event_status, "pending");
        let request = repository.requested.lock().unwrap().clone().unwrap();
        assert_eq!(request.reviewer_user_id, 11);
        assert_eq!(request.notes.as_deref(), Some("checked"));
        assert_eq!(
            request.event_key,
            hash::event_key(
                ev::KNOWLEDGE_PUBLISH_REQUESTED,
                aggregate::KNOWLEDGE_PUBLISH_RECORD,
                7,
                5,
                "version-key"
            )
        );
    }

    #[tokio::test]
    async fn publish_request_requires_dispatcher() {
        let repository = Arc::new(MockReviewRepository {
            detail: review_detail(),
            requested: Mutex::new(None),
        });
        let service = KnowledgeReviewService::new(repository.clone(), false);

        assert!(
            matches!(
                service.request_publish(7, 11, "admin".into(), None).await,
                Err(AppError::Conflict(_))
            ),
            "disabled dispatcher must reject publish requests"
        );
        assert!(repository.requested.lock().unwrap().is_none());
    }
}
