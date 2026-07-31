//! Owner 响应草稿与片段：正文有界，来源失效时可标记失效。
//!
//! 约束 7：只保存有界摘录；限制单条/总字符数；序列化字节数（64KB）由应用层验证。
//! 来源删除/过期/不可见时调用 `invalidate_if_references` 标记失效，由上层重新脱敏或重建。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::SourceEventId;

use super::action::{SecretaryActionReceipt, SecretaryToolKind};
use super::validation::{SecretaryAgentRuntimeError, validate_response_draft};

use crate::NotificationPolicyResponseArtifact;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEventRef {
    pub source_event_id: SourceEventId,
    pub summary: String,
}

/// Owner 响应草稿的单个片段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseSegment {
    /// 来自检索结果的有界正文摘录。envelope_only 内容此处为空字符串。
    Excerpt {
        source_event_id: SourceEventId,
        text: String,
    },
    /// Planner 生成的自然语言摘要。
    Summary { text: String },
}

impl ResponseSegment {
    /// 单条片段正文的字符数。
    fn char_count(&self) -> usize {
        match self {
            Self::Excerpt { text, .. } | Self::Summary { text } => text.chars().count(),
        }
    }

    /// 该片段引用的 source_event_id（Summary 无）。
    pub fn source_event_id(&self) -> Option<&SourceEventId> {
        match self {
            Self::Excerpt {
                source_event_id, ..
            } => Some(source_event_id),
            Self::Summary { .. } => None,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Excerpt { text, .. } | Self::Summary { text } => text,
        }
    }
}

/// Owner 收到的响应草稿。正文有界，来源失效时可标记失效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerResponseDraft {
    segments: Vec<ResponseSegment>,
    /// 草稿依据的来源事件 ID（含 excerpts 引用 + 额外 evidence）。
    source_event_ids: Vec<SourceEventId>,
    created_at_unix_secs: i64,
    /// 是否已因来源失效而标记失效。私有，只能通过 `invalidate_if_references` 修改。
    invalidated: bool,
}

impl OwnerResponseDraft {
    pub fn new(
        segments: Vec<ResponseSegment>,
        source_event_ids: Vec<SourceEventId>,
        created_at_unix_secs: i64,
    ) -> Result<Self, SecretaryAgentRuntimeError> {
        let draft = Self {
            segments,
            source_event_ids,
            created_at_unix_secs,
            invalidated: false,
        };
        validate_response_draft(&draft)?;
        Ok(draft)
    }

    pub fn segments(&self) -> &[ResponseSegment] {
        &self.segments
    }

    pub fn source_event_ids(&self) -> &[SourceEventId] {
        &self.source_event_ids
    }

    pub fn created_at_unix_secs(&self) -> i64 {
        self.created_at_unix_secs
    }

    pub fn invalidated(&self) -> bool {
        self.invalidated
    }

    /// 检查草稿是否引用了已移除的来源事件，若是则标记失效。
    /// 返回是否发生了失效转换（已失效时再次调用返回 false）。
    pub fn invalidate_if_references(&mut self, removed_event_ids: &[SourceEventId]) -> bool {
        if self.invalidated {
            return false;
        }
        let removed: HashSet<&str> = removed_event_ids
            .iter()
            .map(SourceEventId::as_str)
            .collect();
        let references_removed = self
            .source_event_ids
            .iter()
            .any(|id| removed.contains(id.as_str()))
            || self.segments.iter().any(|seg| {
                seg.source_event_id()
                    .is_some_and(|id| removed.contains(id.as_str()))
            });
        if references_removed {
            self.invalidated = true;
            return true;
        }
        false
    }

    /// 草稿正文总字符数（所有 segments 之和）。
    pub fn total_char_count(&self) -> usize {
        self.segments.iter().map(|s| s.char_count()).sum()
    }
}

// ===== 策略响应工件收口 =====

/// 判断是否为通知策略 Action（覆盖全部 11 种）。
fn is_notification_policy_action(kind: &SecretaryToolKind) -> bool {
    use SecretaryToolKind::*;
    matches!(
        kind,
        ListNotificationPolicies
            | ExplainNotificationDecision
            | SetAccountDefaultNotificationMode
            | SetConversationNotificationMode
            | SetQuietHours
            | SetImportantContact
            | SetNotificationCategoryImportance
            | RecordNotificationFeedback
            | CreateSimilarNotificationRule
            | DisableNotificationPolicy
            | SetAutomaticReplyDeniedForContact
    )
}

