# Hybrid Persistence Runbook

## Ownership

- SeaORM entities in `src/infra/repo/entities` are the Rust-facing logical contract.
- `migration/` owns MySQL physical schema history.
- ETL owns selective legacy data movement from `digital_companion` to `server_rs`.
- Qdrant and vector projection state are rebuilt derived artifacts.

## Daily Schema Workflow

1. Edit the SeaORM entity when the Rust-facing logical contract changes.
2. Add or update a `sea-orm-migration` migration for the MySQL physical schema change.
3. Run `cargo check`.
4. Run `cargo check --manifest-path migration/Cargo.toml`.
5. Apply migrations to a disposable or local target database with `DATABASE_URL` pointing at `server_rs`.
6. Do not run schema sync against `digital_companion`, shared dev, staging, or production.

## Local Migration Commands

PowerShell:

```powershell
$env:DATABASE_URL = "mysql://user:password@127.0.0.1:3306/server_rs"
cargo run --manifest-path migration/Cargo.toml -- status
cargo run --manifest-path migration/Cargo.toml -- up
```

## Legacy Database Rule

`digital_companion` is read-only during this transition. Use dbx connection `DQL` only for inspection and later ETL source reads.

## Schema Audit

Run the queries in `docs/schema/mysql-schema-audit-queries.sql` against dbx connection `DQL`. Store the results in `docs/schema/2026-07-05-digital-companion-schema-audit.md`.

The audit report must classify each table into one of these domains:

- AI Core
- Vector Projection
- QQ Bot
- Fresh Context
- Non-AI Content
- Migration/Internal

## Cutover Boundary

Do not update `.env` or `config.toml` to `server_rs` until the baseline migration, selective ETL, ID mapping validation, and vector rebuild validation have passed.
