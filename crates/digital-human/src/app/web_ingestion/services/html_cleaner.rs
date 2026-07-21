//! HTML cleaning service (task-book §4.5, §7.1).
//!
//! Thin wrapper over `extractor::extract_clean_text` so the cleaning concern has
//! one entry point. Returns `(title, clean_text)`.

use crate::app::web_ingestion::extractor;

/// Minimum useful clean-text length (Unicode chars). Below this, the page is
/// rejected before any LLM call (§7.1: "clean_text too short → no LLM").
pub const MIN_CLEAN_CHARS: usize = 100;

/// Clean raw HTML into `(title, clean_text)`.
pub fn clean(html: &str) -> (String, String) {
    extractor::extract_clean_text(html)
}

/// Whether the cleaned text is too short to be worth distilling.
pub fn is_too_short(clean_text: &str) -> bool {
    clean_text.chars().count() < MIN_CLEAN_CHARS
}
