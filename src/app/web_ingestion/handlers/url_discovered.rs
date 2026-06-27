//! `UrlDiscovered` handler (task-book §5.3, §5.4, §5.6, §5.8).
//!
//! - DB `web_source_urls.url` is authoritative for fetching; payload.url
//!   mismatch FAILS (not warn).
//! - source.allowed_domains is enforced end-to-end via the fetcher.
//! - run_key uses the REAL embedding model / prompt / chunker / pipeline
//!   versions (RunProfile), never a placeholder.
//! - A run_key hit does NOT blindly skip: the existing run's state is inspected
//!   and the pipeline is resumed / confirmed idempotent.

use chrono::Utc;

use crate::app::web_ingestion::event_types::{aggregate, event as ev};
use crate::app::web_ingestion::hash;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::{
    run_key_builder, run_profile::RunProfile, terminal_events,
};
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{
    DomainEvent, NewAuditLog, NewIngestionRun, NewOutboxEvent, NewWebPage,
};
use crate::domain::web_ingestion::status::{audit_action, run_stage, run_status};

pub async fn handle(event: &DomainEvent, ctx: &PipelineContext) -> Result<(), WebIngestionError> {
    let payload = &event.payload;

    let source_url_id = payload["source_url_id"]
        .as_u64()
        .filter(|&v| v > 0)
        .ok_or_else(|| {
            WebIngestionError::Internal("UrlDiscovered: missing/invalid source_url_id".into())
        })?;
    let payload_url = payload["url"].as_str().filter(|s| !s.is_empty());

    // ── Resolve source_url (DB authoritative) ──────────────────────────────
    let url_rec = ctx
        .source_url_repo
        .find_by_id(source_url_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_source_url".into(),
            id: source_url_id,
        })?;

    // Disabled / deleted source_url is not processed (§16.1 #12).
    if !url_rec.enabled || url_rec.deleted_at.is_some() {
        tracing::info!(
            source_url_id,
            "UrlDiscovered: source_url disabled/deleted — skipping"
        );
        return Ok(());
    }

    let effective_source_id = url_rec.source_id;

    // payload.source_id mismatch MUST fail (§5.3 #7, hard constraint).
    if let Some(payload_src) = payload["source_id"].as_u64() {
        if payload_src != effective_source_id {
            return Err(WebIngestionError::Internal(format!(
                "UrlDiscovered: payload.source_id {payload_src} != DB source_url.source_id {effective_source_id}"
            )));
        }
    }

    // payload.url mismatch MUST fail (§5.3 #4 — not warn).
    let db_url = url_rec.url.as_str();
    if let Some(p_url) = payload_url {
        if p_url != db_url {
            return Err(WebIngestionError::Internal(format!(
                "UrlDiscovered: payload.url '{p_url}' != DB source_url.url '{db_url}'"
            )));
        }
    }

    // ── Source for allowed_domains + approval ──────────────────────────────
    let source = ctx
        .source_repo
        .find_by_id(effective_source_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_source".into(),
            id: effective_source_id,
        })?;
    if source.deleted_at.is_some() || !source.enabled {
        tracing::info!(
            effective_source_id,
            "UrlDiscovered: source disabled/deleted — skipping"
        );
        return Ok(());
    }

    let allowed_domains: Option<Vec<String>> = source.allowed_domains.as_ref().and_then(|v| {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
    });

    // ── Fetch using the DB-authoritative URL (§5.3 #3) ─────────────────────
    let fetch_result = ctx
        .fetcher
        .fetch(db_url, allowed_domains.as_deref())
        .await?;
    let ch = hash::content_hash(&fetch_result.body_text);
    let url_h = hash::url_hash(&fetch_result.final_url);
    tracing::trace!(
        source_id = effective_source_id,
        source_url_id,
        url = %db_url,
        final_url = %fetch_result.final_url,
        body_bytes = fetch_result.body.len(),
        body_chars = fetch_result.body_text.chars().count(),
        content_hash = %ch,
        url_hash = %url_h,
        "UrlDiscovered: fetch completed"
    );

    // ── Upsert web_page — url column written from DB url (§5.3 #5) ──────────
    let page = match ctx
        .page_repo
        .find_by_source_and_hash(effective_source_id, &url_h)
        .await?
    {
        Some(p) => p,
        None => {
            ctx.page_repo
                .upsert(NewWebPage {
                    source_id: effective_source_id,
                    source_url_id: Some(source_url_id),
                    url: db_url.to_string(),
                    canonical_url: Some(fetch_result.final_url.clone()),
                    url_hash: url_h.clone(),
                })
                .await?
        }
    };

    // page/source_url/source relationship consistency (§5.3 #6).
    if page.source_id != effective_source_id {
        return Err(WebIngestionError::Internal(format!(
            "UrlDiscovered: page.source_id {} != source_url.source_id {effective_source_id}",
            page.source_id
        )));
    }

    // ── Build the real run profile (§5.6) ──────────────────────────────────
    let profile = RunProfile::from_context(ctx)
        .map_err(|e| WebIngestionError::Internal(format!("UrlDiscovered: {e}")))?;

    // ── Content-unchanged check ────────────────────────────────────────────
    if let Some(existing_run_id) = unchanged_run_id(
        url_rec.last_content_hash.as_deref(),
        page.latest_content_hash.as_deref(),
        page.latest_success_run_id,
        &ch,
    ) {
        tracing::trace!(
            source_id = effective_source_id,
            source_url_id,
            page_id = page.id,
            existing_run_id,
            content_hash = %ch,
            "UrlDiscovered: content unchanged; emitting skipped"
        );
        ctx.source_url_repo
            .mark_crawled(source_url_id, &ch, Utc::now())
            .await?;
        ctx.page_repo
            .mark_fetched(page.id, &ch, existing_run_id, Utc::now())
            .await?;
        ctx.audit_repo
            .insert(NewAuditLog {
                source_id: Some(effective_source_id),
                source_url_id: Some(source_url_id),
                page_id: Some(page.id),
                run_id: None,
                publish_record_id: None,
                action: audit_action::CONTENT_UNCHANGED.into(),
                status: "info".into(),
                message: "content unchanged".into(),
                metadata: None,
            })
            .await?;
        terminal_events::emit_skipped(&ctx.outbox_repo, source_url_id, &url_h, "unchanged").await?;
        return Ok(());
    }

    let rk = run_key_builder::build_run_key(effective_source_id, page.id, &ch, &profile);
    let vk = run_key_builder::build_version_key(&rk);
    let ck = run_key_builder::build_content_key(effective_source_id, page.id, &ch);

    // ── run_key idempotency with RESUME (§5.8 special requirement) ──────────
    if let Some(existing) = ctx.run_repo.find_by_run_key(&rk).await? {
        tracing::trace!(
            run_id = existing.id,
            source_id = effective_source_id,
            source_url_id,
            page_id = page.id,
            stage = %existing.stage,
            status = %existing.status,
            "UrlDiscovered: existing run found; checking resume"
        );
        let outcome = resume_existing_run(ctx, &existing, &ch, &fetch_result.body_text).await?;
        if outcome == ResumeOutcome::FetchedOrLater {
            let fetched_at = Utc::now();
            ctx.page_repo
                .mark_fetched(page.id, &ch, existing.id, fetched_at)
                .await?;
            ctx.source_url_repo
                .mark_crawled(source_url_id, &ch, fetched_at)
                .await?;
        }
        return Ok(());
    }

    // ── Create a fresh ingestion run ───────────────────────────────────────
    let run = ctx
        .run_repo
        .insert(NewIngestionRun {
            source_id: effective_source_id,
            source_url_id: Some(source_url_id),
            crawl_job_id: Some(event.aggregate_id),
            page_id: page.id,
            content_hash: ch.clone(),
            content_key: ck,
            run_key: rk.clone(),
            version_key: vk.clone(),
        })
        .await?;

    // Persist the run's profile (embedding provider/model/dimension) for audit
    // and downstream stages.
    ctx.run_repo
        .update_embedding_info(
            run.id,
            &profile.embedding_provider,
            &profile.embedding_model,
            profile.embedding_dimension as u32,
        )
        .await?;
    tracing::trace!(
        run_id = run.id,
        source_id = effective_source_id,
        source_url_id,
        page_id = page.id,
        content_hash = %ch,
        run_key = %rk,
        version_key = %vk,
        embedding_provider = %profile.embedding_provider,
        embedding_model = %profile.embedding_model,
        embedding_dimension = profile.embedding_dimension,
        "UrlDiscovered: ingestion run created"
    );

    // pending/pending → running/fetching
    if !sm::transition(
        &ctx.run_repo,
        run.id,
        run_status::PENDING,
        run_stage::PENDING,
        run_status::RUNNING,
        run_stage::FETCHING,
        None,
    )
    .await?
    .applied()
    {
        tracing::info!(
            run_id = run.id,
            "UrlDiscovered: not at pending — concurrent worker"
        );
        return Ok(());
    }
    ctx.run_repo.mark_started(run.id).await?;

    // Persist the fetched body BEFORE marking fetched, so resume always finds it.
    ctx.run_repo
        .update_artifacts(run.id, Some(&fetch_result.body_text), None, None)
        .await?;

    // running/fetching → running/fetched
    if !sm::transition(
        &ctx.run_repo,
        run.id,
        run_status::RUNNING,
        run_stage::FETCHING,
        run_status::RUNNING,
        run_stage::FETCHED,
        None,
    )
    .await?
    .applied()
    {
        tracing::info!(
            run_id = run.id,
            "UrlDiscovered: not at fetching — concurrent worker"
        );
        return Ok(());
    }

    ctx.page_repo
        .mark_fetched(page.id, &ch, run.id, Utc::now())
        .await?;
    ctx.source_url_repo
        .mark_crawled(source_url_id, &ch, Utc::now())
        .await?;

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(effective_source_id),
            source_url_id: Some(source_url_id),
            page_id: Some(page.id),
            run_id: Some(run.id),
            publish_record_id: None,
            action: "fetch_succeeded".into(),
            status: "success".into(),
            message: format!("fetched {} bytes", fetch_result.body.len()),
            metadata: None,
        })
        .await?;

    tracing::trace!(
        run_id = run.id,
        source_id = effective_source_id,
        source_url_id,
        page_id = page.id,
        body_bytes = fetch_result.body.len(),
        body_chars = fetch_result.body_text.chars().count(),
        "UrlDiscovered: fetched body persisted; emitting PageFetched"
    );
    emit_page_fetched(ctx, run.id, &vk, &ch).await
}

