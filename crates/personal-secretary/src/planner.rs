//! Action Planner 领域类型与端口。
//!
//! 本模块定义 Planner 的输入输出、`ActionPlannerT` 端口、`Clock` 时间抽象和时间解析纯函数。
//! 领域层不依赖 LLM 客户端、SeaORM 或 NapCat；LLM 适配在 `apps/qqbot-server` 中实现，
//! 并将 Trait 与错误类型保持为 `pub(crate)`（见约束 9）。
//!
//! Planner 不新建状态机，复用现有 `SecretaryActionProposal`；它只负责把
//! OwnerCommand + 已检索上下文转换为类型化 Proposal 或 NoAction / Clarification。

use async_trait::async_trait;
use chrono::NaiveDateTime;

use crate::{
    ConversationRef, RecentEventRef, SecretaryAction, SecretaryActionProposal, SourceAccountRef,
    SourceEventId,
};

// ===== 有界常量 =====

const MAX_COMMAND_TEXT_CHARS: usize = 4_000;
const MAX_RECENT_EVENTS: usize = 8;
const MAX_RETRIEVED_EXCERPTS: usize = 20;
const MAX_EXCERPT_CHARS: usize = 1_000;
const MAX_TIMEZONE_NAME_BYTES: usize = 64;
const MAX_TIME_EXPRESSION_CHARS: usize = 200;
const MAX_REASON_CHARS: usize = 1_000;
const MAX_QUESTION_CHARS: usize = 1_000;
const MAX_EVIDENCE: usize = 20;
/// UTC 偏移允许范围：-14h..=14h（秒）。
const MIN_OFFSET_SECS: i64 = -50_400;
const MAX_OFFSET_SECS: i64 = 50_400;

// ===== 本批允许的 Action 白名单（约束 5）=====

/// 本批 Planner 可以生成的 Action 仅限于已类型化白名单；所有策略写仍须在
/// Effect 层完成 OwnerCommand 授权、审批与持久化 Receipt 后才可宣告成功。
pub fn is_allowed_action_in_batch(action: &SecretaryAction) -> bool {
    use SecretaryAction::*;
    matches!(
        action,
        SearchRecentEvents { .. }
            | ReadSourceEvent { .. }
            | SearchEventThreads { .. }
            | ResolveReference { .. }
            | ListUpcomingItems { .. }
            | GetSecretaryStatus
            | ListPendingOwnerWork { .. }
            | GetThreadContext { .. }
            | DraftReminder { .. }
            | AskOwnerClarification { .. }
            | CreateSchedule { .. }
            | CreateTask { .. }
            | CreateReminder { .. }
            | RescheduleItem { .. }
            | CancelItem { .. }
            | CompleteItem { .. }
            | SnoozeItem { .. }
            | ListNotificationPolicies { .. }
            | ExplainNotificationDecision { .. }
            | SetAccountDefaultNotificationMode { .. }
            | SetConversationNotificationMode { .. }
            | SetQuietHours { .. }
            | SetImportantContact { .. }
            | SetNotificationCategoryImportance { .. }
            | RecordNotificationFeedback { .. }
            | CreateSimilarNotificationRule { .. }
            | DisableNotificationPolicy { .. }
            | SetAutomaticReplyDeniedForContact { .. }
            | ListMemoryFacts { .. }
            | ReadMemoryFactSources { .. }
            | CorrectMemoryFact { .. }
            | DeleteMemoryFact { .. }
            | SetMemoryFactTtl { .. }
            | SetConversationMemoryMode { .. }
            | ConfirmThreadDecision { .. }
            | RevokeThreadDecision { .. }
            | DismissThreadQuestion { .. }
            | SetThreadLifecycle { .. }
            | DismissFollowUp { .. }
            | SnoozeFollowUp { .. }
            | DismissFollowUps { .. }
    )
}

// ===== Clock 时间抽象 =====

/// 时间抽象，便于测试注入固定时间。生产用 `SystemClock`。
pub trait Clock: Send + Sync {
    fn now_unix_secs(&self) -> i64;
}

/// 读取真实 wall-clock 时间的系统时钟。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_secs(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

// ===== PlannerInput =====

