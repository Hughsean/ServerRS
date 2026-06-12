//! Bootstrap the web ingestion subsystem.
//!
//! Constructs repositories and optional background workers.  Does NOT
//! contain business logic — that lives in `application::web_ingestion`.

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tracing::info;

use crate::bootstrap::tasks::BackgroundTasks;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_store::VectorStore;
use crate::domain::web_ingestion::repository::*;
use crate::infrastructure::web_ingestion::fetcher::WebFetcher;
use crate::infrastructure::web_ingestion::repositories::*;
use crate::shared::config::AppConfig;
use crate::shared::error::AppError;

pub async fn init_web_ingestion(
    config: &AppConfig,
    db: &DatabaseConnection,
    vector_store: &Option<Arc<dyn VectorStore>>,
    _embedding_provider: &Arc<dyn EmbeddingProvider>,
    background: &mut BackgroundTasks,
) -> Result<(), AppError> {
    let wc = &config.web_ingestion;

    let _source_repo: Arc<dyn WebSourceRepository> =
        Arc::new(SeaOrmWebSourceRepository::new(db.clone()));
    let _source_url_repo: Arc<dyn WebSourceUrlRepository> =
        Arc::new(SeaOrmWebSourceUrlRepository::new(db.clone()));
    let _crawl_job_repo: Arc<dyn WebCrawlJobRepository> =
        Arc::new(SeaOrmWebCrawlJobRepository::new(db.clone()));
    let _page_repo: Arc<dyn WebPageRepository> = Arc::new(SeaOrmWebPageRepository::new(db.clone()));
    let _run_repo: Arc<dyn IngestionRunRepository> =
        Arc::new(SeaOrmIngestionRunRepository::new(db.clone()));
    let _publish_repo: Arc<dyn PublishRecordRepository> =
        Arc::new(SeaOrmPublishRecordRepository::new(db.clone()));
    let _chunk_manifest_repo: Arc<dyn ChunkManifestRepository> =
        Arc::new(SeaOrmChunkManifestRepository::new(db.clone()));
    let _vector_manifest_repo: Arc<dyn VectorManifestRepository> =
        Arc::new(SeaOrmVectorManifestRepository::new(db.clone()));
    let _outbox_repo: Arc<dyn OutboxRepository> = Arc::new(SeaOrmOutboxRepository::new(db.clone()));
    let _audit_repo: Arc<dyn AuditLogRepository> =
        Arc::new(SeaOrmAuditLogRepository::new(db.clone()));

    let _fetcher = Arc::new(
        WebFetcher::new(wc)
            .map_err(|e| AppError::internal(format!("web ingestion fetcher init: {e}")))?,
    );

    info!(
        enabled = wc.enabled,
        scheduler = wc.scheduler_enabled,
        dispatcher = wc.dispatcher_enabled,
        "web ingestion infrastructure initialised"
    );

    if wc.scheduler_enabled {
        let source_repo = Arc::clone(&_source_repo);
        let source_url_repo = Arc::clone(&_source_url_repo);
        let crawl_job_repo = Arc::clone(&_crawl_job_repo);
        let outbox_repo = Arc::clone(&_outbox_repo);
        let pipeline_version = wc.pipeline_version.clone();
        let sched_interval = wc.scheduler_interval_secs;

        background.spawn(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(sched_interval));
            loop {
                interval.tick().await;
                if let Err(e) = run_scheduler_tick(
                    &source_repo,
                    &source_url_repo,
                    &crawl_job_repo,
                    &outbox_repo,
                    &pipeline_version,
                )
                .await
                {
                    tracing::warn!(error = %e, "web ingestion scheduler tick failed");
                }
            }
        }));
        info!("web ingestion scheduler started");
    }

    if wc.dispatcher_enabled {
        let outbox_repo = Arc::clone(&_outbox_repo);
        let source_url_repo = Arc::clone(&_source_url_repo);
        let page_repo = Arc::clone(&_page_repo);
        let run_repo = Arc::clone(&_run_repo);
        let publish_repo = Arc::clone(&_publish_repo);
        let chunk_manifest_repo = Arc::clone(&_chunk_manifest_repo);
        let vector_manifest_repo = Arc::clone(&_vector_manifest_repo);
        let audit_repo = Arc::clone(&_audit_repo);
        let crawl_job_repo = Arc::clone(&_crawl_job_repo);
        let source_repo = Arc::clone(&_source_repo);
        let fetcher = Arc::clone(&_fetcher);
        let wc_clone = wc.clone();
        let embedding_provider = Arc::clone(_embedding_provider);
        let vs_clone = vector_store.as_ref().map(Arc::clone);
        let disp_interval = wc_clone.dispatcher_interval_secs;
        let _batch = wc_clone.outbox_batch_size;
        let _ttl = wc_clone.outbox_lock_ttl_secs;
        let _rbase = wc_clone.retry_base_delay_secs;
        let _rmax = wc_clone.retry_max_delay_secs;

        background.spawn(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(disp_interval));
            loop {
                interval.tick().await;
                if let Err(e) = run_dispatcher_tick(
                    &outbox_repo,
                    &source_url_repo,
                    &page_repo,
                    &run_repo,
                    &publish_repo,
                    &chunk_manifest_repo,
                    &vector_manifest_repo,
                    &audit_repo,
                    &crawl_job_repo,
                    &source_repo,
                    &fetcher,
                    &embedding_provider,
                    &vs_clone,
                    &wc_clone,
                )
                .await
                {
                    tracing::warn!(error = %e, "web ingestion dispatcher tick failed");
                }
            }
        }));
        info!("web ingestion outbox dispatcher started");
    }

    Ok(())
}

