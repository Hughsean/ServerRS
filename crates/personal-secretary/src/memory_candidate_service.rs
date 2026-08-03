//! 记忆候选的端口与用例编排。
//!
//! - `MemoryCandidateExtractorT`：提取器端口（规则、LLM 或人工控制面），只生成候选；
//! - `MemoryCandidateStoreT`：持久化端口（领取批次、提交候选、游标/租约、失效、列表）；
//! - `MemoryCandidateUseCase`：Worker 单次扫描编排 + Owner 列表查询；
//! - `MemoryCandidateControlStoreT`/`MemoryCandidateControlUseCase`：Approve/Reject 的
//!   单事务 Effect 边界（复用 Owner 工作控制的授权与 Receipt 共享层）。

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ActionLeaseToken, ActionRunId, CommitmentMemory, CommitmentStatus, InboundEventStoreError,
    MemoryCandidate, MemoryCandidateBatch, MemoryCandidateEvent, MemoryCandidateId,
    MemoryCandidateKind, MemoryCandidateLeaseToken, MemoryCandidateStatus, MemoryCandidateView,
    MemoryPayload, PersonMemory, ProjectMemory, SecretaryAction, SecretaryActionProposal,
    SecretaryActionReceipt, SourceAccountRef, SourceEventId, candidate_fingerprint,
    validate_memory_candidate,
};

/// 每轮扫描允许失效的 proposed 候选上限（有界；其余留待下轮）。
pub const MAX_INVALIDATE_PER_SCAN: u32 = 500;

/// 单批最多提交的候选数（业务不变量，不可配置；超出部分丢弃并计数）。
pub const MAX_CANDIDATES_PER_BATCH: u32 = 20;

// ===== 提取器端口 =====

#[async_trait]
pub trait MemoryCandidateExtractorT: Send + Sync {
    /// 从一个有界事件批次提取候选。实现可以是规则、LLM 或人工控制面，
    /// 但不得直接写库；输出候选仍须通过 `validate_memory_candidate` 领域校验。
    /// 单个坏候选应被跳过并记录有界错误原因，不能阻塞同批其他合法候选。
    async fn extract(
        &self,
        batch: &MemoryCandidateBatch,
    ) -> Result<Vec<MemoryCandidate>, MemoryCandidateExtractorError>;
}

// ===== 存储端口 =====

#[async_trait]
pub trait MemoryCandidateStoreT: Send + Sync {
    /// 领取一个账号的有界事件批次。SQL 按内容信任策略过滤：
    /// normal 总是可提取；local_only 仅在 `allow_local_only`（已验证 loopback
    /// 端点）时进入；envelope_only/never_long_term 与已 Applied 撤回事件排除。
    /// 同时持有持久化游标与租约（lease token + 到期时间 + fencing）。
    async fn claim_candidate_batch(
        &self,
        account: &SourceAccountRef,
        max_events: u32,
        max_event_chars: u32,
        max_total_chars: u32,
        lease_secs: u64,
        allow_local_only: bool,
    ) -> Result<Option<MemoryCandidateBatch>, InboundEventStoreError>;

    /// 原子提交一批候选：同账号同 fingerprint 只保留第一条（INSERT IGNORE 去重），
    /// 随后推进游标并释放租约。返回实际新建候选数。
    async fn commit_candidates(
        &self,
        batch: &MemoryCandidateBatch,
        candidates: &[MemoryCandidate],
    ) -> Result<u64, InboundEventStoreError>;

