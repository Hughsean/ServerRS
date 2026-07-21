use super::graph::{EffectError, EffectErrorKind, NodeError, NodeErrorKind};
use crate::shared::error::AppError;

pub(crate) fn node_error_from_application(error: AppError) -> NodeError {
    let kind = match &error {
        AppError::Infrastructure(_) => NodeErrorKind::Transient,
        AppError::Validation(_)
        | AppError::Unauthorized
        | AppError::Forbidden(_)
        | AppError::NotFound(_)
        | AppError::Conflict(_)
        | AppError::Internal(_)
        | AppError::NotImplemented(_)
        | AppError::Gone(_) => NodeErrorKind::Permanent,
    };
    NodeError::with_source(kind, error)
}

pub(crate) fn effect_error_from_application(error: AppError) -> EffectError {
    let kind = match &error {
        AppError::Infrastructure(_) => EffectErrorKind::Transient,
        AppError::Validation(_)
        | AppError::Unauthorized
        | AppError::Forbidden(_)
        | AppError::NotFound(_)
        | AppError::Conflict(_)
        | AppError::Internal(_)
        | AppError::NotImplemented(_)
        | AppError::Gone(_) => EffectErrorKind::Permanent,
    };
    EffectError::with_source(kind, error)
}