/// BuildResponseNode 与 PlannerUseCase::build_response_draft 的唯一共享响应构造入口。
///
/// 根据 receipt 中的 `tool_kind` 区分策略 Action 与非策略 Action：
/// - 策略 Action：解析 `result_ref` 为类型化工件，渲染有界中文响应。
/// - 非策略 Action：保持原有 `result_ref` 摘要行为。
/// - 解析失败时返回安全降级响应，不回显原始 JSON。
pub fn build_action_response_draft(
    receipt: Option<&SecretaryActionReceipt>,
    source_event_ids: Vec<SourceEventId>,
    created_at_unix_secs: i64,
) -> Result<OwnerResponseDraft, SecretaryAgentRuntimeError> {
    let is_policy = receipt
        .and_then(|r| r.tool_kind.as_ref())
        .is_some_and(is_notification_policy_action);
    let segments = match receipt {
        Some(receipt) => match &receipt.tool_kind {
            Some(kind) if is_notification_policy_action(kind) => {
                build_policy_response(kind, &receipt.result_ref)?
            }
            _ => {
                // 非策略 Action 或历史兼容（无 tool_kind）：保持原有语义
                let text = format!("动作已执行：{}", receipt.result_ref);
                vec![ResponseSegment::Summary { text }]
            }
        },
        None => vec![ResponseSegment::Summary {
            text: "无需执行动作".into(),
        }],
    };
    let draft = OwnerResponseDraft::new(segments, source_event_ids, created_at_unix_secs)?;
    // 策略响应序列化上限 8 KiB：source_event_ids 数量与单 ID 长度均无硬上限，
    // 因此仅靠 segment 字符数不能推出完整序列化体积。
    if is_policy {
        let serialized = serde_json::to_vec(&draft).map_err(|_| {
            SecretaryAgentRuntimeError::InvalidResponseDraft("策略响应草稿序列化失败".into())
        })?;
        if serialized.len() > 8 * 1024 {
            return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
                "策略响应草稿序列化后超过 8 KiB 上限".into(),
            ));
        }
    }
    Ok(draft)
}

/// 根据策略 Action 类型解析 `result_ref` 并渲染中文响应。
fn build_policy_response(
    kind: &SecretaryToolKind,
    result_ref: &str,
) -> Result<Vec<ResponseSegment>, SecretaryAgentRuntimeError> {
    use SecretaryToolKind::*;
    match kind {
        ListNotificationPolicies => {
            match serde_json::from_str::<Vec<NotificationPolicyResponseArtifact>>(result_ref) {
                Ok(artifacts) => Ok(build_list_segments(&artifacts)),
                Err(_) => Ok(fallback_segments()),
            }
        }
        ExplainNotificationDecision => {
            match serde_json::from_str::<Option<NotificationPolicyResponseArtifact>>(result_ref) {
                Ok(artifact) => Ok(build_explain_segments(&artifact)),
                Err(_) => Ok(fallback_segments()),
            }
        }
        // 可变策略 Action（含 RecordNotificationFeedback）：单个 Artifact
        _ => match serde_json::from_str::<NotificationPolicyResponseArtifact>(result_ref) {
            Ok(artifact) => Ok(build_mutation_segments(kind, &artifact)),
            Err(_) => Ok(fallback_segments()),
        },
    }
}

/// List 响应：中文规则列表，空列表给出明确提示，超长安全截断。
/// 单段 ≤1000 字符，超出时截断并附加省略提示。
fn build_list_segments(artifacts: &[NotificationPolicyResponseArtifact]) -> Vec<ResponseSegment> {
    if artifacts.is_empty() {
        return vec![ResponseSegment::Summary {
            text: "当前没有已配置的提醒规则。".into(),
        }];
    }
    let header = "当前提醒规则：\n";
    let truncation_note = "其余规则已省略，请缩小查询范围。";
    let mut lines: Vec<String> = Vec::new();
    let mut total = header.chars().count();
    let mut truncated = false;

    for (i, a) in artifacts.iter().enumerate() {
        let scope_label = scope_prefix(&a.scope);
        let status_cn = status_cn(&a.status);
        let line = format!(
            "{}. {} {}，状态 {}，版本 {}",
            i + 1,
            scope_label,
            a.scope,
            status_cn,
            a.typed_reason,
        );
        let line_len = line.chars().count();
        let projected = total + line_len + 1;
        // 预留截断提示空间
        if projected + truncation_note.chars().count() > 1_000 && i > 0 {
            truncated = true;
            break;
        }
        total += line_len + 1; // line + newline
        lines.push(line);
    }
    let mut text = format!("{header}{}", lines.join("\n"));
    if truncated {
        text.push_str(truncation_note);
    }
    // UTF-8 安全硬截断
    if text.chars().count() > 1_000 {
        text = text.chars().take(1_000).collect();
    }
    vec![ResponseSegment::Summary { text }]
}