/// Planner 输入的来源命令事件摘要。只包含有界正文，不包含完整消息段。
#[derive(Debug, Clone)]
pub struct PlannerCommandEvent {
    pub source_event_id: SourceEventId,
    pub conversation: ConversationRef,
    pub occurred_at_unix_secs: i64,
    pub normalized_text: String,
}

/// Planner 输入中已检索的上下文摘要。由 Retrieve 节点填充，正文按内容策略脱敏后有界。
#[derive(Debug, Clone)]
pub struct PlannerRetrievedExcerpt {
    pub source_event_id: SourceEventId,
    pub excerpt: String,
    pub occurred_at_unix_secs: i64,
    pub actor_id: String,
}

/// Planner 输入。账号作用域严格限定，跨账号查询被拒绝。
#[derive(Debug, Clone)]
pub struct PlannerInput {
    pub account: SourceAccountRef,
    pub command: PlannerCommandEvent,
    pub recent_events: Vec<RecentEventRef>,
    /// UTC 偏移秒数（如 Asia/Shanghai 为 28800）。由已验证配置生成，不由调用方随意传入。
    pub timezone_offset_secs: i64,
    pub timezone: String,
    pub now_unix_secs: i64,
    pub retrieved: Vec<PlannerRetrievedExcerpt>,
}

// ===== PlannerOutput =====

/// Planner 输出。复用 `SecretaryActionProposal`，不新建状态机。
///
/// `SecretaryActionProposal` 是公共协议类型，对其 Boxing 会破坏所有调用方与序列化兼容性，
/// 因此在没有独立兼容迁移方案之前抑制该枚举尺寸警告。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum PlannerOutput {
    /// Planner 判定无需执行任何动作。
    NoAction { reason: String },
    /// Planner 需要 Owner 澄清，对应 `AskOwnerClarification` 的语义。
    Clarification {
        question: String,
        evidence: Vec<SourceEventId>,
    },
    /// Planner 生成类型化 Proposal，交由 `gate_secretary_action` 路由。
    Proposal(SecretaryActionProposal),
}

// ===== ActionPlannerT 端口 =====

/// Planner 端口。应用层实现（`LlmActionPlanner`）调用 LLM 并把输出转为类型化结果。
#[async_trait]
pub trait ActionPlannerT: Send + Sync {
    async fn plan(&self, input: &PlannerInput) -> Result<PlannerOutput, PlannerError>;
}

// ===== 错误类型 =====

#[derive(Debug, thiserror::Error)]
pub enum PlannerError {
    #[error("invalid planner input: {0}")]
    InvalidInput(String),
    #[error("invalid planner output: {0}")]
    InvalidOutput(String),
    #[error("planner produced disallowed action: {0}")]
    DisallowedAction(String),
    #[error("llm call failed: {0}")]
    LlmCall(String),
    #[error("llm returned unparseable output: {0}")]
    UnparseableOutput(String),
    #[error("planner timed out")]
    Timeout,
}

// ===== 校验纯函数 =====

/// 校验 PlannerInput 的有界约束。
pub fn validate_planner_input(input: &PlannerInput) -> Result<(), PlannerError> {
    non_empty_bounded_text(
        "command.normalized_text",
        &input.command.normalized_text,
        1,
        MAX_COMMAND_TEXT_CHARS,
    )?;
    if input.recent_events.len() > MAX_RECENT_EVENTS {
        return Err(PlannerError::InvalidInput(format!(
            "recent_events must not exceed {MAX_RECENT_EVENTS} items"
        )));
    }
    if !(MIN_OFFSET_SECS..=MAX_OFFSET_SECS).contains(&input.timezone_offset_secs) {
        return Err(PlannerError::InvalidInput(format!(
            "timezone_offset_secs must be in {MIN_OFFSET_SECS}..={MAX_OFFSET_SECS}"
        )));
    }
    if input.timezone.trim().is_empty()
        || input.timezone.len() > MAX_TIMEZONE_NAME_BYTES
        || input.timezone.parse::<chrono_tz::Tz>().is_err()
    {
        return Err(PlannerError::InvalidInput(
            "timezone must be a valid IANA timezone".into(),
        ));
    }
    if input.retrieved.len() > MAX_RETRIEVED_EXCERPTS {
        return Err(PlannerError::InvalidInput(format!(
            "retrieved must not exceed {MAX_RETRIEVED_EXCERPTS} items"
        )));
    }
    for excerpt in &input.retrieved {
        // excerpt 允许空（envelope_only），但 actor_id 必须非空。
        bounded_text("retrieved.excerpt", &excerpt.excerpt, 0, MAX_EXCERPT_CHARS)?;
        non_empty_bounded_text("retrieved.actor_id", &excerpt.actor_id, 1, 191)?;
    }
    Ok(())
}