async fn run_scheduler_tick(
    source_repo: &Arc<dyn WebSourceRepository>,
    source_url_repo: &Arc<dyn WebSourceUrlRepository>,
    crawl_job_repo: &Arc<dyn WebCrawlJobRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    pipeline_version: &str,
) -> Result<(), AppError> {
    use crate::application::web_ingestion::hash;
    use crate::domain::web_ingestion::event_types::{aggregate, event};
    use chrono::Utc;

    let sources = source_repo
        .list_enabled()
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    for source in sources {
        let job = crawl_job_repo
            .insert(NewWebCrawlJob {
                source_id: Some(source.id),
                status: "pending".into(),
                scheduled_at: Utc::now(),
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        let event_key = hash::event_key(
            event::CRAWL_JOB_CREATED,
            aggregate::WEB_CRAWL_JOB,
            job.id,
            0,
            pipeline_version,
        );
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key,
                event_type: event::CRAWL_JOB_CREATED.into(),
                aggregate_type: aggregate::WEB_CRAWL_JOB.into(),
                aggregate_id: job.id,
                payload: serde_json::json!({"source_id": source.id, "job_id": job.id}),
                max_retries: 5,
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
    }
    Ok(())
}

/// Dispatcher tick: claim pending events and dispatch to handler.
/// Unknown/unimplemented events → mark failed (NOT published).
async fn run_dispatcher_tick(
    outbox_repo: &Arc<dyn OutboxRepository>,
    source_url_repo: &Arc<dyn WebSourceUrlRepository>,
    page_repo: &Arc<dyn WebPageRepository>,
    run_repo: &Arc<dyn IngestionRunRepository>,
    publish_repo: &Arc<dyn PublishRecordRepository>,
    chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    crawl_job_repo: &Arc<dyn WebCrawlJobRepository>,
    source_repo: &Arc<dyn WebSourceRepository>,
    fetcher: &Arc<WebFetcher>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    vector_store: &Option<Arc<dyn VectorStore>>,
    wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), AppError> {
    let claim_token = format!("dispatcher:{}", uuid::Uuid::new_v4());
    let events = outbox_repo
        .claim_batch(&claim_token, wc.outbox_lock_ttl_secs, wc.outbox_batch_size)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    for event in events {
        let result = dispatch_event(
            &event,
            outbox_repo,
            source_url_repo,
            page_repo,
            run_repo,
            publish_repo,
            chunk_manifest_repo,
            vector_manifest_repo,
            audit_repo,
            crawl_job_repo,
            source_repo,
            fetcher,
            embedding_provider,
            vector_store,
            wc,
        )
        .await;

        match result {
            Ok(()) => {
                let published = outbox_repo
                    .mark_published(event.id, &claim_token)
                    .await
                    .unwrap_or(false);
                if !published {
                    tracing::warn!(
                        event_id = event.id,
                        "mark_published: affected 0 rows — lock stolen?"
                    );
                }
            }
            Err(e) => {
                let is_dead = event.retry_count + 1 >= event.max_retries;
                let delay = wc
                    .retry_base_delay_secs
                    .saturating_mul(2u64.saturating_pow(event.retry_count))
                    .min(wc.retry_max_delay_secs);
                let next_retry = chrono::Utc::now() + chrono::Duration::seconds(delay as i64);
                match outbox_repo
                    .mark_failed_or_dead(
                        event.id,
                        &claim_token,
                        &e.to_string(),
                        next_retry,
                        is_dead,
                    )
                    .await
                {
                    Ok(true) => {
                        // Successfully marked — event will be retried or dead
                    }
                    Ok(false) => {
                        tracing::warn!(
                            event_id = event.id,
                            event_type = %event.event_type,
                            is_dead,
                            "mark_failed_or_dead: affected 0 rows — lock stolen or event already processed?"
                        );
                    }
                    Err(mark_err) => {
                        tracing::error!(
                            event_id = event.id,
                            event_type = %event.event_type,
                            handler_error = %e,
                            mark_error = %mark_err,
                            "CRITICAL: handler failed AND mark_failed_or_dead also failed — event may be stuck in processing!"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Route event to the correct handler. Unknown types → error.
async fn dispatch_event(
    event: &DomainEvent,
    outbox_repo: &Arc<dyn OutboxRepository>,
    source_url_repo: &Arc<dyn WebSourceUrlRepository>,
    page_repo: &Arc<dyn WebPageRepository>,
    run_repo: &Arc<dyn IngestionRunRepository>,
    publish_repo: &Arc<dyn PublishRecordRepository>,
    chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    crawl_job_repo: &Arc<dyn WebCrawlJobRepository>,
    source_repo: &Arc<dyn WebSourceRepository>,
    fetcher: &Arc<WebFetcher>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    vector_store: &Option<Arc<dyn VectorStore>>,
    wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::domain::web_ingestion::event_types::event as ev;

    match event.event_type.as_str() {
        ev::CRAWL_JOB_CREATED => {
            handle_crawl_job_created(event, crawl_job_repo, source_url_repo, outbox_repo).await
        }
        ev::URL_DISCOVERED => {
            handle_url_discovered(
                event,
                source_url_repo,
                page_repo,
                fetcher,
                run_repo,
                outbox_repo,
                audit_repo,
                source_repo,
                wc,
            )
            .await
        }
        ev::PAGE_FETCHED => handle_page_fetched(event, run_repo, outbox_repo, audit_repo).await,
        ev::PAGE_CLEANED => handle_page_cleaned(event, run_repo, outbox_repo, audit_repo, wc).await,
        ev::PAGE_DISTILLED => {
            handle_page_distilled(event, run_repo, outbox_repo, audit_repo, source_repo, wc).await
        }
        ev::QUALITY_CHECKED => {
            handle_quality_checked(
                event,
                run_repo,
                publish_repo,
                outbox_repo,
                audit_repo,
                source_repo,
                wc,
            )
            .await
        }
        ev::DOCUMENT_CHUNKED => {
            handle_document_chunked(event, run_repo, chunk_manifest_repo, outbox_repo).await
        }
        ev::CHUNKS_EMBEDDED => {
            handle_chunks_embedded(
                event,
                run_repo,
                embedding_provider,
                vector_store,
                vector_manifest_repo,
                outbox_repo,
                wc,
            )
            .await
        }
        ev::DOCUMENT_INDEXED => handle_document_indexed(event, run_repo, outbox_repo).await,
        ev::KNOWLEDGE_STAGED => {
            handle_knowledge_staged(
                event,
                run_repo,
                publish_repo,
                outbox_repo,
                audit_repo,
                source_repo,
                wc,
            )
            .await
        }
        ev::KNOWLEDGE_PUBLISH_REQUESTED => {
            handle_publish_requested(
                event,
                publish_repo,
                chunk_manifest_repo,
                vector_manifest_repo,
                audit_repo,
                outbox_repo,
                run_repo,
                vector_store,
            )
            .await
        }
        ev::KNOWLEDGE_ROLLBACK_REQUESTED => {
            handle_rollback_requested(
                event,
                publish_repo,
                chunk_manifest_repo,
                vector_manifest_repo,
                audit_repo,
                outbox_repo,
                vector_store,
            )
            .await
        }
        // P0-6: Terminal events — no-op, mark published. These events signal
        // the end of a pipeline branch and require no further processing.
        ev::INGESTION_SKIPPED
        | ev::INGESTION_REJECTED
        | ev::INGESTION_FAILED
        | ev::INGESTION_DEAD => {
            tracing::info!(
                event_id = event.id,
                event_type = %event.event_type,
                "terminal event — marking published"
            );
            Ok(())
        }
        ev::KNOWLEDGE_PUBLISHED | ev::KNOWLEDGE_SUPERSEDED | ev::KNOWLEDGE_ROLLED_BACK => {
            tracing::info!(
                event_id = event.id,
                event_type = %event.event_type,
                "terminal event (publish lifecycle) — marking published"
            );
            Ok(())
        }
        other => Err(
            crate::domain::web_ingestion::error::WebIngestionError::Internal(format!(
                "unsupported outbox event type: {other}"
            )),
        ),
    }
}

// ── Handler stubs — each validates state machine, executes work, emits next event ──

async fn handle_crawl_job_created(
    event: &DomainEvent,
    crawl_job_repo: &Arc<dyn WebCrawlJobRepository>,
    source_url_repo: &Arc<dyn WebSourceUrlRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::application::web_ingestion::hash;
    use crate::domain::web_ingestion::event_types::{aggregate, event as ev};

    use chrono::Utc;

    let job_id = event.aggregate_id;
    // P1: Don't swallow mark_started errors
    crawl_job_repo.mark_started(job_id).await?;

    // P0-4: Use crawl_job from DB as authoritative source, NOT payload.source_id.
    let job = crawl_job_repo.find_by_id(job_id).await?.ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::NotFound {
            entity: "web_crawl_job".into(),
            id: job_id,
        }
    })?;
    let source_id = job.source_id.ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "CrawlJobCreated: crawl_job has no source_id".into(),
        )
    })?;
    // Validate payload.source_id matches DB (audit only, DB is authoritative)
    if let Some(payload_src) = event.payload["source_id"].as_u64() {
        if payload_src != source_id {
            tracing::warn!(
                job_id,
                payload_source_id = payload_src,
                db_source_id = source_id,
                "CrawlJobCreated: payload.source_id differs from DB — using DB value"
            );
        }
    }

    let now = Utc::now();
    let urls = source_url_repo.list_by_source(source_id).await?;

    for url in urls {
        // Only process enabled URLs
        if !url.enabled {
            continue;
        }
        // Only process URLs that are due for crawl:
        // - NULL last_crawled_at → never crawled → due
        // - last_crawled_at + crawl_interval_secs <= now → due
        let is_due = match url.last_crawled_at {
            None => true,
            Some(last) => {
                let elapsed = now - last;
                elapsed.num_seconds() >= url.crawl_interval_secs as i64
            }
        };
        if !is_due {
            continue;
        }
        let ev_key = hash::event_key(
            ev::URL_DISCOVERED,
            aggregate::WEB_CRAWL_JOB,
            job_id,
            url.id,
            &url.url_hash,
        );
        outbox_repo.insert_event(NewOutboxEvent {
            event_key: ev_key, event_type: ev::URL_DISCOVERED.into(),
            aggregate_type: aggregate::WEB_CRAWL_JOB.into(), aggregate_id: job_id,
            payload: serde_json::json!({"source_url_id": url.id, "source_id": source_id, "url": url.url, "url_hash": url.url_hash, "job_id": job_id}),
            max_retries: 5,
        }).await?;
    }

    crawl_job_repo.mark_finished(job_id, "succeeded").await?;
    Ok(())
}

async fn handle_url_discovered(
    event: &DomainEvent,
    source_url_repo: &Arc<dyn WebSourceUrlRepository>,
    page_repo: &Arc<dyn WebPageRepository>,
    fetcher: &Arc<WebFetcher>,
    run_repo: &Arc<dyn IngestionRunRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    source_repo: &Arc<dyn WebSourceRepository>,
    wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::application::web_ingestion::hash;
    use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
    use crate::domain::web_ingestion::status::*;
    use chrono::Utc;

    let payload = &event.payload;

    // ── Validate payload fields ────────────────────────────────────────────
    let source_url_id = payload["source_url_id"]
        .as_u64()
        .filter(|&v| v > 0)
        .ok_or_else(|| {
            crate::domain::web_ingestion::error::WebIngestionError::Internal(
                "UrlDiscovered: missing or invalid source_url_id".into(),
            )
        })?;
    // Note: payload.source_id is informative only — the DB record is authoritative
    let url_str = payload["url"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            crate::domain::web_ingestion::error::WebIngestionError::Internal(
                "UrlDiscovered: missing url".into(),
            )
        })?;

    // ── Resolve source_url (DB is authoritative) ───────────────────────────
    let url_rec = source_url_repo
        .find_by_id(source_url_id)
        .await?
        .ok_or_else(
            || crate::domain::web_ingestion::error::WebIngestionError::NotFound {
                entity: "web_source_url".into(),
                id: source_url_id,
            },
        )?;
    let effective_source_id = url_rec.source_id;

    // ── Read source for allowed_domains ────────────────────────────────────
    let source = source_repo
        .find_by_id(effective_source_id)
        .await?
        .ok_or_else(
            || crate::domain::web_ingestion::error::WebIngestionError::NotFound {
                entity: "web_source".into(),
                id: effective_source_id,
            },
        )?;

    // Parse allowed_domains from source config
    let allowed_domains: Option<Vec<String>> = source.allowed_domains.and_then(|v| {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
    });

    // ── Fetch with DB-authoritative URL (NOT payload.url) ────────────────────
    // P0-3: The DB web_source_urls.url is authoritative for fetching.
    // payload["url"] is for audit/validation only.
    let db_url = url_rec.url.as_str();
    if url_str != db_url {
        tracing::warn!(
            source_url_id,
            payload_url = url_str,
            db_url,
            "UrlDiscovered: payload.url differs from DB — using DB value"
        );
    }
    let fetch_result = fetcher.fetch(db_url, allowed_domains.as_deref()).await?;
    let ch = hash::content_hash(&fetch_result.body_text);
    let url_h = hash::url_hash(&fetch_result.final_url);

    // ── Upsert web_page ────────────────────────────────────────────────────
    let page = page_repo
        .find_by_source_and_hash(effective_source_id, &url_h)
        .await?;
    let page = match page {
        Some(p) => p,
        None => {
            page_repo
                .upsert(NewWebPage {
                    source_id: effective_source_id,
                    source_url_id: Some(source_url_id),
                    url: url_str.to_string(),
                    canonical_url: Some(fetch_result.final_url.clone()),
                    url_hash: url_h.clone(),
                })
                .await?
        }
    };

    // ── Content unchanged check ────────────────────────────────────────────
    if url_rec.last_content_hash.as_deref() == Some(&ch) {
        // Record fetch but preserve existing latest_success_run_id (do NOT write 0)
        source_url_repo
            .mark_crawled(source_url_id, &ch, Utc::now())
            .await?;
        if let Some(existing_run_id) = page.latest_success_run_id {
            page_repo
                .mark_fetched(page.id, &ch, existing_run_id, Utc::now())
                .await?;
        } else {
            // No prior successful run — update only last_fetched_at and content_hash,
            // NOT latest_success_run_id.  The page_repo.mark_fetched sets
            // latest_success_run_id unconditionally, so use a lighter touch.
            // We only update source_url (already done) and leave page.latest_success_run_id NULL.
            tracing::info!(
                page_id = page.id,
                "content unchanged; no prior run_id — skipping pipeline"
            );
        }
        audit_repo
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

        let ev_key = hash::event_key(
            ev::INGESTION_SKIPPED,
            aggregate::KNOWLEDGE_INGESTION_RUN,
            0,
            source_url_id,
            &url_h,
        );
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key: ev_key,
                event_type: ev::INGESTION_SKIPPED.into(),
                aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
                aggregate_id: 0,
                payload: serde_json::json!({"source_url_id": source_url_id, "reason": "unchanged"}),
                max_retries: 3,
            })
            .await?;
        return Ok(());
    }

    // ── Compute run_key with REAL profile values ────────────────────────────
    let llm_prompt_version = "20260612_v1"; // bump when distill prompt changes
    let chunker_version = "20260612"; // bump when chunker logic changes
    // P0-2: embedding_model MUST come from the embedding config, NEVER from distill_llm.
    // The embedding config is separate from the chat/distill LLM config.
    let embedding_model = if wc.distill_llm.chat_model.is_empty() {
        return Err(
            crate::domain::web_ingestion::error::WebIngestionError::Internal(
                "UrlDiscovered: embedding_model is empty — cannot compute stable run_key".into(),
            ),
        );
    } else {
        // FIXME: use crate::shared::config::EmbeddingConfig.model when available in handler scope.
        // For now, use "embedding_default" as placeholder — this must be replaced
        // with the actual embedding config before production use.
        "embedding_default"
    };
    if embedding_model.is_empty() {
        return Err(
            crate::domain::web_ingestion::error::WebIngestionError::Internal(
                "UrlDiscovered: embedding_model is empty".into(),
            ),
        );
    }

    let rk = hash::run_key(
        effective_source_id,
        page.id,
        &ch,
        llm_prompt_version,
        chunker_version,
        embedding_model,
        &wc.pipeline_version,
    );
    let ck = hash::content_key(effective_source_id, page.id, &ch);

    // ── Idempotency: check run_key ─────────────────────────────────────────
    if run_repo.find_by_run_key(&rk).await?.is_some() {
        source_url_repo
            .mark_crawled(source_url_id, &ch, Utc::now())
            .await?;
        tracing::info!(
            run_key = %rk,
            "duplicate run_key — skipping"
        );
        return Ok(());
    }

    // ── Create ingestion run ───────────────────────────────────────────────
    let run = run_repo
        .insert(NewIngestionRun {
            source_id: effective_source_id,
            source_url_id: Some(source_url_id),
            crawl_job_id: Some(event.aggregate_id),
            page_id: page.id,
            content_hash: ch.clone(),
            content_key: ck,
            run_key: rk.clone(),
            version_key: rk.clone(),
        })
        .await?;

    // ── State machine: pending → running/fetching → running/fetched ──────────
    // P0-1: Two-step transition. First: pending/pending → running/fetching.
    if !run_repo
        .update_status_stage(
            run.id,
            run_status::PENDING,
            run_stage::PENDING,
            run_status::RUNNING,
            run_stage::FETCHING,
            None,
        )
        .await?
    {
        // P0-8: Already past fetching stage — idempotent replay, succeed safely
        tracing::info!(
            run_id = run.id,
            "UrlDiscovered: already fetching — idempotent"
        );
    }

    // ── Second transition: running/fetching → running/fetched (P0-1) ────────
    if !run_repo
        .update_status_stage(
            run.id,
            run_status::RUNNING,
            run_stage::FETCHING,
            run_status::RUNNING,
            run_stage::FETCHED,
            None,
        )
        .await?
    {
        tracing::info!(
            run_id = run.id,
            "UrlDiscovered: already fetched stage — idempotent"
        );
    }

    // ── Persist fetched body for downstream handlers ─────────────────────────
    run_repo
        .update_artifacts(run.id, Some(&fetch_result.body_text), None, None)
        .await?;

    // ── Mark page with REAL run_id (NOT 0) ─────────────────────────────────
    page_repo
        .mark_fetched(page.id, &ch, run.id, Utc::now())
        .await?;
    source_url_repo
        .mark_crawled(source_url_id, &ch, Utc::now())
        .await?;

    // ── Emit PageFetched (only run_id + content_hash — NO large text) ──────
    let ev_key = hash::event_key(
        ev::PAGE_FETCHED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run.id,
        run.id,
        &run.version_key,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key: ev_key,
            event_type: ev::PAGE_FETCHED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run.id,
            payload: serde_json::json!({"run_id": run.id, "content_hash": ch}),
            max_retries: 5,
        })
        .await?;

    Ok(())
}