/// Explain 响应：展示决策结果、原因与审计引用。
fn build_explain_segments(
    artifact: &Option<NotificationPolicyResponseArtifact>,
) -> Vec<ResponseSegment> {
    match artifact {
        None => vec![ResponseSegment::Summary {
            text: "未找到该提醒决策，或该决策不属于当前账号。".into(),
        }],
        Some(a) => {
            let reason_text = translate_reason(&a.typed_reason);
            let outcome = outcome_cn(&a.status);
            let text = format!(
                "该提醒的处理结果为：{}。\n原因：{}。\n审计引用：{}",
                outcome, reason_text, a.audit_reference,
            );
            vec![ResponseSegment::Summary { text }]
        }
    }
}

/// Mutation 响应：根据 Action 类型返回确定性中文文案。
fn build_mutation_segments(
    kind: &SecretaryToolKind,
    _artifact: &NotificationPolicyResponseArtifact,
) -> Vec<ResponseSegment> {
    use SecretaryToolKind::*;
    let text = match kind {
        RecordNotificationFeedback => "你的反馈已记录。",
        DisableNotificationPolicy => "提醒规则已停用。",
        SetAutomaticReplyDeniedForContact => "已记录该联系人的自动回复禁用策略。",
        _ => "提醒规则已更新。",
    };
    vec![ResponseSegment::Summary { text: text.into() }]
}

/// 解析失败时的安全降级响应：不泄漏原始 `result_ref`。
fn fallback_segments() -> Vec<ResponseSegment> {
    vec![ResponseSegment::Summary {
        text: "操作结果已经记录，但无法安全展示详细结果，请通过审计记录查询。".into(),
    }]
}

/// 根据 scope 前缀返回中文范围标签。
fn scope_prefix(scope: &str) -> &str {
    if scope.starts_with("conversation") {
        "会话范围"
    } else if scope.starts_with("actor") {
        "联系人范围"
    } else if scope.starts_with("category") {
        "类别范围"
    } else if scope.starts_with("account") {
        "账号范围"
    } else {
        "范围"
    }
}

/// 策略状态中文翻译。穷举已知值；未知值统一返回安全文案。
fn status_cn(status: &str) -> &str {
    match status {
        "rule" => "规则",
        "tombstone" => "已停用",
        "remind" => "提醒",
        "delay" => "延迟",
        "suppress" => "静默",
        "recorded" => "已记录",
        _ => "已配置",
    }
}

/// 决策结果中文翻译。穷举已知 NotificationOutcome，未知值不原样输出数据库字符串。
fn outcome_cn(outcome: &str) -> &str {
    match outcome {
        "remind" => "提醒",
        "delay" => "延迟",
        "suppress" => "静默",
        "candidate_expired" => "候选已过期",
        "evaluation_failed_terminal" => "评估失败",
        "delivery_window_expired" => "投递窗口已过期",
        "schedule_time_ambiguous" => "时区歧义",
        _ => "策略判定",
    }
}

/// 决策原因中文翻译。穷举已知 DecisionReason，未知值返回安全固定文案。
/// `schedule_time_ambiguous` 必须正确解释为时区歧义，不得翻译为系统故障。
fn translate_reason(reason: &str) -> &str {
    // 包含检查优先：部分实现可能附加额外上下文字段
    if reason.contains("schedule_time_ambiguous") {
        return "规则时间存在时区歧义，需要重新确认静默时段。";
    }
    match reason {
        "quiet_hours" => "当前处于静默时段。",
        "fully_silent" => "该会话已设为完全静默。",
        "account_default_policy" => "根据账号默认策略。",
        "conversation_policy" => "根据会话策略。",
        "contact_policy" => "根据联系人策略。",
        "category_policy" => "根据类别策略。",
        "candidate_expired" => "候选已过期。",
        "delivery_window_expired" => "投递窗口已过期。",
        "evaluation_failed_terminal" => "策略评估失败。",
        "invalid_quiet_hours" => "静默时段配置无效。",
        "feedback_recorded" => "反馈已记录。",
        "policy_written" => "策略已写入。",
        _ => "策略引擎判定。",
    }
}