    /// 释放租约并记录有界错误原因（批次失败时由用例调用）。
    async fn release_candidate_claim(
        &self,
        lease_token: &MemoryCandidateLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError>;

    /// 把来源已失效（撤回 tombstone 已 Applied，或会话/正文切换为
    /// envelope_only/never_long_term，或来源跨账号）的 proposed 候选置为
    /// invalidated，版本 +1。任一条来源失效即失效整条候选，避免候选永久
    /// 停留在 proposed 且每次审批必然失败。
    async fn invalidate_stale_proposed(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<u64, InboundEventStoreError>;

    /// 按账号列出候选（status/kind 可选过滤），返回有界视图。
    async fn list_candidates(
        &self,
        account: &SourceAccountRef,
        status: Option<MemoryCandidateStatus>,
        kind: Option<MemoryCandidateKind>,
        limit: u32,
    ) -> Result<Vec<MemoryCandidateView>, InboundEventStoreError>;
}

// ===== Worker 用例 =====

/// 单次扫描报告。
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryCandidateRun {
    pub events_read: usize,
    pub candidates_committed: u64,
    pub candidates_skipped: u64,
    pub candidates_invalidated: u64,
}

pub struct MemoryCandidateUseCase {
    store: Arc<dyn MemoryCandidateStoreT>,
    extractor: Arc<dyn MemoryCandidateExtractorT>,
    account: SourceAccountRef,
    max_events: u32,
    max_event_chars: u32,
    max_total_chars: u32,
    lease_secs: u64,
    allow_local_only: bool,
}

impl MemoryCandidateUseCase {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<dyn MemoryCandidateStoreT>,
        extractor: Arc<dyn MemoryCandidateExtractorT>,
        account: SourceAccountRef,
        max_events: u32,
        max_event_chars: u32,
        max_total_chars: u32,
        lease_secs: u64,
        allow_local_only: bool,
    ) -> Result<Self, MemoryCandidateUseCaseError> {
        if max_events == 0 || max_events > 100 {
            return Err(MemoryCandidateUseCaseError::InvalidConfiguration(
                "max_events must be between 1 and 100".into(),
            ));
        }
        if max_event_chars == 0 || max_event_chars > 4_000 {
            return Err(MemoryCandidateUseCaseError::InvalidConfiguration(
                "max_event_chars must be between 1 and 4000".into(),
            ));
        }
        if max_total_chars == 0 || max_total_chars > 16_000 {
            return Err(MemoryCandidateUseCaseError::InvalidConfiguration(
                "max_total_chars must be between 1 and 16000".into(),
            ));
        }
        if lease_secs == 0 || lease_secs > 3600 {
            return Err(MemoryCandidateUseCaseError::InvalidConfiguration(
                "lease_secs must be between 1 and 3600".into(),
            ));
        }
        Ok(Self {
            store,
            extractor,
            account,
            max_events,
            max_event_chars,
            max_total_chars,
            lease_secs,
            allow_local_only,
        })
    }

    /// 单次扫描：先失效来源失效的 proposed 候选，再领取批次、提取、校验并提交。
    pub async fn run_once(
        &self,
    ) -> Result<Option<MemoryCandidateRun>, MemoryCandidateUseCaseError> {
        let invalidated = self
            .store
            .invalidate_stale_proposed(&self.account, MAX_INVALIDATE_PER_SCAN)
            .await?;
        let Some(batch) = self
            .store
            .claim_candidate_batch(
                &self.account,
                self.max_events,
                self.max_event_chars,
                self.max_total_chars,
                self.lease_secs,
                self.allow_local_only,
            )
            .await?
        else {
            return Ok(None);
        };
        let events_read = batch.events.len();
        let candidates = match self.extractor.extract(&batch).await {
            Ok(candidates) => candidates,
            Err(error) => {
                let _ = self
                    .store
                    .release_candidate_claim(&batch.lease_token, &error.to_string())
                    .await;
                return Err(error.into());
            }
        };
        let mut valid = Vec::new();
        let mut skipped = 0u64;
        for candidate in candidates {
            match validate_memory_candidate(&candidate, &batch) {
                Ok(()) => {
                    if valid.len() >= MAX_CANDIDATES_PER_BATCH as usize {
                        skipped += 1;
                        tracing::warn!(
                            candidate_id = candidate.candidate_id.as_str(),
                            "记忆候选超过单批上限，已丢弃"
                        );
                    } else {
                        valid.push(candidate);
                    }
                }
                Err(error) => {
                    skipped += 1;
                    tracing::warn!(
                        candidate_id = candidate.candidate_id.as_str(),
                        error = %error,
                        "记忆候选未通过领域校验，已跳过并记录原因"
                    );
                }
            }
        }
        let committed = self.store.commit_candidates(&batch, &valid).await?;
        Ok(Some(MemoryCandidateRun {
            events_read,
            candidates_committed: committed,
            candidates_skipped: skipped,
            candidates_invalidated: invalidated,
        }))
    }

    /// Owner 列表查询（L0 只读，有界）。
    /// `account` 显式传入调用方（如 action_run 所属账号），保证租户隔离，
    /// 不依赖构造时绑定的提取账号（该账号仅用于提取 worker 游标）。
    pub async fn list(
        &self,
        account: &SourceAccountRef,
        status: Option<MemoryCandidateStatus>,
        kind: Option<MemoryCandidateKind>,
        limit: u32,
    ) -> Result<Vec<MemoryCandidateView>, InboundEventStoreError> {
        if !(1..=100).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "memory candidate list limit must be in 1..=100".into(),
            ));
        }
        self.store
            .list_candidates(account, status, kind, limit)
            .await
    }
}