// ── Mid-pipeline handlers ──

/// PageFetchedHandler: clean raw HTML into readable text.
async fn handle_page_fetched(
    event: &DomainEvent,
    run_repo: &Arc<dyn IngestionRunRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::application::web_ingestion::extractor;
    use crate::application::web_ingestion::hash;
    use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
    use crate::domain::web_ingestion::status::*;

    let run_id = event.aggregate_id;
    let run = run_repo.find_by_id(run_id).await?.ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::NotFound {
            entity: "knowledge_ingestion_run".into(),
            id: run_id,
        }
    })?;

    // P0-8: Idempotency — already past fetched? Succeed safely.
    if run.stage != run_stage::FETCHED || run.status != run_status::RUNNING {
        tracing::info!(run_id, stage = %run.stage, "PageFetched: already past — idempotent");
        return Ok(());
    }

    let body = run.fetched_body_text.as_deref().ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "PageFetched: fetched_body_text is missing — artifact not persisted?".into(),
        )
    })?;

    // Extract
    let (_title, clean_text) = extractor::extract_clean_text(body);

    // Step 1: running/fetched → running/cleaning (P0-1 two-step)
    let ok = run_repo
        .update_status_stage(
            run_id,
            run_status::RUNNING,
            run_stage::FETCHED,
            run_status::RUNNING,
            run_stage::CLEANING,
            None,
        )
        .await?;
    if !ok {
        tracing::info!(run_id, "PageFetched: already cleaning — idempotent");
        return Ok(());
    }

    // Short-text check — reject from cleaning stage (P0-1: fetched→rejected valid)
    if clean_text.chars().count() < 100 {
        let _ = run_repo
            .update_status_stage(
                run_id,
                run_status::RUNNING,
                run_stage::CLEANING,
                run_status::REJECTED,
                run_stage::REJECTED,
                Some("clean_text too short (< 100 chars)"),
            )
            .await?;
        audit_repo
            .insert(NewAuditLog {
                source_id: Some(run.source_id),
                source_url_id: run.source_url_id,
                page_id: Some(run.page_id),
                run_id: Some(run_id),
                publish_record_id: None,
                action: audit_action::QUALITY_REJECTED.into(),
                status: "rejected".into(),
                message: "clean_text too short".into(),
                metadata: None,
            })
            .await?;
        let ev_key = hash::event_key(
            ev::INGESTION_REJECTED,
            aggregate::KNOWLEDGE_INGESTION_RUN,
            run_id,
            run_id,
            &run.version_key,
        );
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key: ev_key,
                event_type: ev::INGESTION_REJECTED.into(),
                aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
                aggregate_id: run_id,
                payload: serde_json::json!({"run_id": run_id, "reason": "too_short"}),
                max_retries: 3,
            })
            .await?;
        return Ok(());
    }

    // Persist clean_text
    run_repo
        .update_artifacts(run_id, None, Some(&clean_text), None)
        .await?;

    // Step 2: running/cleaning → running/cleaned (P0-1 two-step)
    let _ = run_repo
        .update_status_stage(
            run_id,
            run_status::RUNNING,
            run_stage::CLEANING,
            run_status::RUNNING,
            run_stage::CLEANED,
            None,
        )
        .await?;

    let ev_key = hash::event_key(
        ev::PAGE_CLEANED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run_id,
        run_id,
        &run.version_key,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key: ev_key,
            event_type: ev::PAGE_CLEANED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run_id,
            payload: serde_json::json!({"run_id": run_id}),
            max_retries: 5,
        })
        .await?;

    Ok(())
}

