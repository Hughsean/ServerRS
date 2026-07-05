use std::collections::BTreeSet;
use std::error::Error;
use std::sync::Arc;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, Value};
use server_rs::app::web_ingestion::services::qdrant_activation_service;
use server_rs::bootstrap::infra::InfraContext;
use server_rs::bootstrap::repos::build_repos;
use server_rs::bootstrap::vector::VectorContext;
use server_rs::domain::rag::RAGRepoT;
use server_rs::domain::web_ingestion::repo::{
    IngestionRunRepoT, PublishRecordRepoT, VectorManifestRepoT,
};
use server_rs::infra::web_ingestion::repo::{
    IngestionRunRepo, PublishRecordRepo, VectorManifestRepo,
};
use server_rs::shared::config::AppConfig;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Clone)]
struct Args {
    dry_run: bool,
    include_inactive: bool,
    limit: Option<u64>,
    ids: Option<Vec<u64>>,
}

#[tokio::main]
async fn main() -> Result<(), DynError> {
    dotenvy::dotenv().ok();

    let config = AppConfig::load();
    init_tracing(&config.logging.level);

    let args = Args::parse()?;
    info!(?args, "repair_qdrant_activation started");

    if !config.qdrant.enabled {
        return Err("qdrant.enabled is false".into());
    }

    let infra = InfraContext::new(&config).await?;

    let repos = build_repos(
        &infra.db,
        &config.qdrant.memory_collection,
        &config.qdrant.summary_collection,
    );

    let vector = VectorContext::new(&config, &infra, &repos).await?;

    if vector.vector_store.is_none() {
        return Err("vector_store is none; check qdrant.enabled and qdrant feature".into());
    }

    let vector_manifest_repo: Arc<dyn VectorManifestRepoT> =
        Arc::new(VectorManifestRepo::new(infra.db.clone()));
    let publish_record_repo: Arc<dyn PublishRecordRepoT> =
        Arc::new(PublishRecordRepo::new(infra.db.clone()));
    let run_repo: Arc<dyn IngestionRunRepoT> = Arc::new(IngestionRunRepo::new(infra.db.clone()));

    let active_ids = load_active_ids(&infra.db, &args).await?;

    let inactive_ids = if args.include_inactive {
        load_inactive_ids(&infra.db, &args).await?
    } else {
        Vec::new()
    };

    println!("active=true records: {}", active_ids.len());
    println!("active=false records: {}", inactive_ids.len());

    if args.dry_run {
        println!("dry run only; qdrant is not modified");
        print_preview("active=true", &active_ids);
        print_preview("active=false", &inactive_ids);
        return Ok(());
    }

    let dimension = config.embedding.dimension;

    let mut active_ok = 0usize;
    let mut active_failed = 0usize;

    for publish_record_id in active_ids {
        match sync_record(
            &vector,
            &vector_manifest_repo,
            &publish_record_repo,
            &run_repo,
            &repos.rag_repo,
            publish_record_id,
            dimension,
        )
        .await
        {
            Ok(()) => {
                active_ok += 1;
                info!(publish_record_id, active = true, "qdrant sync ok");
            }
            Err(err) => {
                active_failed += 1;
                error!(
                    publish_record_id,
                    active = true,
                    error = %err,
                    "qdrant sync failed"
                );
            }
        }
    }

    let mut inactive_ok = 0usize;
    let mut inactive_failed = 0usize;

    for publish_record_id in inactive_ids {
        match sync_record(
            &vector,
            &vector_manifest_repo,
            &publish_record_repo,
            &run_repo,
            &repos.rag_repo,
            publish_record_id,
            dimension,
        )
        .await
        {
            Ok(()) => {
                inactive_ok += 1;
                info!(publish_record_id, active = false, "qdrant sync ok");
            }
            Err(err) => {
                inactive_failed += 1;
                error!(
                    publish_record_id,
                    active = false,
                    error = %err,
                    "qdrant sync failed"
                );
            }
        }
    }

    println!("done");
    println!("active=true ok: {}", active_ok);
    println!("active=true failed: {}", active_failed);
    println!("active=false ok: {}", inactive_ok);
    println!("active=false failed: {}", inactive_failed);

    if active_failed > 0 || inactive_failed > 0 {
        return Err("some qdrant sync jobs failed; check logs".into());
    }

    Ok(())
}

