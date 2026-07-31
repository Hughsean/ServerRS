//! Owner 通知策略的协议无关领域模型。
//!
//! 本模块只表达有限的结构化策略、候选和决策；不得承载聊天正文、模型推理或平台实现。

use chrono::{DateTime, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Clock, ConversationRef, SourceAccountRef};

pub const MAX_NOTIFICATION_POLICY_ID_BYTES: usize = 36;
pub const MAX_CANONICAL_SCOPE_KEY_BYTES: usize = 512;
pub const MAX_NOTIFICATION_REASON_BYTES: usize = 256;
pub const MAX_NOTIFICATION_AUDIT_SUMMARY_BYTES: usize = 1_024;
pub const MAX_NOTIFICATION_JSON_BYTES: usize = 8 * 1024;

macro_rules! notification_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, NotificationPolicyError> {
                let value = value.into();
                if value.trim().is_empty() || value.len() > MAX_NOTIFICATION_POLICY_ID_BYTES {
                    return Err(NotificationPolicyError::InvalidIdentity(format!(concat!(
                        $label,
                        " must contain 1..=36 bytes"
                    ))));
                }
                Ok(Self(value))
            }

            pub fn generate() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

notification_id!(PolicyFamilyId, "policy family id");
notification_id!(PolicyRevisionId, "policy revision id");
notification_id!(NotificationCandidateId, "notification candidate id");
notification_id!(EvaluationRequestId, "evaluation request id");
notification_id!(NotificationDecisionId, "notification decision id");

/// 匹配数据的三态表示。未知值绝不等同于缺失值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum MatchField<T> {
    Known(T),
    Absent,
    Unknown,
}