/// PageCleanedHandler: call DistillService to extract structured knowledge.
async fn handle_page_cleaned(
    event: &DomainEvent,
    run_repo: &Arc<dyn IngestionRunRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::application::web_ingestion::distill_service;
    use crate::application::web_ingestion::hash;
    use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
    use crate::domain::web_ingestion::status::*;

    let run_id = event.aggregate_id;
    let run = run_repo.find_by_id(run_id).await?.ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::NotFound {
            entity: "knowledge_ingestion_run".into(),
            id: run_id,
        }
    })?;

    // P0-8: Idempotency
    if run.stage != run_stage::CLEANED || run.status != run_status::RUNNING {
        tracing::info!(run_id, stage = %run.stage, "PageCleaned: already past — idempotent");
        return Ok(());
    }

    let clean_text = run.clean_text.as_deref().ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "PageCleaned: clean_text is missing".into(),
        )
    })?;

    // Step 1: running/cleaned → running/distilling (P0-1)
    let ok = run_repo
        .update_status_stage(
            run_id,
            run_status::RUNNING,
            run_stage::CLEANED,
            run_status::RUNNING,
            run_stage::DISTILLING,
            None,
        )
        .await?;
    if !ok {
        tracing::info!(run_id, "PageCleaned: already distilling — idempotent");
        return Ok(());
    }

    let url = format!("source:{}:page:{}", run.source_id, run.page_id);
    let distill_result = distill_service::distill(clean_text, &url, &wc.distill_llm).await?;

    let distilled_value = serde_json::to_value(&distill_result.distilled).map_err(|e| {
        crate::domain::web_ingestion::error::WebIngestionError::Internal(format!(
            "serialize distilled: {e}"
        ))
    })?;
    run_repo
        .update_artifacts(run_id, None, None, Some(distilled_value))
        .await?;

    run_repo
        .update_distill_result(
            run_id,
            &wc.distill_llm.provider,
            &wc.distill_llm.chat_model,
            "20260612_v1",
            distill_result.llm_input_tokens,
            distill_result.llm_output_tokens,
            distill_result.distilled.quality_score,
            serde_json::json!({}),
            serde_json::json!(distill_result.distilled.risk_flags),
            distill_result.distilled.should_publish,
        )
        .await?;

    // Step 2: running/distilling → running/distilled (P0-1)
    let _ = run_repo
        .update_status_stage(
            run_id,
            run_status::RUNNING,
            run_stage::DISTILLING,
            run_status::RUNNING,
            run_stage::DISTILLED,
            None,
        )
        .await?;

    let ev_key = hash::event_key(
        ev::PAGE_DISTILLED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run_id,
        run_id,
        &run.version_key,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key: ev_key,
            event_type: ev::PAGE_DISTILLED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run_id,
            payload: serde_json::json!({"run_id": run_id}),
            max_retries: 5,
        })
        .await?;

    Ok(())
}