async fn sync_record(
    vector: &VectorContext,
    vector_manifest_repo: &Arc<dyn VectorManifestRepoT>,
    publish_record_repo: &Arc<dyn PublishRecordRepoT>,
    run_repo: &Arc<dyn IngestionRunRepoT>,
    rag_repo: &Arc<dyn RAGRepoT>,
    publish_record_id: u64,
    dimension: usize,
) -> Result<(), DynError> {
    qdrant_activation_service::sync_active(
        &vector.vector_store,
        vector_manifest_repo,
        publish_record_repo,
        run_repo,
        rag_repo,
        publish_record_id,
        dimension,
    )
    .await?;

    Ok(())
}

async fn load_active_ids(db: &DatabaseConnection, args: &Args) -> Result<Vec<u64>, DynError> {
    let mut sql = String::from(
        "
        SELECT DISTINCT kpr.id AS publish_record_id
        FROM knowledge_publish_records kpr
        JOIN knowledge_vector_manifests kvm
          ON kvm.publish_record_id = kpr.id
        WHERE kpr.publish_status = 'published'
          AND kpr.active = 1
          AND kvm.active = 1
        ",
    );

    let mut values = Vec::new();

    append_ids_filter(&mut sql, &mut values, args.ids.as_deref());
    append_order_limit(&mut sql, &mut values, args.limit);

    load_ids(db, sql, values).await
}

async fn load_inactive_ids(db: &DatabaseConnection, args: &Args) -> Result<Vec<u64>, DynError> {
    let mut sql = String::from(
        "
        SELECT DISTINCT kpr.id AS publish_record_id
        FROM knowledge_publish_records kpr
        JOIN knowledge_vector_manifests kvm
          ON kvm.publish_record_id = kpr.id
        WHERE NOT (
          kpr.publish_status = 'published'
          AND kpr.active = 1
        )
        ",
    );

    let mut values = Vec::new();

    append_ids_filter(&mut sql, &mut values, args.ids.as_deref());
    append_order_limit(&mut sql, &mut values, args.limit);

    load_ids(db, sql, values).await
}

async fn load_ids(
    db: &DatabaseConnection,
    sql: String,
    values: Vec<Value>,
) -> Result<Vec<u64>, DynError> {
    let stmt = Statement::from_sql_and_values(DatabaseBackend::MySql, sql, values);

    let rows = db.query_all_raw(stmt).await?;

    let mut ids = BTreeSet::new();

    for row in rows {
        let id: u64 = row.try_get("", "publish_record_id")?;
        ids.insert(id);
    }

    Ok(ids.into_iter().collect())
}

fn append_ids_filter(sql: &mut String, values: &mut Vec<Value>, ids: Option<&[u64]>) {
    let Some(ids) = ids else {
        return;
    };

    if ids.is_empty() {
        return;
    }

    sql.push_str(" AND kpr.id IN (");

    for index in 0..ids.len() {
        if index > 0 {
            sql.push_str(", ");
        }

        sql.push('?');
        values.push(Value::BigUnsigned(Some(ids[index])));
    }

    sql.push(')');
}

fn append_order_limit(sql: &mut String, values: &mut Vec<Value>, limit: Option<u64>) {
    sql.push_str(" ORDER BY kpr.id ASC");

    if let Some(limit) = limit {
        sql.push_str(" LIMIT ?");
        values.push(Value::BigUnsigned(Some(limit)));
    }
}