/// 校验 PlannerOutput。Proposal 复用 `validate_action_proposal`，并检查白名单。
pub fn validate_planner_output(output: &PlannerOutput) -> Result<(), PlannerError> {
    match output {
        PlannerOutput::NoAction { reason } => {
            non_empty_bounded_text("NoAction.reason", reason, 1, MAX_REASON_CHARS)?;
        }
        PlannerOutput::Clarification { question, evidence } => {
            non_empty_bounded_text("Clarification.question", question, 1, MAX_QUESTION_CHARS)?;
            if evidence.len() > MAX_EVIDENCE {
                return Err(PlannerError::InvalidOutput(format!(
                    "Clarification.evidence must not exceed {MAX_EVIDENCE} items"
                )));
            }
        }
        PlannerOutput::Proposal(proposal) => {
            crate::validate_action_proposal(proposal)
                .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?;
            if !is_allowed_action_in_batch(&proposal.action) {
                return Err(PlannerError::DisallowedAction(format!(
                    "{:?} is not allowed in this batch",
                    proposal.action.kind()
                )));
            }
        }
    }
    Ok(())
}

// ===== 时间解析纯函数 =====

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TimeParseError {
    #[error("empty time expression")]
    Empty,
    #[error("time expression too long")]
    TooLong,
    #[error("invalid datetime format: {0}")]
    InvalidFormat(String),
    #[error("unknown timezone: {0}")]
    UnknownTimezone(String),
}

/// 解析 ISO 8601 / 简化日期时间字符串为 `NaiveDateTime`。
///
/// 支持格式：
/// - `2026-07-25T15:30:00`
/// - `2026-07-25 15:30:00`
/// - `2026-07-25T15:30`
/// - `2026-07-25 15:30`
/// - `2026-07-25`（默认 00:00:00）
///
/// 不处理自然语言（"明天下午3点"）；LLM 被要求输出 ISO 格式。
pub fn parse_iso_datetime(input: &str) -> Result<NaiveDateTime, TimeParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(TimeParseError::Empty);
    }
    if trimmed.chars().count() > MAX_TIME_EXPRESSION_CHARS {
        return Err(TimeParseError::TooLong);
    }
    for format in &[
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Ok(dt);
        }
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        // 固定偏移时区没有 DST，午夜一定存在；用 `and_hms_opt` 避免 deprecated 警告。
        return date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| TimeParseError::InvalidFormat(trimmed.into()));
    }
    Err(TimeParseError::InvalidFormat(trimmed.into()))
}

/// 把 `NaiveDateTime` 按固定 UTC 偏移转为 Unix 秒。
///
/// 固定偏移不会产生 DST 歧义，直接用 naive 当作 UTC 的时间戳减去偏移即可。
/// 偏移超出合理范围时按 UTC 处理（不 panic）。
pub fn naive_to_unix(naive: NaiveDateTime, offset_secs: i64) -> i64 {
    use chrono::TimeZone;
    let utc_unix = chrono::Utc.from_utc_datetime(&naive).timestamp();
    if (MIN_OFFSET_SECS..=MAX_OFFSET_SECS).contains(&offset_secs) {
        utc_unix - offset_secs
    } else {
        utc_unix
    }
}