// ===== 保守零模型提取器 =====

/// 保守的零模型提取器：只识别明确前缀，所有结果仍是 `proposed` 候选，
/// 用于在没有模型配置时提供可审计的最低能力。不承担自由文本完整理解。
pub struct ConservativeMemoryCandidateExtractor {
    max_event_chars: usize,
    extractor_version: String,
}

impl ConservativeMemoryCandidateExtractor {
    pub fn new(
        max_event_chars: usize,
        extractor_version: impl Into<String>,
    ) -> Result<Self, MemoryCandidateUseCaseError> {
        if max_event_chars == 0 || max_event_chars > 4_000 {
            return Err(MemoryCandidateUseCaseError::InvalidConfiguration(
                "max_event_chars must be between 1 and 4000".into(),
            ));
        }
        let extractor_version = extractor_version.into();
        if extractor_version.trim().is_empty() || extractor_version.len() > 32 {
            return Err(MemoryCandidateUseCaseError::InvalidConfiguration(
                "extractor_version must contain 1..=32 bytes".into(),
            ));
        }
        Ok(Self {
            max_event_chars,
            extractor_version,
        })
    }
}

#[async_trait]
impl MemoryCandidateExtractorT for ConservativeMemoryCandidateExtractor {
    async fn extract(
        &self,
        batch: &MemoryCandidateBatch,
    ) -> Result<Vec<MemoryCandidate>, MemoryCandidateExtractorError> {
        let mut candidates = Vec::new();
        for event in batch.events.iter().filter(|event| !event.content_omitted) {
            let text = event.normalized_text.trim();
            if text.is_empty() || text.chars().count() > self.max_event_chars {
                continue;
            }
            let sources = vec![candidate_source(event)];
            if let Some(relationship) = explicit_value(text, &["人物：", "人物:", "人："]) {
                let payload = MemoryPayload::Person(PersonMemory {
                    person: event.actor.clone(),
                    relationship: Some(relationship.into()),
                    responsibilities: Vec::new(),
                    communication_preferences: Vec::new(),
                });
                let subject_key = format!("person:{}", event.actor.actor_id);
                if let Some(candidate) = build_candidate(
                    batch,
                    subject_key,
                    payload,
                    &sources,
                    &self.extractor_version,
                ) {
                    candidates.push(candidate);
                }
            }
            if let Some(rest) = explicit_value(text, &["项目：", "项目:"]) {
                let (project_key, goal) = split_first_token(rest);
                if !project_key.is_empty() {
                    let payload = MemoryPayload::Project(ProjectMemory {
                        project_key: project_key.into(),
                        goal: goal.into(),
                        member_actor_ids: Vec::new(),
                        member_actor_refs: Vec::new(),
                        progress: None,
                        decision_ids: Vec::new(),
                        risks: Vec::new(),
                        blockers: Vec::new(),
                        artifact_refs: Vec::new(),
                    });
                    let subject_key = format!("project:{project_key}");
                    if let Some(candidate) = build_candidate(
                        batch,
                        subject_key,
                        payload,
                        &sources,
                        &self.extractor_version,
                    ) {
                        candidates.push(candidate);
                    }
                }
            }
            if let Some(rest) = explicit_value(text, &["承诺：", "承诺:", "答应："]) {
                // 兼容"承诺：给 X action"与"承诺：X action"两种常见表述。
                let rest = rest.strip_prefix("给").map(str::trim).unwrap_or(rest);
                let (beneficiary_id, action) = split_first_token(rest);
                if !action.is_empty()
                    && let Some(beneficiary) = batch
                        .events
                        .iter()
                        .find(|batch_event| batch_event.actor.actor_id == beneficiary_id)
                {
                    // 承诺双方事件都必须进入证据来源（身份-证据强绑定）：
                    // promisor 事件即当前事件，beneficiary 事件单独并入并去重。
                    let mut commitment_sources = vec![candidate_source(event)];
                    if beneficiary.source_event_id != event.source_event_id {
                        commitment_sources.push(candidate_source(beneficiary));
                    }
                    let payload = MemoryPayload::Commitment(CommitmentMemory {
                        promisor: event.actor.clone(),
                        beneficiary: beneficiary.actor.clone(),
                        action: action.into(),
                        // 不得从模糊时间强行生成时间戳；v1 保守提取不产出 due。
                        due_at_unix_secs: None,
                        status: CommitmentStatus::Proposed,
                        completion_source_event_id: None,
                    });
                    let subject_key = format!(
                        "commitment:{}:{}:{}",
                        event.actor.actor_id,
                        beneficiary.actor.actor_id,
                        action.chars().take(160).collect::<String>()
                    );
                    if let Some(candidate) = build_candidate(
                        batch,
                        subject_key,
                        payload,
                        &commitment_sources,
                        &self.extractor_version,
                    ) {
                        candidates.push(candidate);
                    }
                }
            }
        }
        Ok(candidates)
    }
}

