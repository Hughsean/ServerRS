//! Owner 日程、任务与提醒的协议无关领域模型。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{SourceAccountRef, SourceEventId};

const MAX_TITLE_CHARS: usize = 500;
const MAX_TIMEZONE_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgendaItemId(String);

impl AgendaItemId {
    pub fn new(value: impl Into<String>) -> Result<Self, AgendaError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 36 {
            return Err(AgendaError::Invalid(
                "agenda item id must contain 1..=36 bytes".into(),
            ));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgendaItemKind {
    Schedule,
    Task,
    Reminder,
}

impl AgendaItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Task => "task",
            Self::Reminder => "reminder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgendaItemStatus {
    Scheduled,
    Completed,
    Cancelled,
}

impl AgendaItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgendaItem {
    pub item_id: AgendaItemId,
    pub account: SourceAccountRef,
    pub kind: AgendaItemKind,
    pub title: String,
    pub scheduled_at_unix_secs: Option<i64>,
    pub timezone: String,
    pub status: AgendaItemStatus,
    pub version: u64,
    pub created_by_command: SourceEventId,
    pub current_version_command: SourceEventId,
    pub created_at_unix_secs: i64,
    pub updated_at_unix_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgendaMutation {
    Create {
        kind: AgendaItemKind,
        title: String,
        scheduled_at_unix_secs: Option<i64>,
        timezone: String,
    },
    Reschedule {
        item_id: AgendaItemId,
        expected_version: u64,
        scheduled_at_unix_secs: i64,
        timezone: String,
    },
    Complete {
        item_id: AgendaItemId,
        expected_version: u64,
    },
    Cancel {
        item_id: AgendaItemId,
        expected_version: u64,
    },
    Snooze {
        item_id: AgendaItemId,
        expected_version: u64,
        scheduled_at_unix_secs: i64,
        timezone: String,
    },
}

impl AgendaMutation {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Reschedule { .. } => "reschedule",
            Self::Complete { .. } => "complete",
            Self::Cancel { .. } => "cancel",
            Self::Snooze { .. } => "snooze",
        }
    }
}

pub fn validate_agenda_mutation(
    mutation: &AgendaMutation,
    now_unix_secs: i64,
) -> Result<(), AgendaError> {
    let validate_time = |value: i64| {
        if value <= now_unix_secs {
            Err(AgendaError::Invalid(
                "agenda time must be in the future".into(),
            ))
        } else {
            Ok(())
        }
    };
    let validate_timezone = |value: &str| {
        if value.trim().is_empty()
            || value.len() > MAX_TIMEZONE_BYTES
            || value.parse::<chrono_tz::Tz>().is_err()
        {
            Err(AgendaError::Invalid(
                "agenda timezone must be a valid IANA name".into(),
            ))
        } else {
            Ok(())
        }
    };
    match mutation {
        AgendaMutation::Create {
            title,
            scheduled_at_unix_secs,
            timezone,
            ..
        } => {
            if title.trim().is_empty() || title.chars().count() > MAX_TITLE_CHARS {
                return Err(AgendaError::Invalid(
                    "agenda title must contain 1..=500 characters".into(),
                ));
            }
            validate_timezone(timezone)?;
            if let Some(value) = scheduled_at_unix_secs {
                validate_time(*value)?;
            }
        }
        AgendaMutation::Reschedule {
            scheduled_at_unix_secs,
            timezone,
            ..
        }
        | AgendaMutation::Snooze {
            scheduled_at_unix_secs,
            timezone,
            ..
        } => {
            validate_timezone(timezone)?;
            validate_time(*scheduled_at_unix_secs)?;
        }
        AgendaMutation::Complete { .. } | AgendaMutation::Cancel { .. } => {}
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum AgendaError {
    #[error("invalid agenda data: {0}")]
    Invalid(String),
    #[error("agenda item was not found")]
    NotFound,
    #[error("agenda item version conflict")]
    VersionConflict,
    #[error("agenda store failed: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_or_past_time_never_validates_as_a_write() {
        let mutation = AgendaMutation::Create {
            kind: AgendaItemKind::Reminder,
            title: "提醒".into(),
            scheduled_at_unix_secs: Some(99),
            timezone: "Asia/Shanghai".into(),
        };
        assert!(validate_agenda_mutation(&mutation, 100).is_err());
    }

    #[test]
    fn invalid_iana_timezone_is_rejected() {
        let mutation = AgendaMutation::Create {
            kind: AgendaItemKind::Task,
            title: "报价".into(),
            scheduled_at_unix_secs: Some(200),
            timezone: "UTC+8".into(),
        };
        assert!(validate_agenda_mutation(&mutation, 100).is_err());
    }
}
