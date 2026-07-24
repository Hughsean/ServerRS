use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ClaimKind, ClaimedThreadSemanticBatch, InboundEventStoreError, OpenQuestionCandidate,
    OpenQuestionId, ThreadClaimCandidate, ThreadClaimId, ThreadDecisionCandidate, ThreadDecisionId,
    ThreadSemanticError, ThreadSemanticLeaseToken, ThreadSemanticPatch, validate_semantic_patch,
};

#[async_trait]
pub trait ThreadSemanticExtractorT: Send + Sync {
    /// 一次处理一个线程的有界事件批次。实现可以是规则、LLM 或人工控制面，但不得直接写库。
    async fn extract(
        &self,
        batch: &ClaimedThreadSemanticBatch,
    ) -> Result<ThreadSemanticPatch, ThreadSemanticExtractorError>;
}

#[async_trait]
pub trait ThreadSemanticStoreT: Send + Sync {
    async fn claim_semantic_batch(
        &self,
        max_events: u32,
        max_total_chars: u32,
        lease_secs: u64,
    ) -> Result<Option<ClaimedThreadSemanticBatch>, InboundEventStoreError>;

    async fn commit_semantic_patch(
        &self,
        batch: &ClaimedThreadSemanticBatch,
        patch: &ThreadSemanticPatch,
    ) -> Result<(), InboundEventStoreError>;

