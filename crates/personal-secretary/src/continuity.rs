use serde::{Deserialize, Serialize};
use thiserror::Error;

macro_rules! persisted_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ContinuityIdentityError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ContinuityIdentityError::EmptyField($field));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

persisted_id!(ConnectionEpochId, "connection_epoch_id");
persisted_id!(IngestionGapId, "ingestion_gap_id");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionEpochStatus {
    Connecting,
    Connected,
    Disconnected,
    Shutdown,
    ConnectFailed,
}

impl ConnectionEpochStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Shutdown => "shutdown",
            Self::ConnectFailed => "connect_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionCursorScope {
    Account,
    Conversation,
}

impl IngestionCursorScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Conversation => "conversation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionGapStatus {
    Uncertain,
    Backfilling,
    VerifiedComplete,
    Unrecoverable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionGapReason {
    QueueOverflow,
    DatabaseUnavailable,
    HistoryUnprovable,
    InvalidEvent,
}

impl IngestionGapReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueueOverflow => "queue_overflow",
            Self::DatabaseUnavailable => "database_unavailable",
            Self::HistoryUnprovable => "history_unprovable",
            Self::InvalidEvent => "invalid_event",
        }
    }
}

impl IngestionGapStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uncertain => "uncertain",
            Self::Backfilling => "backfilling",
            Self::VerifiedComplete => "verified_complete",
            Self::Unrecoverable => "unrecoverable",
        }
    }
}

/// 一次 NapCat/开放平台传输连接结束的可审计原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionEndReason {
    RemoteClosed,
    TransportError,
    ProcessShutdown,
    ObserverRejected,
}

impl ConnectionEndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RemoteClosed => "remote_closed",
            Self::TransportError => "transport_error",
            Self::ProcessShutdown => "process_shutdown",
            Self::ObserverRejected => "observer_rejected",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContinuityIdentityError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_cannot_be_claimed_complete_by_a_transport_reconnect() {
        assert_ne!(
            IngestionGapStatus::Uncertain.as_str(),
            IngestionGapStatus::VerifiedComplete.as_str()
        );
        assert_eq!(
            ConnectionEpochStatus::Connected.as_str(),
            "connected",
            "transport health and history completeness are separate states"
        );
    }
}
