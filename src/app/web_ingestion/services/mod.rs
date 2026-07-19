//! Stateless helper services for the web-ingestion pipeline.
//!
//! Each service owns one concern so handlers stay thin (task-book §4.5–§4.8):
//!   - `due_url_selector`   — which URLs are due for crawl
//!   - `run_profile`        — versioned run identity (real embedding model etc.)
//!   - `run_key_builder`    — run_key / version_key / content_key construction
//!   - `artifact_service`   — read/write run artifacts (no large text in outbox)
//!   - `quality_result`     — stable, machine-readable quality-gate result
//!   - `terminal_events`    — emit terminal / next-stage outbox events
//!   - `html_cleaner`       — wrapper over the extractor for boilerplate removal

pub mod artifact_service;
pub mod due_url_selector;
pub mod html_cleaner;
pub mod quality_result;
pub mod run_key_builder;
pub mod run_profile;
pub mod terminal_events;
pub mod vector_activation_service;