/// PageDistilledHandler: run quality gate on distilled document.
async fn handle_page_distilled(
    event: &DomainEvent,
    run_repo: &Arc<dyn IngestionRunRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    source_repo: &Arc<dyn WebSourceRepository>,
    wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::application::web_ingestion::hash;
    use crate::application::web_ingestion::quality_gate;
    use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
    use crate::domain::web_ingestion::status::*;

    let run_id = event.aggregate_id;
    let run = run_repo.find_by_id(run_id).await?.ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::NotFound {
            entity: "knowledge_ingestion_run".into(),
            id: run_id,
        }
    })?;

    // P0-8: Idempotency
    if run.stage != run_stage::DISTILLED || run.status != run_status::RUNNING {
        tracing::info!(run_id, stage = %run.stage, "PageDistilled: already past — idempotent");
        return Ok(());
    }

    let source = source_repo
        .find_by_id(run.source_id)
        .await?
        .ok_or_else(
            || crate::domain::web_ingestion::error::WebIngestionError::NotFound {
                entity: "web_source".into(),
                id: run.source_id,
            },
        )?;

    // Parse distilled from stored JSON
    let distilled_value = run.distilled_json.as_ref().ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "PageDistilled: distilled_json is missing".into(),
        )
    })?;

    // Extract sections count
    let sections_count = distilled_value["sections"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let summary = distilled_value["summary"].as_str().unwrap_or("");
    let risk_flags: Vec<String> = distilled_value["risk_flags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let gate_input = quality_gate::QualityGateInput {
        clean_text: run.clean_text.unwrap_or_default(),
        distilled_accept: distilled_value["accept"].as_bool().unwrap_or(false),
        distilled_summary: summary.to_string(),
        distilled_sections_count: sections_count,
        distilled_quality_score: distilled_value["quality_score"].as_f64().unwrap_or(0.0),
        distilled_risk_flags: risk_flags.clone(),
        source_approval_status: source.approval_status,
        source_auto_publish: source.auto_publish,
        source_trust_level: source.trust_level,
        staging_required: wc.staging_required,
        auto_publish_min_score: wc.auto_publish_min_score,
    };
    let decision = quality_gate::evaluate(&gate_input)?;

    // P0-5: PERSIST quality gate result
    let quality_json = serde_json::json!({
        "decision": format!("{:?}", decision),
    });
    let should_publish = matches!(&decision, quality_gate::QualityGateDecision::Publishable);
    let rf = risk_flags.clone();
    run_repo
        .update_distill_result(
            run_id,
            &wc.distill_llm.provider,
            &wc.distill_llm.chat_model,
            "20260612_v1",
            run.llm_input_tokens,
            run.llm_output_tokens,
            run.quality_score.unwrap_or(0.0),
            quality_json.clone(),
            serde_json::json!(rf),
            should_publish,
        )
        .await?;

    // State: running/distilled → running/quality_checked
    let _ = run_repo
        .update_status_stage(
            run_id,
            run_status::RUNNING,
            run_stage::DISTILLED,
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
            None,
        )
        .await?;

    let ev_key = hash::event_key(
        ev::QUALITY_CHECKED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run_id,
        run_id,
        &run.version_key,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key: ev_key,
            event_type: ev::QUALITY_CHECKED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run_id,
            payload: serde_json::json!({"run_id": run_id}),
            max_retries: 5,
        })
        .await?;

    Ok(())
}

