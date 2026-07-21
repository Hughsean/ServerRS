//! Transitional host re-export. Business code is owned by `digital-human`.

pub use digital_human::app::*;

#[cfg(feature = "qq_bot")]
pub use qqbot::app::qq_bot;