/// 构造一个 proposed/version 1 候选；fingerprint 由领域函数稳定派生。
fn build_candidate(
    batch: &MemoryCandidateBatch,
    subject_key: String,
    payload: MemoryPayload,
    sources: &[crate::MemoryCandidateSource],
    extractor_version: &str,
) -> Option<MemoryCandidate> {
    if sources.is_empty() {
        return None;
    }
    let fingerprint = candidate_fingerprint(
        &batch.account,
        &payload,
        &subject_key,
        sources,
        extractor_version,
    );
    Some(MemoryCandidate {
        candidate_id: MemoryCandidateId::generate(),
        account: batch.account.clone(),
        subject_key,
        payload,
        status: MemoryCandidateStatus::Proposed,
        version: crate::MemoryCandidateVersion::new(crate::INITIAL_CANDIDATE_VERSION).ok()?,
        extractor_version: extractor_version.to_owned(),
        deterministic_fingerprint: fingerprint,
        sources: sources.to_vec(),
    })
}

fn candidate_source(event: &MemoryCandidateEvent) -> crate::MemoryCandidateSource {
    crate::MemoryCandidateSource {
        source_event_id: event.source_event_id.clone(),
        actor: event.actor.clone(),
        occurred_at_unix_secs: event.occurred_at_unix_secs,
        content_trust_level: event.content_trust_level,
    }
}