/// Resume an existing run found by run_key (§5.8). Rather than blindly skipping,
/// inspect the run's stage:
///   - already fetched or later  → ensure PageFetched exists (emit if missing)
///   - still fetching            → re-persist the body and re-emit
///   - terminal                  → idempotent Ok
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeOutcome {
    FetchedOrLater,
    Terminal,
}

async fn resume_existing_run(
    ctx: &PipelineContext,
    existing: &crate::domain::web_ingestion::repository::KnowledgeIngestionRun,
    content_hash: &str,
    body_text: &str,
) -> Result<ResumeOutcome, WebIngestionError> {
    use crate::domain::web_ingestion::status::is_terminal_run_status;

    if is_terminal_run_status(&existing.status) {
        tracing::info!(run_id = existing.id, status = %existing.status, "UrlDiscovered resume: terminal — idempotent");
        return Ok(ResumeOutcome::Terminal);
    }

    match existing.stage.as_str() {
        run_stage::FETCHING => {
            // Body may not have been persisted — ensure it is, then advance.
            if existing.fetched_body_text.is_none() {
                ctx.run_repo
                    .update_artifacts(existing.id, Some(body_text), None, None)
                    .await?;
            }
            let _ = sm::transition(
                &ctx.run_repo,
                existing.id,
                run_status::RUNNING,
                run_stage::FETCHING,
                run_status::RUNNING,
                run_stage::FETCHED,
                None,
            )
            .await?;
            emit_page_fetched(ctx, existing.id, &existing.version_key, content_hash).await?;
            tracing::trace!(
                run_id = existing.id,
                "UrlDiscovered resume: fetching run advanced; emitted PageFetched"
            );
            Ok(ResumeOutcome::FetchedOrLater)
        }
        run_stage::FETCHED => {
            // Re-emit PageFetched (idempotent via event_key) so the pipeline
            // continues even if the original event was lost.
            emit_page_fetched(ctx, existing.id, &existing.version_key, content_hash).await?;
            tracing::trace!(
                run_id = existing.id,
                "UrlDiscovered resume: fetched run; emitted PageFetched"
            );
            Ok(ResumeOutcome::FetchedOrLater)
        }
        other => {
            tracing::info!(run_id = existing.id, stage = %other, "UrlDiscovered resume: past fetched — idempotent");
            Ok(ResumeOutcome::FetchedOrLater)
        }
    }
}

