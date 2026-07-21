//! Transitional host re-export. Domain code is owned by `digital-human`.

pub use digital_human::domain::*;

#[cfg(feature = "qq_bot")]
pub use qqbot::domain::qq_bot;