fn explicit_value<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|prefix| {
        text.strip_prefix(prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// 把首段（空白分隔）与其余部分拆开；其余部分不得为空。
fn split_first_token(text: &str) -> (&str, &str) {
    let text = text.trim();
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

// ===== Owner 控制端口与用例 =====

#[derive(Debug, Clone)]
pub struct MemoryCandidateControlEffectRequest {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub run_id: ActionRunId,
    pub lease_token: ActionLeaseToken,
    pub effect_id: String,
    pub proposal_id: String,
    pub proposal_json: String,
    pub action: SecretaryAction,
}

#[derive(Debug, Error)]
pub enum MemoryCandidateControlStoreError {
    #[error("memory candidate control is unauthorized")]
    Unauthorized,
    #[error("memory candidate control target or state is invalid: {0}")]
    InvalidData(String),
    #[error("memory candidate control lease was lost")]
    LeaseLost,
    #[error("memory candidate control database operation failed")]
    Database,
}

#[async_trait]
pub trait MemoryCandidateControlStoreT: Send + Sync {
    async fn apply_effect(
        &self,
        request: &MemoryCandidateControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, MemoryCandidateControlStoreError>;
}

pub struct MemoryCandidateControlUseCase {
    store: Arc<dyn MemoryCandidateControlStoreT>,
}

impl MemoryCandidateControlUseCase {
    pub fn new(store: Arc<dyn MemoryCandidateControlStoreT>) -> Self {
        Self { store }
    }

    pub async fn apply_effect(
        &self,
        request: &MemoryCandidateControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, MemoryCandidateControlStoreError> {
        if request.effect_id.trim().is_empty()
            || request.effect_id.len() > 255
            || request.proposal_id.trim().is_empty()
            || request.proposal_id.len() > 36
            || request.proposal_json.len() > 65_536
        {
            return Err(MemoryCandidateControlStoreError::InvalidData(
                "memory candidate control effect identifiers or proposal are invalid".into(),
            ));
        }
        let proposal: SecretaryActionProposal = serde_json::from_str(&request.proposal_json)
            .map_err(|_| {
                MemoryCandidateControlStoreError::InvalidData(
                    "memory candidate control proposal_json is invalid".into(),
                )
            })?;
        if proposal.proposal_id != request.proposal_id || proposal.action != request.action {
            return Err(MemoryCandidateControlStoreError::InvalidData(
                "memory candidate control proposal does not match the requested action".into(),
            ));
        }
        match &request.action {
            SecretaryAction::ApproveMemoryCandidate {
                expected_candidate_version,
                reason,
                ..
            }
            | SecretaryAction::RejectMemoryCandidate {
                expected_candidate_version,
                reason,
                ..
            } if *expected_candidate_version == 0
                || reason.trim().is_empty()
                || reason.chars().count() > 1_000 =>
            {
                return Err(MemoryCandidateControlStoreError::InvalidData(
                    "memory candidate version or reason is invalid".into(),
                ));
            }
            SecretaryAction::ApproveMemoryCandidate { .. }
            | SecretaryAction::RejectMemoryCandidate { .. } => {}
            _ => {
                return Err(MemoryCandidateControlStoreError::InvalidData(
                    "action is not a memory candidate control".into(),
                ));
            }
        }
        self.store.apply_effect(request).await
    }
}

#[derive(Debug, Error)]
pub enum MemoryCandidateExtractorError {
    #[error("memory candidate extractor failed: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum MemoryCandidateUseCaseError {
    #[error("invalid memory candidate configuration: {0}")]
    InvalidConfiguration(String),
    #[error(transparent)]
    Extractor(#[from] MemoryCandidateExtractorError),
    #[error(transparent)]
    Candidate(#[from] crate::MemoryCandidateError),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContentTrustLevel, ConversationKind, ConversationRef, MemoryCandidateCursor, MessageRole,
        MessageSource, ThreadActorRef,
    };

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn event(id: &str, actor_id: &str, text: &str) -> MemoryCandidateEvent {
        MemoryCandidateEvent {
            source_event_id: SourceEventId::new(id).unwrap(),
            actor: ThreadActorRef {
                account: account(),
                actor_id: actor_id.into(),
                platform_identity_kind: None,
            },
            role: MessageRole::ExternalObservation,
            occurred_at_unix_secs: 1,
            content_trust_level: ContentTrustLevel::Normal,
            normalized_text: text.into(),
            content_omitted: false,
        }
    }

    fn batch(events: Vec<MemoryCandidateEvent>) -> MemoryCandidateBatch {
        MemoryCandidateBatch {
            account: account(),
            conversation: ConversationRef::new(ConversationKind::Group, "conv-1").unwrap(),
            lease_token: MemoryCandidateLeaseToken::generate(),
            events,
            next_cursor: MemoryCandidateCursor {
                received_at_unix_micros: 1,
                source_event_id: SourceEventId::new("e0").unwrap(),
            },
        }
    }

    #[tokio::test]
    async fn conservative_extractor_emits_three_kinds_with_stable_subjects() {
        let extractor = ConservativeMemoryCandidateExtractor::new(2000, "v1").unwrap();
        let batch = batch(vec![
            event("e0", "alice", "承诺：给 alice 发送报价单"),
            event("e1", "alice", "人物：重要客户"),
            event("e2", "bob", "项目：alpha 8 月上线"),
        ]);
        let candidates = extractor.extract(&batch).await.unwrap();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].payload.kind(), "commitment");
        assert_eq!(candidates[1].payload.kind(), "person");
        assert_eq!(candidates[2].payload.kind(), "project");
        for candidate in &candidates {
            validate_memory_candidate(candidate, &batch).unwrap();
        }
        // 同内容重复提取产生不同候选 ID 但相同 fingerprint（由 INSERT IGNORE 去重）
        let again = extractor.extract(&batch).await.unwrap();
        assert_eq!(
            again[0].deterministic_fingerprint,
            candidates[0].deterministic_fingerprint
        );
    }

    #[tokio::test]
    async fn conservative_extractor_skips_ambiguous_text_and_out_of_batch_beneficiary() {
        let extractor = ConservativeMemoryCandidateExtractor::new(2000, "v1").unwrap();
        // beneficiary "carol" 不在批次内：承诺候选被跳过，其余保留
        let batch = batch(vec![
            event("e0", "alice", "今天有点忙"),
            event("e1", "alice", "承诺：给 carol 发文档"),
            event("e2", "alice", "人物：老朋友"),
        ]);
        let candidates = extractor.extract(&batch).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].payload.kind(), "person");
    }
}
