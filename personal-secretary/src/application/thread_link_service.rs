use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    ClaimedThreadLinkBatch, ContentSegment, InboundEventStoreError, MediaKind, SourceAccountRef,
    SourceEventId, ThreadLinkCandidateCursor, ThreadLinkCandidateId, ThreadLinkCandidateStatus,
    ThreadLinkCandidateView, ThreadLinkError, ThreadLinkHint, ThreadLinkLeaseToken,
    ThreadLinkReviewAction, ThreadLinkReviewContext, ThreadLinkReviewId, ThreadLinkSignalKind,
    ValidatedThreadLinkReview, validate_thread_link_review,
};

#[async_trait]
pub trait ThreadLinkStoreT: Send + Sync {
    async fn claim_link_batch(
        &self,
        max_events: u32,
        max_total_chars: u32,
        lease_secs: u64,
    ) -> Result<Option<ClaimedThreadLinkBatch>, InboundEventStoreError>;

    /// 必须原子写入提示、产生 proposed 候选和完成租约；不得改写线程归属。
    async fn commit_link_hints(
        &self,
        lease_token: &ThreadLinkLeaseToken,
        hints: &[ThreadLinkHint],
    ) -> Result<usize, InboundEventStoreError>;

    async fn fail_link_batch(
        &self,
        lease_token: &ThreadLinkLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError>;

    async fn list_link_candidates(
        &self,
        account: &SourceAccountRef,
        cursor: Option<&ThreadLinkCandidateCursor>,
        limit: u32,
    ) -> Result<Vec<ThreadLinkCandidateView>, InboundEventStoreError>;

    /// 只返回仍处于 `proposed` 的待确认候选。适配器应在存储查询中完成过滤，
    /// 避免大量历史审核行遮蔽新的待确认项。
    async fn list_pending_link_candidates(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<ThreadLinkCandidateView>, InboundEventStoreError> {
        Ok(self
            .list_link_candidates(account, None, limit)
            .await?
            .into_iter()
            .filter(|view| view.status == ThreadLinkCandidateStatus::Proposed)
            .collect())
    }

    async fn load_link_review_context(
        &self,
        candidate_id: &ThreadLinkCandidateId,
        command_source_event_id: &SourceEventId,
    ) -> Result<ThreadLinkReviewContext, InboundEventStoreError>;

    async fn commit_link_review(
        &self,
        review: &ValidatedThreadLinkReview,
    ) -> Result<ThreadLinkReviewReceipt, InboundEventStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLinkRun {
    pub events_read: usize,
    pub hints_created: usize,
    pub candidates_created: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLinkReviewReceipt {
    pub review_id: ThreadLinkReviewId,
    pub candidate_id: ThreadLinkCandidateId,
    pub status: ThreadLinkCandidateStatus,
    pub changed: bool,
}

pub struct ConservativeThreadLinkExtractor;

impl ConservativeThreadLinkExtractor {
    pub fn extract(&self, batch: &ClaimedThreadLinkBatch) -> Vec<ThreadLinkHint> {
        let mut hints = Vec::new();
        for event in &batch.events {
            if event.content_omitted {
                continue;
            }
            for value in explicit_project_ids(&event.normalized_text) {
                hints.push(make_hint(
                    event,
                    ThreadLinkSignalKind::ExplicitProjectId,
                    value,
                    HintValueNormalization::AsciiCaseInsensitive,
                ));
            }
            for segment in &event.segments {
                match segment {
                    ContentSegment::Media {
                        kind: MediaKind::File,
                        source_key,
                        ..
                    } => hints.push(make_hint(
                        event,
                        ThreadLinkSignalKind::ExactFileSourceKey,
                        source_key,
                        // Preserve the already-deployed file-key fingerprint contract.
                        HintValueNormalization::AsciiCaseInsensitive,
                    )),
                    ContentSegment::FileVersionReference {
                        current_source_key,
                        previous_source_key,
                    } => {
                        hints.push(make_hint(
                            event,
                            ThreadLinkSignalKind::ExactFileSourceKey,
                            current_source_key,
                            HintValueNormalization::AsciiCaseInsensitive,
                        ));
                        hints.push(make_hint(
                            event,
                            ThreadLinkSignalKind::ExplicitFileVersion,
                            previous_source_key,
                            HintValueNormalization::AsciiCaseInsensitive,
                        ));
                    }
                    ContentSegment::Forward { source_key } => hints.push(make_hint(
                        event,
                        ThreadLinkSignalKind::ExactForwardSourceKey,
                        source_key,
                        HintValueNormalization::Exact,
                    )),
                    ContentSegment::Rich { source_key, .. }
                        if is_content_digest_reference(source_key) =>
                    {
                        hints.push(make_hint(
                            event,
                            ThreadLinkSignalKind::ExactRichContentKey,
                            source_key,
                            HintValueNormalization::Exact,
                        ));
                    }
                    _ => {}
                }
            }
        }
        hints.sort_by(|left, right| {
            left.source_event_id
                .as_str()
                .cmp(right.source_event_id.as_str())
                .then(left.kind.as_str().cmp(right.kind.as_str()))
                .then(left.fingerprint_sha256.cmp(&right.fingerprint_sha256))
        });
        hints.dedup_by(|left, right| {
            left.source_event_id == right.source_event_id
                && left.kind == right.kind
                && left.fingerprint_sha256 == right.fingerprint_sha256
        });
        hints
    }
}

fn explicit_project_ids(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        [
            "项目ID:",
            "项目ID：",
            "项目编号:",
            "项目编号：",
            "project_id:",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 191
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    })
}

fn make_hint(
    event: &crate::ThreadLinkEvent,
    kind: ThreadLinkSignalKind,
    raw_value: &str,
    normalization: HintValueNormalization,
) -> ThreadLinkHint {
    let trimmed = raw_value.trim();
    let normalized;
    let value = match normalization {
        HintValueNormalization::Exact => trimmed,
        HintValueNormalization::AsciiCaseInsensitive => {
            normalized = trimmed.to_ascii_lowercase();
            normalized.as_str()
        }
    };
    let digest = Sha256::digest(value.as_bytes());
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut fingerprint_sha256 = String::with_capacity(64);
    for byte in digest {
        fingerprint_sha256.push(HEX[(byte >> 4) as usize] as char);
        fingerprint_sha256.push(HEX[(byte & 0x0f) as usize] as char);
    }
    ThreadLinkHint {
        source_event_id: event.source_event_id.clone(),
        account: event.account.clone(),
        conversation: event.conversation.clone(),
        thread_id: event.thread_id.clone(),
        kind,
        fingerprint_sha256,
    }
}

#[derive(Debug, Clone, Copy)]
enum HintValueNormalization {
    Exact,
    AsciiCaseInsensitive,
}

fn is_content_digest_reference(value: &str) -> bool {
    let Some(digest) = value.strip_prefix("sha256:") else {
        return false;
    };
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub struct ThreadLinkUseCase {
    store: Arc<dyn ThreadLinkStoreT>,
    extractor: ConservativeThreadLinkExtractor,
    max_events: u32,
    max_total_chars: u32,
    lease_secs: u64,
}

impl ThreadLinkUseCase {
    pub fn new(
        store: Arc<dyn ThreadLinkStoreT>,
        max_events: u32,
        max_total_chars: u32,
        lease_secs: u64,
    ) -> Result<Self, ThreadLinkUseCaseError> {
        if max_events == 0 || max_total_chars == 0 || lease_secs == 0 {
            return Err(ThreadLinkUseCaseError::InvalidConfiguration(
                "link scan budgets and lease must be positive".into(),
            ));
        }
        Ok(Self {
            store,
            extractor: ConservativeThreadLinkExtractor,
            max_events,
            max_total_chars,
            lease_secs,
        })
    }

    pub async fn run_once(&self) -> Result<Option<ThreadLinkRun>, ThreadLinkUseCaseError> {
        let Some(batch) = self
            .store
            .claim_link_batch(self.max_events, self.max_total_chars, self.lease_secs)
            .await?
        else {
            return Ok(None);
        };
        let hints = self.extractor.extract(&batch);
        match self
            .store
            .commit_link_hints(&batch.lease_token, &hints)
            .await
        {
            Ok(candidates_created) => Ok(Some(ThreadLinkRun {
                events_read: batch.events.len(),
                hints_created: hints.len(),
                candidates_created,
            })),
            Err(error) => {
                let _ = self
                    .store
                    .fail_link_batch(&batch.lease_token, &error.to_string())
                    .await;
                Err(error.into())
            }
        }
    }
}

pub struct ThreadLinkReviewUseCase {
    store: Arc<dyn ThreadLinkStoreT>,
}

impl ThreadLinkReviewUseCase {
    pub fn new(store: Arc<dyn ThreadLinkStoreT>) -> Self {
        Self { store }
    }

    pub async fn list(
        &self,
        account: &SourceAccountRef,
        cursor: Option<&ThreadLinkCandidateCursor>,
        limit: u32,
    ) -> Result<Vec<ThreadLinkCandidateView>, ThreadLinkUseCaseError> {
        if limit == 0 || limit > 100 {
            return Err(ThreadLinkUseCaseError::InvalidConfiguration(
                "thread link review page limit must be between 1 and 100".into(),
            ));
        }
        Ok(self
            .store
            .list_link_candidates(account, cursor, limit)
            .await?)
    }

    /// 有界列出仍待 Owner 确认的候选。生产适配器在数据库查询中直接过滤
    /// `proposed`，避免历史审核行遮蔽新的待确认项。
    pub async fn list_pending(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<ThreadLinkCandidateView>, ThreadLinkUseCaseError> {
        if limit == 0 || limit > 20 {
            return Err(ThreadLinkUseCaseError::InvalidConfiguration(
                "pending thread link candidate limit must be between 1 and 20".into(),
            ));
        }
        Ok(self
            .store
            .list_pending_link_candidates(account, limit)
            .await?)
    }

    pub async fn review(
        &self,
        candidate_id: &ThreadLinkCandidateId,
        command_source_event_id: &SourceEventId,
        action: ThreadLinkReviewAction,
    ) -> Result<ThreadLinkReviewReceipt, ThreadLinkUseCaseError> {
        let context = self
            .store
            .load_link_review_context(candidate_id, command_source_event_id)
            .await?;
        let review = validate_thread_link_review(&context, action)?;
        let receipt = self.store.commit_link_review(&review).await?;
        tracing::debug!(
            candidate_id = %candidate_id.as_str(),
            command_source_event_id = %command_source_event_id.as_str(),
            action = action.as_str(),
            changed = receipt.changed,
            "Owner 线程关联候选审核已提交"
        );
        Ok(receipt)
    }
}

#[derive(Debug, Error)]
pub enum ThreadLinkUseCaseError {
    #[error("invalid thread link configuration: {0}")]
    InvalidConfiguration(String),
    #[error(transparent)]
    Link(#[from] ThreadLinkError),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConversationKind, ConversationRef, EventThreadId, MessageSource, SourceAccountRef,
        SourceEventId, ThreadLinkEvent, ThreadLinkLeaseToken,
    };

    fn batch(text: &str, segments: Vec<ContentSegment>) -> ClaimedThreadLinkBatch {
        ClaimedThreadLinkBatch {
            lease_token: ThreadLinkLeaseToken::new("lease").unwrap(),
            events: vec![ThreadLinkEvent {
                source_event_id: SourceEventId::new("event").unwrap(),
                account: SourceAccountRef::new(MessageSource::NapCat, "account").unwrap(),
                conversation: ConversationRef::new(ConversationKind::Group, "group").unwrap(),
                thread_id: EventThreadId::new("thread").unwrap(),
                normalized_text: text.into(),
                segments,
                content_omitted: false,
            }],
        }
    }

    #[test]
    fn explicit_and_structured_reference_signals_are_extracted() {
        let batch = batch(
            "项目ID:PAYMENT_V2\n讨论支付项目",
            vec![
                ContentSegment::Media {
                    kind: MediaKind::File,
                    source_key: "exact-content-key".into(),
                    source_url: None,
                    display_name: Some("同名报价单.pdf".into()),
                },
                ContentSegment::FileVersionReference {
                    current_source_key: "current-file-key".into(),
                    previous_source_key: "previous-file-key".into(),
                },
                ContentSegment::Forward {
                    source_key: "forward-reference".into(),
                },
                ContentSegment::Rich {
                    kind: crate::RichContentKind::Json,
                    source_key: format!("sha256:{}", "a".repeat(64)),
                    summary: None,
                },
            ],
        );
        let hints = ConservativeThreadLinkExtractor.extract(&batch);
        assert_eq!(hints.len(), 6);
        assert!(hints.iter().all(|hint| hint.kind.is_strong()));
        assert!(hints.iter().all(|hint| hint.fingerprint_sha256.len() == 64));
        assert!(
            hints
                .iter()
                .any(|hint| hint.kind == ThreadLinkSignalKind::ExplicitFileVersion)
        );
        assert!(
            hints
                .iter()
                .any(|hint| hint.kind == ThreadLinkSignalKind::ExactForwardSourceKey)
        );
        assert!(
            hints
                .iter()
                .any(|hint| hint.kind == ThreadLinkSignalKind::ExactRichContentKey)
        );
    }

    #[test]
    fn similar_topic_and_filename_without_source_key_do_not_become_hints() {
        let batch = batch("还是之前的支付项目", Vec::new());
        assert!(ConservativeThreadLinkExtractor.extract(&batch).is_empty());
    }

    #[test]
    fn legacy_rich_kind_sentinels_are_not_strong_references() {
        let batch = batch(
            "",
            vec![ContentSegment::Rich {
                kind: crate::RichContentKind::Json,
                source_key: "rich_json".into(),
                summary: Some("相同摘要不能成为证据".into()),
            }],
        );
        assert!(ConservativeThreadLinkExtractor.extract(&batch).is_empty());
    }

    #[test]
    fn exact_forward_keys_remain_case_sensitive() {
        let upper = batch(
            "",
            vec![ContentSegment::Forward {
                source_key: "Forward-Aa".into(),
            }],
        );
        let lower = batch(
            "",
            vec![ContentSegment::Forward {
                source_key: "forward-aa".into(),
            }],
        );
        assert_ne!(
            ConservativeThreadLinkExtractor.extract(&upper)[0].fingerprint_sha256,
            ConservativeThreadLinkExtractor.extract(&lower)[0].fingerprint_sha256
        );
    }
}
