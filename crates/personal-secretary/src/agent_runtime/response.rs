//! Owner 响应草稿与片段：正文有界，来源失效时可标记失效。
//!
//! 约束 7：只保存有界摘录；限制单条/总字符数；序列化字节数（64KB）由应用层验证。
//! 来源删除/过期/不可见时调用 `invalidate_if_references` 标记失效，由上层重新脱敏或重建。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::SourceEventId;

use super::validation::{SecretaryAgentRuntimeError, validate_response_draft};

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