fn unchanged_run_id(
    source_content_hash: Option<&str>,
    page_content_hash: Option<&str>,
    latest_success_run_id: Option<u64>,
    fetched_content_hash: &str,
) -> Option<u64> {
    if source_content_hash == Some(fetched_content_hash)
        && page_content_hash == Some(fetched_content_hash)
    {
        latest_success_run_id
    } else {
        None
    }
}

async fn emit_page_fetched(
    ctx: &PipelineContext,
    run_id: u64,
    version_key: &str,
    content_hash: &str,
) -> Result<(), WebIngestionError> {
    let event_key = hash::event_key(
        ev::PAGE_FETCHED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run_id,
        run_id,
        version_key,
    );
    ctx.outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: ev::PAGE_FETCHED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run_id,
            payload: serde_json::json!({"run_id": run_id, "content_hash": content_hash}),
            max_retries: 5,
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::unchanged_run_id;

    #[test]
    fn unchanged_requires_source_page_and_run_to_agree() {
        assert_eq!(
            unchanged_run_id(Some("hash"), Some("hash"), Some(42), "hash"),
            Some(42)
        );
    }

    #[test]
    fn source_hash_alone_does_not_skip_stranded_run() {
        assert_eq!(unchanged_run_id(Some("hash"), None, None, "hash"), None);
    }

    #[test]
    fn stale_page_hash_does_not_skip_new_content() {
        assert_eq!(
            unchanged_run_id(Some("new"), Some("old"), Some(42), "new"),
            None
        );
    }
}