/// 解析常见时区名称为 UTC 偏移秒数。
///
/// 仅返回固定偏移，**不处理 DST**。本批主要服务中国用户（UTC+8，无 DST）。
/// 需要 DST 支持的时区应后续引入 `chrono-tz`。
pub fn parse_common_timezone_offset_secs(name: &str) -> Option<i64> {
    if name.len() > MAX_TIMEZONE_NAME_BYTES {
        return None;
    }
    match name.trim() {
        "UTC" | "Z" | "Etc/UTC" | "Etc/GMT" | "GMT" => Some(0),
        "Asia/Shanghai" | "Asia/Hong_Kong" | "Asia/Taipei" | "Asia/Macau" | "Asia/Singapore" => {
            Some(28_800)
        }
        "Asia/Tokyo" | "Asia/Seoul" => Some(32_400),
        "Asia/Bangkok" | "Asia/Jakarta" => Some(25_200),
        "Asia/Kolkata" | "Asia/Calcutta" => Some(19_800),
        "Asia/Dubai" => Some(14_400),
        "America/New_York" | "America/Detroit" => Some(-18_000),
        "America/Chicago" => Some(-21_600),
        "America/Denver" => Some(-25_200),
        "America/Los_Angeles" => Some(-28_800),
        "America/Sao_Paulo" => Some(-10_800),
        "Europe/London" | "Europe/Lisbon" | "Atlantic/Reykjavik" => Some(0),
        "Europe/Paris" | "Europe/Berlin" | "Europe/Rome" | "Europe/Madrid" | "Europe/Amsterdam" => {
            Some(3_600)
        }
        "Europe/Moscow" => Some(10_800),
        "Australia/Sydney" => Some(36_000),
        "Australia/Perth" => Some(28_800),
        _ => None,
    }
}

/// 解析 IANA timezone 中的本地日期时间；DST 歧义或不存在时间一律要求澄清。
pub fn parse_datetime_with_timezone(
    input: &str,
    timezone_name: &str,
) -> Result<i64, TimeParseError> {
    use chrono::TimeZone;
    let naive = parse_iso_datetime(input)?;
    let timezone = timezone_name
        .parse::<chrono_tz::Tz>()
        .map_err(|_| TimeParseError::UnknownTimezone(timezone_name.into()))?;
    match timezone.from_local_datetime(&naive) {
        chrono::LocalResult::Single(value) => Ok(value.timestamp()),
        chrono::LocalResult::Ambiguous(_, _) => Err(TimeParseError::InvalidFormat(
            "ambiguous local datetime".into(),
        )),
        chrono::LocalResult::None => Err(TimeParseError::InvalidFormat(
            "nonexistent local datetime".into(),
        )),
    }
}

fn bounded_text(field: &str, value: &str, min: usize, max: usize) -> Result<(), PlannerError> {
    let count = value.chars().count();
    if !(min..=max).contains(&count) {
        return Err(PlannerError::InvalidInput(format!(
            "{field} must contain {min}..={max} characters"
        )));
    }
    Ok(())
}