    async fn release_semantic_claim(
        &self,
        lease_token: &ThreadSemanticLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSemanticRun {
    pub events_read: usize,
    pub claims_created: usize,
    pub decisions_created: usize,
    pub questions_created: usize,
    pub lifecycle_changed: bool,
}

pub struct ThreadSemanticUseCase {
    store: Arc<dyn ThreadSemanticStoreT>,
    extractor: Arc<dyn ThreadSemanticExtractorT>,
    max_events: u32,
    max_total_chars: u32,
    lease_secs: u64,
}

impl ThreadSemanticUseCase {
    pub fn new(
        store: Arc<dyn ThreadSemanticStoreT>,
        extractor: Arc<dyn ThreadSemanticExtractorT>,
        max_events: u32,
        max_total_chars: u32,
        lease_secs: u64,
    ) -> Result<Self, ThreadSemanticUseCaseError> {
        if max_events == 0 || max_events > 500 {
            return Err(ThreadSemanticUseCaseError::InvalidConfiguration(
                "max_events must be between 1 and 500".into(),
            ));
        }
        if max_total_chars == 0 || max_total_chars > 1_000_000 {
            return Err(ThreadSemanticUseCaseError::InvalidConfiguration(
                "max_total_chars must be between 1 and 1000000".into(),
            ));
        }
        if lease_secs == 0 || lease_secs > 3600 {
            return Err(ThreadSemanticUseCaseError::InvalidConfiguration(
                "lease_secs must be between 1 and 3600".into(),
            ));
        }
        Ok(Self {
            store,
            extractor,
            max_events,
            max_total_chars,
            lease_secs,
        })
    }

    pub async fn run_once(&self) -> Result<Option<ThreadSemanticRun>, ThreadSemanticUseCaseError> {
        let Some(batch) = self
            .store
            .claim_semantic_batch(self.max_events, self.max_total_chars, self.lease_secs)
            .await?
        else {
            return Ok(None);
        };
        let patch = match self.extractor.extract(&batch).await {
            Ok(patch) => patch,
            Err(error) => {
                let _ = self
                    .store
                    .release_semantic_claim(&batch.lease_token, &error.to_string())
                    .await;
                return Err(error.into());
            }
        };
        if let Err(error) = validate_semantic_patch(&batch, &patch) {
            let _ = self
                .store
                .release_semantic_claim(&batch.lease_token, &error.to_string())
                .await;
            return Err(error.into());
        }
        let result = ThreadSemanticRun {
            events_read: batch.events.len(),
            claims_created: patch.claims.len(),
            decisions_created: patch.decisions.len(),
            questions_created: patch.questions.len(),
            lifecycle_changed: patch.lifecycle_change.is_some(),
        };
        if let Err(error) = self.store.commit_semantic_patch(&batch, &patch).await {
            let _ = self
                .store
                .release_semantic_claim(&batch.lease_token, &error.to_string())
                .await;
            return Err(error.into());
        }
        Ok(Some(result))
    }
}

/// 保守的零模型提取器：仅识别明确前缀和问号，所有结果仍是 `proposed` 候选。
/// 它用于在没有模型配置时提供可审计的最低能力，不承担自由文本完整理解。
pub struct ConservativeThreadSemanticExtractor {
    max_event_chars: usize,
}

impl ConservativeThreadSemanticExtractor {
    pub fn new(max_event_chars: usize) -> Result<Self, ThreadSemanticUseCaseError> {
        if max_event_chars == 0 || max_event_chars > 100_000 {
            return Err(ThreadSemanticUseCaseError::InvalidConfiguration(
                "max_event_chars must be between 1 and 100000".into(),
            ));
        }
        Ok(Self { max_event_chars })
    }
}

#[async_trait]
impl ThreadSemanticExtractorT for ConservativeThreadSemanticExtractor {
    async fn extract(
        &self,
        batch: &ClaimedThreadSemanticBatch,
    ) -> Result<ThreadSemanticPatch, ThreadSemanticExtractorError> {
        let mut patch = ThreadSemanticPatch::default();
        for event in &batch.events {
            let text = event.normalized_text.trim();
            if event.content_omitted
                || text.is_empty()
                || text.chars().count() > self.max_event_chars
            {
                continue;
            }
            if !text.starts_with("请问")
                && let Some(statement) =
                    explicit_value(text, &["请求：", "请求:", "请", "麻烦", "务必", "需要你"])
            {
                patch.claims.push(ThreadClaimCandidate {
                    claim_id: ThreadClaimId::generate(),
                    thread_id: batch.thread_id.clone(),
                    kind: ClaimKind::Request,
                    claimant: event.actor.clone(),
                    statement: statement.into(),
                    confidence_bps: 9000,
                    source_event_ids: vec![event.source_event_id.clone()],
                });
            }
            if let Some(statement) =
                explicit_value(text, &["反对：", "反对:", "我反对", "不同意", "不赞成"])
            {
                patch.claims.push(ThreadClaimCandidate {
                    claim_id: ThreadClaimId::generate(),
                    thread_id: batch.thread_id.clone(),
                    kind: ClaimKind::Objection,
                    claimant: event.actor.clone(),
                    statement: statement.into(),
                    confidence_bps: 9000,
                    source_event_ids: vec![event.source_event_id.clone()],
                });
            }
            if let Some(statement) =
                explicit_value(text, &["确认：", "确认:", "已确认：", "已确认:"])
            {
                patch.claims.push(ThreadClaimCandidate {
                    claim_id: ThreadClaimId::generate(),
                    thread_id: batch.thread_id.clone(),
                    kind: ClaimKind::Confirmation,
                    claimant: event.actor.clone(),
                    statement: statement.into(),
                    confidence_bps: 9500,
                    source_event_ids: vec![event.source_event_id.clone()],
                });
            }
            if let Some(statement) = explicit_value(
                text,
                &[
                    "最终决定：",
                    "最终决定:",
                    "决定：",
                    "决定:",
                    "结论：",
                    "结论:",
                ],
            ) {
                patch.decisions.push(ThreadDecisionCandidate {
                    decision_id: ThreadDecisionId::generate(),
                    thread_id: batch.thread_id.clone(),
                    statement: statement.into(),
                    confidence_bps: 9500,
                    supersedes: None,
                    source_event_ids: vec![event.source_event_id.clone()],
                });
            }
            if text.ends_with('？') || text.ends_with('?') || text.starts_with("请问") {
                patch.questions.push(OpenQuestionCandidate {
                    question_id: OpenQuestionId::generate(),
                    thread_id: batch.thread_id.clone(),
                    question: text.into(),
                    raised_by: event.actor.clone(),
                    confidence_bps: 9000,
                    source_event_ids: vec![event.source_event_id.clone()],
                });
            }
        }
        Ok(patch)
    }
}

fn explicit_value<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|prefix| {
        text.strip_prefix(prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

#[derive(Debug, Error)]
pub enum ThreadSemanticExtractorError {
    #[error("thread semantic extractor failed: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum ThreadSemanticUseCaseError {
    #[error("invalid thread semantic configuration: {0}")]
    InvalidConfiguration(String),
    #[error(transparent)]
    Extractor(#[from] ThreadSemanticExtractorError),
    #[error(transparent)]
    Semantic(#[from] ThreadSemanticError),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EventThreadId, MessageRole, MessageSource, SourceAccountRef, SourceEventId, ThreadActorRef,
        ThreadSemanticCursor, ThreadSemanticEvent, ThreadSemanticLeaseToken, ThreadStatus,
    };

    fn batch(texts: &[&str]) -> ClaimedThreadSemanticBatch {
        let account = SourceAccountRef::new(MessageSource::NapCat, "account").unwrap();
        let events = texts
            .iter()
            .enumerate()
            .map(|(index, text)| ThreadSemanticEvent {
                source_event_id: SourceEventId::new(format!("event-{index}")).unwrap(),
                actor: ThreadActorRef {
                    account: account.clone(),
                    actor_id: format!("actor-{index}"),
                },
                role: MessageRole::ExternalObservation,
                occurred_at_unix_secs: index as i64,
                normalized_text: (*text).into(),
                content_omitted: false,
            })
            .collect();
        ClaimedThreadSemanticBatch {
            lease_token: ThreadSemanticLeaseToken::new("lease").unwrap(),
            thread_id: EventThreadId::new("thread").unwrap(),
            current_status: ThreadStatus::Open,
            confirmed_decision_ids: Vec::new(),
            open_question_ids: Vec::new(),
            events,
            next_cursor: ThreadSemanticCursor {
                added_at_unix_micros: 1,
                source_event_id: SourceEventId::new("event-0").unwrap(),
            },
        }
    }

    #[tokio::test]
    async fn conservative_extractor_returns_typed_candidates_with_sources() {
        let extractor = ConservativeThreadSemanticExtractor::new(2000).unwrap();
        let batch = batch(&[
            "请发送报价单",
            "我反对周一上线",
            "确认：采用第二版",
            "决定：周五发布",
            "什么时候给客户回复？",
            "请问谁负责验收？",
        ]);
        let patch = extractor.extract(&batch).await.unwrap();
        assert_eq!(patch.claims.len(), 3);
        assert_eq!(patch.decisions.len(), 1);
        assert_eq!(patch.questions.len(), 2);
        assert!(validate_semantic_patch(&batch, &patch).is_ok());
    }

    #[tokio::test]
    async fn ambiguous_chat_is_not_invented_as_a_fact() {
        let extractor = ConservativeThreadSemanticExtractor::new(2000).unwrap();
        let batch = batch(&["今天好像有点忙", "收到"]);
        let patch = extractor.extract(&batch).await.unwrap();
        assert_eq!(patch, ThreadSemanticPatch::default());
    }
}