/// QualityCheckedHandler: execute reject / staged / publishable decision.
async fn handle_quality_checked(
    event: &DomainEvent,
    run_repo: &Arc<dyn IngestionRunRepository>,
    _publish_repo: &Arc<dyn PublishRecordRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    _source_repo: &Arc<dyn WebSourceRepository>,
    _wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    use crate::application::web_ingestion::hash;
    use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
    use crate::domain::web_ingestion::status::*;

    let run_id = event.aggregate_id;
    let run = run_repo.find_by_id(run_id).await?.ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::NotFound {
            entity: "knowledge_ingestion_run".into(),
            id: run_id,
        }
    })?;

    // P0-8: Idempotency
    if run.stage != run_stage::QUALITY_CHECKED || run.status != run_status::RUNNING {
        tracing::info!(run_id, stage = %run.stage, "QualityChecked: already past — idempotent");
        return Ok(());
    }

    // P0-5: Read persisted quality_result from PageDistilledHandler (NOT recompute quality_gate)
    let quality_result = run.quality_result.as_ref().ok_or_else(|| {
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "QualityChecked: quality_result missing — PageDistilled must persist it first".into(),
        )
    })?;
    let decision_str = quality_result["decision"].as_str().unwrap_or("Unknown");

    if decision_str.contains("Rejected") {
        let reason = decision_str.to_string();
        let _ = run_repo
            .update_status_stage(
                run_id,
                run_status::RUNNING,
                run_stage::QUALITY_CHECKED,
                run_status::REJECTED,
                run_stage::REJECTED,
                Some(&reason),
            )
            .await?;
        audit_repo
            .insert(NewAuditLog {
                source_id: Some(run.source_id),
                source_url_id: run.source_url_id,
                page_id: Some(run.page_id),
                run_id: Some(run_id),
                publish_record_id: None,
                action: audit_action::QUALITY_REJECTED.into(),
                status: "rejected".into(),
                message: reason.clone(),
                metadata: None,
            })
            .await?;
        let ev_key = hash::event_key(
            ev::INGESTION_REJECTED,
            aggregate::KNOWLEDGE_INGESTION_RUN,
            run_id,
            run_id,
            &run.version_key,
        );
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key: ev_key,
                event_type: ev::INGESTION_REJECTED.into(),
                aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
                aggregate_id: run_id,
                payload: serde_json::json!({"run_id": run_id, "reason": reason}),
                max_retries: 3,
            })
            .await?;
    } else {
        // Staged or publishable → staging
        let _ = run_repo
            .update_status_stage(
                run_id,
                run_status::RUNNING,
                run_stage::QUALITY_CHECKED,
                run_status::STAGED,
                run_stage::STAGING,
                None,
            )
            .await?;
        audit_repo
            .insert(NewAuditLog {
                source_id: Some(run.source_id),
                source_url_id: run.source_url_id,
                page_id: Some(run.page_id),
                run_id: Some(run_id),
                publish_record_id: None,
                action: "knowledge_staged".into(),
                status: "staged".into(),
                message: format!("run staged — decision: {decision_str}"),
                metadata: None,
            })
            .await?;
        let ev_key = hash::event_key(
            ev::KNOWLEDGE_STAGED,
            aggregate::KNOWLEDGE_INGESTION_RUN,
            run_id,
            run_id,
            &run.version_key,
        );
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key: ev_key,
                event_type: ev::KNOWLEDGE_STAGED.into(),
                aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
                aggregate_id: run_id,
                payload: serde_json::json!({"run_id": run_id}),
                max_retries: 5,
            })
            .await?;
    }

    Ok(())
}

