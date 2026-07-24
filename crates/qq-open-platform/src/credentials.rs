use std::fmt;
use std::sync::Arc;

use thiserror::Error;

#[derive(Clone)]
pub struct QqBotCredentials {
    app_id: Arc<str>,
    client_secret: Arc<str>,
}

impl QqBotCredentials {
    pub fn new(
        app_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self, QqCredentialsError> {
        let app_id = app_id.into();
        let client_secret = client_secret.into();
        if app_id.trim().is_empty() || app_id.len() > 191 {
            return Err(QqCredentialsError::InvalidAppId);
        }
        if client_secret.trim().is_empty() || client_secret.len() > 1024 {
            return Err(QqCredentialsError::InvalidSecret);
        }
        Ok(Self {
            app_id: Arc::from(app_id),
            client_secret: Arc::from(client_secret),
        })
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub(crate) fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

impl fmt::Debug for QqBotCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QqBotCredentials")
            .field("app_id", &self.app_id)
            .field("client_secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QqCredentialsError {
    #[error("QQ Bot AppID must contain 1..=191 bytes")]
    InvalidAppId,
    #[error("QQ Bot client secret must contain 1..=1024 bytes")]
    InvalidSecret,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_exposes_secret() {
        let credentials = QqBotCredentials::new("app", "very-secret").unwrap();
        let rendered = format!("{credentials:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("very-secret"));
    }
}