fn print_preview(label: &str, ids: &[u64]) {
    println!("{} count={}", label, ids.len());

    let preview = ids
        .iter()
        .take(50)
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    if !preview.is_empty() {
        println!("{} first_50={}", label, preview);
    }
}

impl Args {
    fn parse() -> Result<Self, DynError> {
        let mut dry_run = false;
        let mut include_inactive = false;
        let mut limit = None;
        let mut ids = None;

        let mut iter = std::env::args().skip(1);

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--dry-run" => {
                    dry_run = true;
                }
                "--include-inactive" => {
                    include_inactive = true;
                }
                "--limit" => {
                    let raw = iter.next().ok_or("--limit requires a number")?;
                    let parsed = raw
                        .parse::<u64>()
                        .map_err(|_| "--limit must be a u64 number")?;
                    limit = Some(parsed);
                }
                "--ids" => {
                    let raw = iter.next().ok_or("--ids requires comma separated ids")?;
                    ids = Some(parse_ids(&raw)?);
                }
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    return Err(format!("unknown argument: {}", other).into());
                }
            }
        }

        Ok(Self {
            dry_run,
            include_inactive,
            limit,
            ids,
        })
    }
}

fn parse_ids(raw: &str) -> Result<Vec<u64>, DynError> {
    let mut ids = Vec::new();

    for item in raw.split(',') {
        let item = item.trim();

        if item.is_empty() {
            continue;
        }

        let id = item
            .parse::<u64>()
            .map_err(|_| format!("invalid publish_record_id: {}", item))?;

        ids.push(id);
    }

    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return Err("ids list is empty".into());
    }

    Ok(ids)
}

fn print_help() {
    println!("repair_qdrant_activation");
    println!();
    println!("Usage:");
    println!("  cargo run --bin repair_qdrant_activation --release -- [options]");
    println!();
    println!("Options:");
    println!("  --dry-run");
    println!("      Print records only. Do not write qdrant.");
    println!();
    println!("  --ids 3848,5345,5346");
    println!("      Only sync selected publish_record_id values.");
    println!();
    println!("  --limit 100");
    println!("      Limit selected records.");
    println!();
    println!("  --include-inactive");
    println!("      Also sync non active-published records to active=false.");
    println!();
    println!("Examples:");
    println!("  cargo run --bin repair_qdrant_activation --release -- --dry-run");
    println!("  cargo run --bin repair_qdrant_activation --release -- --ids 3848,5345");
    println!("  cargo run --bin repair_qdrant_activation --release -- --include-inactive");
}

fn init_tracing(configured_level: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(configured_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_target(true)
        .with_line_number(true)
        .compact();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ids_trims_sorts_and_deduplicates_values() {
        let ids = parse_ids("5346, 3848,5346").expect("ids should parse");

        assert_eq!(ids, vec![3848, 5346]);
    }

    #[test]
    fn parse_ids_rejects_empty_lists() {
        let err = parse_ids(" ,, ").expect_err("empty ids should be rejected");

        assert_eq!(err.to_string(), "ids list is empty");
    }

    #[test]
    fn parse_ids_reports_invalid_item() {
        let err = parse_ids("1,abc").expect_err("invalid ids should be rejected");

        assert_eq!(err.to_string(), "invalid publish_record_id: abc");
    }

    #[test]
    fn id_filter_and_limit_use_parameter_placeholders() {
        let mut sql = String::from("SELECT kpr.id FROM knowledge_publish_records kpr WHERE 1 = 1");
        let mut values = Vec::new();

        append_ids_filter(&mut sql, &mut values, Some(&[5, 3]));
        append_order_limit(&mut sql, &mut values, Some(10));

        assert!(sql.contains(" AND kpr.id IN (?, ?)"));
        assert!(sql.ends_with(" ORDER BY kpr.id ASC LIMIT ?"));
        assert_eq!(values.len(), 3);
    }
}