impl<T: PartialEq> MatchField<T> {
    pub fn matches(&self, actual: &Self) -> bool {
        match (self, actual) {
            (Self::Known(expected), Self::Known(value)) => expected == value,
            (Self::Absent, Self::Absent) => true,
            _ => false,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCategory {
    Agenda,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredImportance {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    AgendaDue,
    FollowUpDue,
}

/// v1 只允许用于结构化策略的元数据，绝不包含正文或相似度特征。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationMatchKeyV1 {
    pub account: SourceAccountRef,
    pub conversation: MatchField<ConversationRef>,
    pub actor_id: MatchField<String>,
    pub category: MatchField<NotificationCategory>,
    pub mentioned_owner: MatchField<bool>,
    pub structured_importance: MatchField<StructuredImportance>,
    pub event_kind: MatchField<EventKind>,
}

impl NotificationMatchKeyV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account: SourceAccountRef,
        conversation: MatchField<ConversationRef>,
        actor_id: MatchField<String>,
        category: MatchField<NotificationCategory>,
        mentioned_owner: MatchField<bool>,
        structured_importance: MatchField<StructuredImportance>,
        event_kind: MatchField<EventKind>,
    ) -> Result<Self, NotificationPolicyError> {
        let key = Self {
            account,
            conversation,
            actor_id,
            category,
            mentioned_owner,
            structured_importance,
            event_kind,
        };
        key.validate()?;
        Ok(key)
    }

    pub fn validate(&self) -> Result<(), NotificationPolicyError> {
        if matches!(&self.actor_id, MatchField::Known(value) if value.trim().is_empty()) {
            return Err(NotificationPolicyError::InvalidMatchKey(
                "actor id must not be empty when known".into(),
            ));
        }
        Ok(())
    }

    pub fn eligibility_for_long_term_rule(&self) -> Result<(), NotificationPolicyError> {
        if self.conversation.is_unknown()
            || self.actor_id.is_unknown()
            || self.category.is_unknown()
            || self.mentioned_owner.is_unknown()
            || self.structured_importance.is_unknown()
            || self.event_kind.is_unknown()
        {
            return Err(NotificationPolicyError::UnknownMatchMetadata);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCandidateRef {
    pub source_kind: String,
    pub source_id: String,
    pub source_version: u64,
    pub account: SourceAccountRef,
}

impl NotificationCandidateRef {
    pub fn new(
        source_kind: impl Into<String>,
        source_id: impl Into<String>,
        source_version: u64,
        account: SourceAccountRef,
    ) -> Result<Self, NotificationPolicyError> {
        let source_kind = source_kind.into();
        let source_id = source_id.into();
        if source_kind.trim().is_empty() || source_id.trim().is_empty() {
            return Err(NotificationPolicyError::InvalidCandidateRef);
        }
        Ok(Self {
            source_kind,
            source_id,
            source_version,
            account,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPolicyKind {
    AccountDefault,
    Conversation,
    Contact,
    Category,
    QuietHours,
    SimilarNotification,
    AutomaticReplyDenied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionKind {
    Rule,
    Tombstone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationOutcome {
    Remind,
    Delay,
    Suppress,
    CandidateExpired,
    EvaluationFailedTerminal,
    DeliveryWindowExpired,
    ScheduleTimeAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicyRule {
    pub match_key: NotificationMatchKeyV1,
    pub outcome: NotificationOutcome,
    pub bypass_quiet: bool,
    /// 旧 Revision JSON 不含这些可选字段；serde 默认值保证历史规则仍可读取。
    #[serde(default)]
    pub conversation: Option<ConversationNotificationRule>,
    #[serde(default)]
    pub quiet_hours: Option<QuietHoursRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicyFamily {
    pub policy_family_id: PolicyFamilyId,
    pub account: SourceAccountRef,
    pub canonical_scope_key: String,
    pub policy_kind: NotificationPolicyKind,
    pub current_revision_id: PolicyRevisionId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicyRevision {
    pub policy_revision_id: PolicyRevisionId,
    pub policy_family_id: PolicyFamilyId,
    pub revision_number: u64,
    pub supersedes_revision_id: Option<PolicyRevisionId>,
    pub revision_kind: RevisionKind,
    pub rule: Option<NotificationPolicyRule>,
}

impl NotificationPolicyRevision {
    pub fn validate(&self) -> Result<(), NotificationPolicyError> {
        match (self.revision_kind, &self.rule) {
            (RevisionKind::Rule, Some(_)) | (RevisionKind::Tombstone, None) => Ok(()),
            _ => Err(NotificationPolicyError::InvalidRevisionShape),
        }
    }
}

/// 会话完全静默时，只有该会话和命中的候选规则均显式授权才可突破。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationMode {
    Normal,
    FullySilent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationNotificationRule {
    pub mode: ConversationMode,
    pub allow_bypass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    CandidateExpired,
    ConversationFullySilent,
    ConversationPolicy,
    ContactPolicy,
    CategoryPolicy,
    AccountDefaultPolicy,
    QuietHours,
    ScheduleTimeAmbiguous,
    InvalidQuietHours,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationPlan {
    pub outcome: NotificationOutcome,
    pub reason: DecisionReason,
    /// 仅 Delay 使用；由纯 evaluator 根据输入时钟确定，提交层不得自行猜测。
    pub next_allowed_at_unix_secs: Option<i64>,
}

impl EvaluationPlan {
    fn new(outcome: NotificationOutcome, reason: DecisionReason) -> Self {
        Self {
            outcome,
            reason,
            next_allowed_at_unix_secs: None,
        }
    }

    pub fn with_next_allowed_at_unix_secs(mut self, value: i64) -> Self {
        self.next_allowed_at_unix_secs = Some(value);
        self
    }
}

/// 仅支持每日本地时间段；有效日期边界均包含。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHoursRule {
    pub timezone_name: String,
    pub start_local_time: String,
    pub end_local_time: String,
    pub effective_from_local_date: Option<String>,
    pub effective_until_local_date: Option<String>,
    /// 只有命中规则也显式 `bypass_quiet` 时才允许绕过静默时段。
    #[serde(default)]
    pub allow_bypass: bool,
}

impl QuietHoursRule {
    fn timezone(&self) -> Result<Tz, NotificationPolicyError> {
        self.timezone_name.parse().map_err(|_| {
            NotificationPolicyError::InvalidQuietHours("timezone must be a valid IANA name".into())
        })
    }

    fn boundaries(&self) -> Result<(NaiveTime, NaiveTime), NotificationPolicyError> {
        let start = NaiveTime::parse_from_str(&self.start_local_time, "%H:%M").map_err(|_| {
            NotificationPolicyError::InvalidQuietHours("start time must use HH:MM".into())
        })?;
        let end = NaiveTime::parse_from_str(&self.end_local_time, "%H:%M").map_err(|_| {
            NotificationPolicyError::InvalidQuietHours("end time must use HH:MM".into())
        })?;
        if start == end {
            return Err(NotificationPolicyError::InvalidQuietHours(
                "start and end local times must differ".into(),
            ));
        }
        Ok((start, end))
    }

    fn effective_range(
        &self,
    ) -> Result<(Option<NaiveDate>, Option<NaiveDate>), NotificationPolicyError> {
        let parse = |value: &Option<String>| {
            value
                .as_deref()
                .map(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d"))
                .transpose()
                .map_err(|_| {
                    NotificationPolicyError::InvalidQuietHours(
                        "effective date must use YYYY-MM-DD".into(),
                    )
                })
        };
        let range = (
            parse(&self.effective_from_local_date)?,
            parse(&self.effective_until_local_date)?,
        );
        if matches!(range, (Some(start), Some(end)) if start > end) {
            return Err(NotificationPolicyError::InvalidQuietHours(
                "effective start date must not exceed end date".into(),
            ));
        }
        Ok(range)
    }

    fn applies_on(&self, date: NaiveDate) -> Result<bool, NotificationPolicyError> {
        let (from, until) = self.effective_range()?;
        Ok(from.is_none_or(|from| date >= from) && until.is_none_or(|until| date <= until))
    }

    fn contains_local_time(&self, time: NaiveTime) -> Result<bool, NotificationPolicyError> {
        let (start, end) = self.boundaries()?;
        Ok(if start < end {
            start <= time && time < end
        } else {
            start <= time || time < end
        })
    }
}

#[derive(Debug, Clone)]
pub struct EvaluationInput {
    pub candidate_is_current: bool,
    pub matching_rule: Option<NotificationPolicyRule>,
    pub conversation_rule: Option<ConversationNotificationRule>,
    pub contact_rule: Option<NotificationPolicyRule>,
    pub category_rule: Option<NotificationPolicyRule>,
    pub account_default_rule: Option<NotificationPolicyRule>,
    pub quiet_hours: Option<QuietHoursRule>,
    pub now_unix_secs: i64,
}

#[derive(Debug, Default)]
pub struct NotificationPolicyEvaluator;

impl NotificationPolicyEvaluator {
    pub fn evaluate(&self, input: &EvaluationInput) -> EvaluationPlan {
        if !input.candidate_is_current {
            return EvaluationPlan::new(
                NotificationOutcome::CandidateExpired,
                DecisionReason::CandidateExpired,
            );
        }
        if let Some(conversation) = input.conversation_rule
            && conversation.mode == ConversationMode::FullySilent
            && !matches!(&input.matching_rule, Some(rule) if rule.bypass_quiet && conversation.allow_bypass)
        {
            return EvaluationPlan::new(
                NotificationOutcome::Suppress,
                DecisionReason::ConversationFullySilent,
            );
        }
        let selected_rule = input
            .matching_rule
            .as_ref()
            .map(|rule| (rule, DecisionReason::ConversationPolicy))
            .or_else(|| {
                input
                    .contact_rule
                    .as_ref()
                    .map(|rule| (rule, DecisionReason::ContactPolicy))
            })
            .or_else(|| {
                input
                    .category_rule
                    .as_ref()
                    .map(|rule| (rule, DecisionReason::CategoryPolicy))
            })
            .or_else(|| {
                input
                    .account_default_rule
                    .as_ref()
                    .map(|rule| (rule, DecisionReason::AccountDefaultPolicy))
            });
        match input
            .quiet_hours
            .as_ref()
            .map(|rule| quiet_hours_result(rule, input.now_unix_secs))
        {
            Some(Ok(true))
                if !matches!(
                    selected_rule,
                    Some((rule, _)) if rule.bypass_quiet
                        && input.quiet_hours.as_ref().is_some_and(|rule| rule.allow_bypass)
                ) =>
            {
                EvaluationPlan::new(NotificationOutcome::Suppress, DecisionReason::QuietHours)
            }
            Some(Err(NotificationPolicyError::ScheduleTimeAmbiguous)) => EvaluationPlan::new(
                NotificationOutcome::ScheduleTimeAmbiguous,
                DecisionReason::ScheduleTimeAmbiguous,
            ),
            Some(Err(_)) => EvaluationPlan::new(
                NotificationOutcome::EvaluationFailedTerminal,
                DecisionReason::InvalidQuietHours,
            ),
            Some(Ok(true)) | Some(Ok(false)) | None => match selected_rule {
                Some((rule, reason)) => EvaluationPlan::new(rule.outcome, reason),
                None => EvaluationPlan::new(
                    NotificationOutcome::Remind,
                    DecisionReason::AccountDefaultPolicy,
                ),
            },
        }
    }
}

/// 在写入策略前检查未来 400 个本地日期的两个边界，禁止引入 DST 不存在或重复时间。
pub fn validate_quiet_hours(
    rule: &QuietHoursRule,
    clock: &dyn Clock,
) -> Result<(), NotificationPolicyError> {
    let timezone = rule.timezone()?;
    let (start, end) = rule.boundaries()?;
    let (from, until) = rule.effective_range()?;
    let now = DateTime::<Utc>::from_timestamp(clock.now_unix_secs(), 0).ok_or_else(|| {
        NotificationPolicyError::InvalidQuietHours("clock timestamp is invalid".into())
    })?;
    let first_day = now.with_timezone(&timezone).date_naive();
    for offset in 0..400 {
        let date = first_day
            .checked_add_days(chrono::Days::new(offset))
            .ok_or_else(|| {
                NotificationPolicyError::InvalidQuietHours("date range overflow".into())
            })?;
        if from.is_some_and(|value| date < value) || until.is_some_and(|value| date > value) {
            continue;
        }
        for time in [start, end] {
            if !matches!(
                timezone.from_local_datetime(&date.and_time(time)),
                LocalResult::Single(_)
            ) {
                return Err(NotificationPolicyError::AmbiguousScheduleTime);
            }
        }
    }
    Ok(())
}

fn quiet_hours_result(
    rule: &QuietHoursRule,
    now_unix_secs: i64,
) -> Result<bool, NotificationPolicyError> {
    let timezone = rule.timezone()?;
    let now = DateTime::<Utc>::from_timestamp(now_unix_secs, 0).ok_or_else(|| {
        NotificationPolicyError::InvalidQuietHours("clock timestamp is invalid".into())
    })?;
    let local = now.with_timezone(&timezone);
    if !rule.applies_on(local.date_naive())? || !rule.contains_local_time(local.time())? {
        return Ok(false);
    }
    let (start, end) = rule.boundaries()?;
    for time in [start, end] {
        if !matches!(
            timezone.from_local_datetime(&local.date_naive().and_time(time)),
            LocalResult::Single(_)
        ) {
            return Err(NotificationPolicyError::ScheduleTimeAmbiguous);
        }
    }
    Ok(true)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NotificationPolicyError {
    #[error("invalid notification policy identity: {0}")]
    InvalidIdentity(String),
    #[error("invalid notification policy match key: {0}")]
    InvalidMatchKey(String),
    #[error("long-term rule requires known or absent match metadata")]
    UnknownMatchMetadata,
    #[error("invalid notification candidate reference")]
    InvalidCandidateRef,
    #[error("rule and tombstone revision shapes are mutually exclusive")]
    InvalidRevisionShape,
    #[error("invalid quiet-hours rule: {0}")]
    InvalidQuietHours(String),
    #[error("quiet-hours boundary has an ambiguous or nonexistent local time")]
    AmbiguousScheduleTime,
    #[error("quiet-hours evaluation encountered an ambiguous local time")]
    ScheduleTimeAmbiguous,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, MessageSource};

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn key(conversation: MatchField<ConversationRef>) -> NotificationMatchKeyV1 {
        NotificationMatchKeyV1::new(
            account(),
            conversation,
            MatchField::Known("actor-1".into()),
            MatchField::Known(NotificationCategory::Agenda),
            MatchField::Known(true),
            MatchField::Known(StructuredImportance::Normal),
            MatchField::Known(EventKind::AgendaDue),
        )
        .unwrap()
    }

    #[test]
    fn unknown_match_field_never_matches_absent_field() {
        assert!(!MatchField::<bool>::Unknown.matches(&MatchField::Absent));
    }

    #[test]
    fn feedback_with_unknown_required_metadata_cannot_promote_rule() {
        let key = key(MatchField::Unknown);
        assert_eq!(
            key.eligibility_for_long_term_rule(),
            Err(NotificationPolicyError::UnknownMatchMetadata)
        );
    }

    #[test]
    fn known_conversation_can_be_used_for_long_term_rule() {
        let conversation = ConversationRef::new(ConversationKind::Group, "group-1").unwrap();
        assert_eq!(
            key(MatchField::Known(conversation)).eligibility_for_long_term_rule(),
            Ok(())
        );
    }

    #[test]
    fn revision_requires_rule_or_tombstone_shape() {
        let revision = NotificationPolicyRevision {
            policy_revision_id: PolicyRevisionId::generate(),
            policy_family_id: PolicyFamilyId::generate(),
            revision_number: 1,
            supersedes_revision_id: None,
            revision_kind: RevisionKind::Rule,
            rule: None,
        };
        assert_eq!(
            revision.validate(),
            Err(NotificationPolicyError::InvalidRevisionShape)
        );
    }

    fn rule(outcome: NotificationOutcome, bypass_quiet: bool) -> NotificationPolicyRule {
        NotificationPolicyRule {
            match_key: key(MatchField::Absent),
            outcome,
            bypass_quiet,
            conversation: None,
            quiet_hours: None,
        }
    }

    fn input(now_unix_secs: i64) -> EvaluationInput {
        EvaluationInput {
            candidate_is_current: true,
            matching_rule: None,
            conversation_rule: None,
            contact_rule: None,
            category_rule: None,
            account_default_rule: None,
            quiet_hours: None,
            now_unix_secs,
        }
    }

    #[test]
    fn fully_silent_conversation_requires_two_explicit_bypass_grants() {
        let mut input = input(0);
        input.conversation_rule = Some(ConversationNotificationRule {
            mode: ConversationMode::FullySilent,
            allow_bypass: false,
        });
        input.matching_rule = Some(rule(NotificationOutcome::Remind, true));
        let decision = NotificationPolicyEvaluator.evaluate(&input);
        assert_eq!(decision.outcome, NotificationOutcome::Suppress);
        assert_eq!(decision.reason, DecisionReason::ConversationFullySilent);

        input.conversation_rule = Some(ConversationNotificationRule {
            mode: ConversationMode::FullySilent,
            allow_bypass: true,
        });
        assert_eq!(
            NotificationPolicyEvaluator.evaluate(&input).outcome,
            NotificationOutcome::Remind
        );
    }

    #[test]
    fn quiet_hours_bypass_requires_rule_and_quiet_hours_grants() {
        let mut input = input(1_704_125_700);
        input.quiet_hours = Some(QuietHoursRule {
            timezone_name: "Asia/Shanghai".into(),
            start_local_time: "23:00".into(),
            end_local_time: "07:00".into(),
            effective_from_local_date: None,
            effective_until_local_date: None,
            allow_bypass: true,
        });
        input.contact_rule = Some(rule(NotificationOutcome::Remind, false));
        assert_eq!(
            NotificationPolicyEvaluator.evaluate(&input).reason,
            DecisionReason::QuietHours
        );

        input.contact_rule = Some(rule(NotificationOutcome::Remind, true));
        assert_eq!(
            NotificationPolicyEvaluator.evaluate(&input).outcome,
            NotificationOutcome::Remind
        );

        input.quiet_hours.as_mut().unwrap().allow_bypass = false;
        assert_eq!(
            NotificationPolicyEvaluator.evaluate(&input).reason,
            DecisionReason::QuietHours
        );
    }

    #[test]
    fn quiet_hours_cross_midnight_are_suppressed() {
        // 2024-01-01T16:15:00Z，即 Asia/Shanghai 的 2024-01-02 00:15。
        let mut input = input(1_704_125_700);
        input.quiet_hours = Some(QuietHoursRule {
            timezone_name: "Asia/Shanghai".into(),
            start_local_time: "23:00".into(),
            end_local_time: "07:00".into(),
            effective_from_local_date: None,
            effective_until_local_date: None,
            allow_bypass: false,
        });
        assert_eq!(
            NotificationPolicyEvaluator.evaluate(&input).reason,
            DecisionReason::QuietHours
        );
    }

    struct FixedClock(i64);

    impl Clock for FixedClock {
        fn now_unix_secs(&self) -> i64 {
            self.0
        }
    }

    #[test]
    fn malformed_quiet_hours_are_not_classified_as_schedule_ambiguity() {
        let mut input = input(1_704_125_700);
        input.quiet_hours = Some(QuietHoursRule {
            timezone_name: "Asia/Shanghai".into(),
            start_local_time: "25:00".into(),
            end_local_time: "07:00".into(),
            effective_from_local_date: None,
            effective_until_local_date: None,
            allow_bypass: false,
        });
        let decision = NotificationPolicyEvaluator.evaluate(&input);
        assert_eq!(
            decision.outcome,
            NotificationOutcome::EvaluationFailedTerminal
        );
        assert_eq!(decision.reason, DecisionReason::InvalidQuietHours);
    }

    #[test]
    fn dst_nonexistent_and_repeated_boundaries_are_rejected() {
        let nonexistent = QuietHoursRule {
            timezone_name: "America/New_York".into(),
            start_local_time: "02:30".into(),
            end_local_time: "03:30".into(),
            effective_from_local_date: Some("2026-03-08".into()),
            effective_until_local_date: Some("2026-03-08".into()),
            allow_bypass: false,
        };
        assert_eq!(
            validate_quiet_hours(&nonexistent, &FixedClock(1_772_928_000)),
            Err(NotificationPolicyError::AmbiguousScheduleTime)
        );

        let repeated = QuietHoursRule {
            timezone_name: "America/New_York".into(),
            start_local_time: "01:30".into(),
            end_local_time: "02:30".into(),
            effective_from_local_date: Some("2026-11-01".into()),
            effective_until_local_date: Some("2026-11-01".into()),
            allow_bypass: false,
        };
        assert_eq!(
            validate_quiet_hours(&repeated, &FixedClock(1_793_491_200)),
            Err(NotificationPolicyError::AmbiguousScheduleTime)
        );
    }
}