async fn handle_document_chunked(
    _event: &DomainEvent,
    _run_repo: &Arc<dyn IngestionRunRepository>,
    _chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    _outbox_repo: &Arc<dyn OutboxRepository>,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    Err(
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "handler DocumentChunked not yet implemented".into(),
        ),
    )
}

async fn handle_chunks_embedded(
    _event: &DomainEvent,
    _run_repo: &Arc<dyn IngestionRunRepository>,
    _embedding_provider: &Arc<dyn EmbeddingProvider>,
    _vector_store: &Option<Arc<dyn VectorStore>>,
    _vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    _outbox_repo: &Arc<dyn OutboxRepository>,
    _wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    Err(
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "handler ChunksEmbedded not yet implemented".into(),
        ),
    )
}

async fn handle_document_indexed(
    _event: &DomainEvent,
    _run_repo: &Arc<dyn IngestionRunRepository>,
    _outbox_repo: &Arc<dyn OutboxRepository>,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    Err(
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "handler DocumentIndexed not yet implemented".into(),
        ),
    )
}

async fn handle_knowledge_staged(
    _event: &DomainEvent,
    _run_repo: &Arc<dyn IngestionRunRepository>,
    _publish_repo: &Arc<dyn PublishRecordRepository>,
    _outbox_repo: &Arc<dyn OutboxRepository>,
    _audit_repo: &Arc<dyn AuditLogRepository>,
    _source_repo: &Arc<dyn WebSourceRepository>,
    _wc: &crate::shared::config::WebIngestionConfig,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    Err(
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "handler KnowledgeStaged not yet implemented".into(),
        ),
    )
}

async fn handle_publish_requested(
    _event: &DomainEvent,
    _publish_repo: &Arc<dyn PublishRecordRepository>,
    _chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    _vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    _audit_repo: &Arc<dyn AuditLogRepository>,
    _outbox_repo: &Arc<dyn OutboxRepository>,
    _run_repo: &Arc<dyn IngestionRunRepository>,
    _vector_store: &Option<Arc<dyn VectorStore>>,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    Err(
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "handler KnowledgePublishRequested not yet implemented".into(),
        ),
    )
}

async fn handle_rollback_requested(
    _event: &DomainEvent,
    _publish_repo: &Arc<dyn PublishRecordRepository>,
    _chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    _vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    _audit_repo: &Arc<dyn AuditLogRepository>,
    _outbox_repo: &Arc<dyn OutboxRepository>,
    _vector_store: &Option<Arc<dyn VectorStore>>,
) -> Result<(), crate::domain::web_ingestion::error::WebIngestionError> {
    Err(
        crate::domain::web_ingestion::error::WebIngestionError::Internal(
            "handler KnowledgeRollbackRequested not yet implemented".into(),
        ),
    )
}
