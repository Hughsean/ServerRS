//! Transitional host re-export. Business adapters are owned by `digital-human`.

pub use digital_human::infra::*;

#[cfg(feature = "qq_bot")]
pub use qqbot::infra::qq_bot;