/// 与 `bounded_text` 相同，但额外拒绝纯空白字符串（trim 后为空）。
/// 用于语义上必须有实际内容的字段，如命令文本和理由。
fn non_empty_bounded_text(
    field: &str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), PlannerError> {
    if value.trim().is_empty() {
        return Err(PlannerError::InvalidInput(format!(
            "{field} must not be blank"
        )));
    }
    bounded_text(field, value, min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConversationKind, MessageSource, SecretaryAction, SourceAccountRef, SourceEventId,
    };
    use chrono::{Datelike, Timelike};

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn command_event(text: &str) -> PlannerCommandEvent {
        PlannerCommandEvent {
            source_event_id: SourceEventId::new("event-1").unwrap(),
            conversation: ConversationRef::new(ConversationKind::OwnerControl, "conv-1").unwrap(),
            occurred_at_unix_secs: 1_000,
            normalized_text: text.into(),
        }
    }

    fn minimal_input(text: &str) -> PlannerInput {
        PlannerInput {
            account: account(),
            command: command_event(text),
            recent_events: Vec::new(),
            timezone_offset_secs: 28_800,
            timezone: "Asia/Shanghai".into(),
            now_unix_secs: 1_000,
            retrieved: Vec::new(),
        }
    }

    // ===== is_allowed_action_in_batch =====

    #[test]
    fn search_recent_events_is_allowed() {
        assert!(is_allowed_action_in_batch(
            &SecretaryAction::SearchRecentEvents {
                query: "今天".into(),
                limit: 20,
            }
        ));
    }

    #[test]
    fn draft_reminder_is_allowed() {
        assert!(is_allowed_action_in_batch(
            &SecretaryAction::DraftReminder {
                text: "提醒".into(),
                due_at_unix: 1_800_000_000,
            }
        ));
    }

    #[test]
    fn ask_owner_clarification_is_allowed() {
        assert!(is_allowed_action_in_batch(
            &SecretaryAction::AskOwnerClarification {
                question: "哪个？".into(),
            }
        ));
    }

    #[test]
    fn create_reminder_is_allowed_and_suspends_for_owner_approval() {
        assert!(is_allowed_action_in_batch(
            &SecretaryAction::CreateReminder {
                text: "提醒".into(),
                due_at_unix: 1_800_000_000,
                timezone: "Asia/Shanghai".into(),
            }
        ));
    }

    #[test]
    fn send_owner_message_is_not_allowed_in_batch() {
        assert!(!is_allowed_action_in_batch(
            &SecretaryAction::SendOwnerMessage {
                text: "通知".into(),
            }
        ));
    }

    // ===== validate_planner_input =====

    #[test]
    fn validate_input_rejects_empty_command() {
        let mut input = minimal_input("有效命令");
        input.command.normalized_text = "  ".into();
        assert!(validate_planner_input(&input).is_err());
    }

    #[test]
    fn validate_input_rejects_oversized_command() {
        let mut input = minimal_input("有效命令");
        input.command.normalized_text = "x".repeat(MAX_COMMAND_TEXT_CHARS + 1);
        assert!(validate_planner_input(&input).is_err());
    }

    #[test]
    fn validate_input_rejects_too_many_recent_events() {
        let mut input = minimal_input("有效命令");
        input.recent_events = (0..=MAX_RECENT_EVENTS)
            .map(|i| RecentEventRef {
                source_event_id: SourceEventId::new(format!("event-{i}")).unwrap(),
                summary: "摘要".into(),
            })
            .collect();
        assert!(validate_planner_input(&input).is_err());
    }

    #[test]
    fn validate_input_rejects_timezone_out_of_range() {
        let mut input = minimal_input("有效命令");
        input.timezone_offset_secs = MAX_OFFSET_SECS + 1;
        assert!(validate_planner_input(&input).is_err());
    }

    #[test]
    fn validate_input_rejects_negative_timezone_below_range() {
        let mut input = minimal_input("有效命令");
        input.timezone_offset_secs = MIN_OFFSET_SECS - 1;
        assert!(validate_planner_input(&input).is_err());
    }

    #[test]
    fn validate_input_rejects_oversized_excerpt() {
        let mut input = minimal_input("有效命令");
        input.retrieved.push(PlannerRetrievedExcerpt {
            source_event_id: SourceEventId::new("event-2").unwrap(),
            excerpt: "x".repeat(MAX_EXCERPT_CHARS + 1),
            occurred_at_unix_secs: 100,
            actor_id: "actor-1".into(),
        });
        assert!(validate_planner_input(&input).is_err());
    }

    #[test]
    fn validate_input_accepts_empty_excerpt_for_envelope_only() {
        let mut input = minimal_input("有效命令");
        input.retrieved.push(PlannerRetrievedExcerpt {
            source_event_id: SourceEventId::new("event-2").unwrap(),
            excerpt: String::new(),
            occurred_at_unix_secs: 100,
            actor_id: "actor-1".into(),
        });
        assert!(validate_planner_input(&input).is_ok());
    }

    #[test]
    fn validate_input_accepts_valid_input() {
        assert!(validate_planner_input(&minimal_input("帮我查最近的消息")).is_ok());
    }

    // ===== validate_planner_output =====

    #[test]
    fn validate_output_accepts_no_action() {
        let output = PlannerOutput::NoAction {
            reason: "无需动作".into(),
        };
        assert!(validate_planner_output(&output).is_ok());
    }

    #[test]
    fn validate_output_rejects_empty_no_action_reason() {
        let output = PlannerOutput::NoAction {
            reason: "  ".into(),
        };
        assert!(validate_planner_output(&output).is_err());
    }

    #[test]
    fn validate_output_accepts_clarification() {
        let output = PlannerOutput::Clarification {
            question: "你指的是哪个会议？".into(),
            evidence: vec![SourceEventId::new("event-1").unwrap()],
        };
        assert!(validate_planner_output(&output).is_ok());
    }

    #[test]
    fn validate_output_rejects_clarification_with_too_much_evidence() {
        let output = PlannerOutput::Clarification {
            question: "哪个？".into(),
            evidence: (0..=MAX_EVIDENCE)
                .map(|i| SourceEventId::new(format!("event-{i}")).unwrap())
                .collect(),
        };
        assert!(validate_planner_output(&output).is_err());
    }

    #[test]
    fn validate_output_accepts_allowed_proposal() {
        let proposal = SecretaryActionProposal::new(
            SecretaryAction::SearchRecentEvents {
                query: "报价单".into(),
                limit: 20,
            },
            "用户要求检索最近事件",
            vec![SourceEventId::new("event-1").unwrap()],
            None,
        )
        .unwrap();
        assert!(validate_planner_output(&PlannerOutput::Proposal(proposal)).is_ok());
    }

    #[test]
    fn validate_output_rejects_disallowed_proposal() {
        // CreateReminder 为 L2；在 server-generated idempotency key 到位后允许进入 Suspend。
        let proposal = SecretaryActionProposal::new(
            SecretaryAction::CreateReminder {
                text: "提交报价单".into(),
                due_at_unix: 1_800_000_000,
                timezone: "Asia/Shanghai".into(),
            },
            "用户要求创建提醒",
            vec![SourceEventId::new("event-1").unwrap()],
            Some("reminder:quote:deadline".into()),
        )
        .unwrap();
        assert!(validate_planner_output(&PlannerOutput::Proposal(proposal)).is_ok());
    }

    // ===== 时间解析 =====

    #[test]
    fn parse_iso_datetime_full_t_separator() {
        let dt = parse_iso_datetime("2026-07-25T15:30:00").unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 7);
        assert_eq!(dt.day(), 25);
        assert_eq!(dt.hour(), 15);
        assert_eq!(dt.minute(), 30);
        assert_eq!(dt.second(), 0);
    }

    #[test]
    fn parse_iso_datetime_full_space_separator() {
        let dt = parse_iso_datetime("2026-07-25 15:30:00").unwrap();
        assert_eq!(dt.hour(), 15);
        assert_eq!(dt.minute(), 30);
    }

    #[test]
    fn parse_iso_datetime_without_seconds() {
        let dt = parse_iso_datetime("2026-07-25T15:30").unwrap();
        assert_eq!(dt.hour(), 15);
        assert_eq!(dt.minute(), 30);
        assert_eq!(dt.second(), 0);
    }

    #[test]
    fn parse_iso_datetime_date_only_defaults_to_midnight() {
        let dt = parse_iso_datetime("2026-07-25").unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 7);
        assert_eq!(dt.day(), 25);
        assert_eq!(dt.hour(), 0);
        assert_eq!(dt.minute(), 0);
    }

    #[test]
    fn parse_iso_datetime_trims_whitespace() {
        let dt = parse_iso_datetime("  2026-07-25T15:30:00  ").unwrap();
        assert_eq!(dt.hour(), 15);
    }

    #[test]
    fn parse_iso_datetime_rejects_empty() {
        assert_eq!(parse_iso_datetime(""), Err(TimeParseError::Empty));
        assert_eq!(parse_iso_datetime("   "), Err(TimeParseError::Empty));
    }

    #[test]
    fn parse_iso_datetime_rejects_too_long() {
        let long = "a".repeat(MAX_TIME_EXPRESSION_CHARS + 1);
        assert_eq!(parse_iso_datetime(&long), Err(TimeParseError::TooLong));
    }

    #[test]
    fn parse_iso_datetime_rejects_natural_language() {
        assert!(matches!(
            parse_iso_datetime("明天下午3点"),
            Err(TimeParseError::InvalidFormat(_))
        ));
    }

    #[test]
    fn parse_iso_datetime_rejects_invalid_format() {
        assert!(matches!(
            parse_iso_datetime("2026/07/25"),
            Err(TimeParseError::InvalidFormat(_))
        ));
    }

    // ===== naive_to_unix =====

    #[test]
    fn naive_to_unix_subtracts_offset() {
        let dt = parse_iso_datetime("2026-07-25T15:00:00").unwrap();
        let utc_unix = naive_to_unix(dt, 0);
        let shanghai_unix = naive_to_unix(dt, 28_800);
        // UTC+8 的 Unix 秒 = UTC 时间戳 - 28800
        assert_eq!(shanghai_unix, utc_unix - 28_800);
    }

    #[test]
    fn naive_to_unix_falls_back_to_utc_for_invalid_offset() {
        let dt = parse_iso_datetime("2026-07-25T15:00:00").unwrap();
        let utc_unix = naive_to_unix(dt, 0);
        let invalid = naive_to_unix(dt, 99_999_999);
        assert_eq!(invalid, utc_unix);
    }

    #[test]
    fn naive_to_unix_handles_negative_offset() {
        let dt = parse_iso_datetime("2026-07-25T15:00:00").unwrap();
        let utc_unix = naive_to_unix(dt, 0);
        let ny_unix = naive_to_unix(dt, -18_000);
        assert_eq!(ny_unix, utc_unix + 18_000);
    }

    // ===== parse_common_timezone_offset_secs =====

    #[test]
    fn shanghai_offset_is_utc_plus_8() {
        assert_eq!(
            parse_common_timezone_offset_secs("Asia/Shanghai"),
            Some(28_800)
        );
    }

    #[test]
    fn tokyo_offset_is_utc_plus_9() {
        assert_eq!(
            parse_common_timezone_offset_secs("Asia/Tokyo"),
            Some(32_400)
        );
    }

    #[test]
    fn utc_offset_is_zero() {
        assert_eq!(parse_common_timezone_offset_secs("UTC"), Some(0));
        assert_eq!(parse_common_timezone_offset_secs("Z"), Some(0));
    }

    #[test]
    fn new_york_offset_is_utc_minus_5() {
        assert_eq!(
            parse_common_timezone_offset_secs("America/New_York"),
            Some(-18_000)
        );
    }

    #[test]
    fn unknown_timezone_returns_none() {
        assert_eq!(parse_common_timezone_offset_secs("Mars/Olympus"), None);
    }

    #[test]
    fn overly_long_timezone_name_returns_none() {
        let long = "X".repeat(MAX_TIMEZONE_NAME_BYTES + 1);
        assert_eq!(parse_common_timezone_offset_secs(&long), None);
    }

    #[test]
    fn timezone_name_is_trimmed() {
        assert_eq!(
            parse_common_timezone_offset_secs("  Asia/Shanghai  "),
            Some(28_800)
        );
    }

    // ===== parse_datetime_with_timezone =====

    #[test]
    fn parse_datetime_with_shanghai_timezone() {
        let unix = parse_datetime_with_timezone("2026-07-25T15:00:00", "Asia/Shanghai").unwrap();
        let utc_unix = parse_datetime_with_timezone("2026-07-25T15:00:00", "UTC").unwrap();
        assert_eq!(unix, utc_unix - 28_800);
    }

    #[test]
    fn parse_datetime_with_unknown_timezone_errors() {
        let result = parse_datetime_with_timezone("2026-07-25T15:00:00", "Mars/Olympus");
        assert!(matches!(result, Err(TimeParseError::UnknownTimezone(_))));
    }

    #[test]
    fn parse_datetime_with_invalid_format_errors() {
        let result = parse_datetime_with_timezone("明天", "Asia/Shanghai");
        assert!(matches!(result, Err(TimeParseError::InvalidFormat(_))));
    }

    // ===== SystemClock =====

    #[test]
    fn system_clock_returns_positive_timestamp() {
        let clock = SystemClock;
        let now = clock.now_unix_secs();
        // 2026 年的时间戳远大于 0
        assert!(now > 1_700_000_000);
    }
}
