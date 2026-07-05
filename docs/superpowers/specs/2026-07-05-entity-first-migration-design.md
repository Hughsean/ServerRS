# Entity-First SeaORM Migration Design

## Context

ServerRS currently treats `database/sql/*.sql` as the practical schema source, and `src/infra/repo/entities` is regenerated from the database with `sea-orm-cli generate entity`. This has become unstable after moving to SeaORM 2 dense entities because MySQL-specific column types such as `MEDIUMTEXT` and `LONGBLOB` are generated as custom column types with incorrect `ignore` or `select_as = "text"` attributes.

The project will switch to a SeaORM entity-first schema workflow. SeaORM entities become the schema source of truth, SeaORM migrations become the database change execution mechanism, and SQL files become reference or export artifacts rather than the driver of schema changes.

## Goals

- Make `src/infra/repo/entities` the authoritative schema definition.
- Introduce `sea-orm-migration` as the standard migration mechanism.
- Preserve selected existing data by migrating it from the old `digital_companion` database into a new database.
- Stop using `digital_companion` as the runtime database after migration.
- Audit live database schema, SQL files, and entity definitions before establishing a baseline.
- Fix MySQL-specific type mappings in entities so dense `ModelEx` and `ActiveModelEx` remain usable.
- Require dependency changes to be made through CLI commands such as `cargo add`, not by directly editing dependency files.

## Non-Goals

- Do not keep `sea-orm-cli generate entity` as the normal schema workflow.
- Do not migrate QQ bot data.
- Do not preserve all legacy non-AI content by default.
- Do not modify the old `digital_companion` database destructively during migration.
- Do not rely on `database/sql/init.sql` as the ongoing schema authority.

## Primary Decisions

- Entity-first means SeaORM entities are the source of truth for schema shape, column types, nullability, and relations.
- The migration system will be `sea-orm-migration`.
- The rollout strategy is audit first, then baseline, then entity-first migrations.
- The old `digital_companion` database remains a read-only source during data migration.
- A new runtime database will be created. The default proposed database name is `server_rs`; implementation can parameterize the name, but it must not be `digital_companion`.
- Existing data preservation is selective: AI-related data is migrated, QQ bot data is not.

## Architecture

### Entities

`src/infra/repo/entities` remains the home for SeaORM dense entities. These files define the schema contract used by repository implementations and by migrations.

Dense entities are retained because this project wants SeaORM 2 `ModelEx` and `ActiveModelEx` capabilities. Generated entities are no longer blindly accepted. If `sea-orm-cli` is used for comparison, its output is treated as an audit input, not the source of truth.

### Migrations

A new migration area will be introduced using `sea-orm-migration`, following SeaORM conventions. Baseline and future migrations are written from the approved entity definitions.

For a fresh database, the migration chain creates the complete schema. For an existing database migration, the new database is created from migrations and then selected data is copied from `digital_companion`.

### SQL Files

`database/sql/*.sql` is retained temporarily as historical reference and for comparison during the initial audit. Documentation must be updated so contributors no longer follow the old "write SQL first, then generate Rust entity" workflow.

After the migration system is established, SQL files may be exported from migrations or kept only as reference material.

### Database Inspection

Live schema inspection uses dbx as the source for the actual current MySQL structure. The audit compares:

- live MySQL schema from dbx
- `database/sql/*.sql`
- `src/infra/repo/entities`

The audit result is captured as a report before baseline is accepted.

## Data Flow

The new workflow for schema changes is:

```text
edit SeaORM entity
-> write or update SeaORM migration
-> run compile and migration checks
-> optionally export SQL reference
-> deploy migration
```

The old workflow is retired:

```text
edit init.sql
-> apply SQL
-> generate entity
```

## Baseline Strategy

The project will not directly stamp the current database as valid. It will first run a schema audit.

The baseline process is:

1. Inspect `digital_companion` live schema through dbx.
2. Compare live schema with SQL files and current entities.
3. Classify differences as type, nullability, default, index, foreign key, naming, or generated-entity artifact.
4. Fix entity definitions for accepted schema semantics.
5. Create a baseline migration that expresses the corrected entity-first schema.
6. Create the new database and apply baseline migrations there.
7. Selectively migrate approved data from `digital_companion`.
8. Point `.env` and `config.toml` to the new database after validation.

The old database is never dropped by this process.

## Data Migration Scope

Data to migrate:

- user and authentication data, including users and refresh tokens
- conversations, conversation messages, and conversation summaries
- long-term user context such as memories, memory evidence, persona snapshots, and context versions
- RAG and knowledge tables with `knowledge_*`
- web ingestion tables with `web_*`
- AI workflow records such as domain event outbox, vector index jobs, vector index records, publish records, manifests, and audit logs
- risk audit data related to AI conversations

Data not migrated by default:

- all `qq_*` tables
- QQ bot runtime data
- community data
- music data
- psychology content
- mock or fixture data

If a later implementation plan decides to migrate a non-default table, it must name the table and explain why it is needed for AI runtime continuity.

## Type Rules

Entity types must reflect data semantics, not only what `sea-orm-cli` generated.

- MySQL `TEXT`, `MEDIUMTEXT`, and `LONGTEXT` map to `String` or `Option<String>`.
- `MEDIUMTEXT` uses `#[sea_orm(column_type = "custom(\"MEDIUMTEXT\")")]`.
- Text columns must not use `#[sea_orm(ignore)]` when they are real database columns.
- Text columns must not use `select_as = "text"` as a workaround for real text fields.
- MySQL `BLOB`, `MEDIUMBLOB`, and `LONGBLOB` map to `Vec<u8>` or `Option<Vec<u8>>`.
- `LONGBLOB` uses `#[sea_orm(column_type = "custom(\"LONGBLOB\")")]`.
- Blob columns must not be represented as `String`.
- Blob columns must not use `select_as = "text"`.
- JSON columns remain `Json` unless a later change introduces explicit typed newtypes.
- Date and time fields continue using `chrono`, matching the current project configuration.

Examples:

```rust
#[sea_orm(column_type = "custom(\"MEDIUMTEXT\")", nullable)]
pub clean_text: Option<String>,
```

```rust
#[sea_orm(column_type = "custom(\"LONGBLOB\")")]
pub media_data: Vec<u8>,
```

## Error Handling

The schema audit must not silently correct differences. Differences are reported and classified.

Blocking differences include:

- a required entity column missing from live schema
- a live column that would be lost by the baseline without an explicit decision
- incompatible binary/text mappings
- nullable mismatches that can break inserts or reads
- missing primary keys or foreign keys needed by repository logic

During data migration:

- source reads from `digital_companion` are read-only
- target writes go only to the new database
- migration can be retried by rebuilding the new database
- large text and blob fields are copied without text casts
- row counts and key relationship checks are recorded after each migrated table group

## Testing And Verification

Required verification before switching runtime config:

- `cargo check` passes.
- Entity definitions no longer produce missing `ActiveModel` fields for real `MEDIUMTEXT` columns.
- `LONGBLOB` fields compile as `Vec<u8>` where they are still modeled in entities.
- The new database can be created from migrations from an empty state.
- The selected data migration runs against a test or local copy and reports expected row counts.
- Users can log in against the new database.
- Existing conversations, summaries, memories, and persona data can be queried.
- Knowledge, web ingestion, vector index, and risk audit data needed by AI workflows can be read.
- The application starts with `.env` and `config.toml` pointed at the new database.

## Rollout Plan

1. Add migration tooling using CLI commands only.
2. Add schema audit tooling and report generation.
3. Audit `digital_companion`, SQL files, and entities.
4. Fix entity definitions for accepted schema semantics.
5. Add baseline migration for the corrected schema.
6. Create the new database and apply migrations.
7. Run selective data migration from `digital_companion`.
8. Validate application behavior against the new database.
9. Update configuration to use the new database.
10. Update documentation to state entity-first schema ownership.

## Dependency Rule

Dependency files are not edited directly. Any dependency change must be performed through command-line tooling, for example:

```text
cargo add sea-orm-migration
```

The resulting `Cargo.toml` and `Cargo.lock` changes can be reviewed and committed after the command modifies them.

## Acceptance Criteria

This design is complete when:

- the team agrees that entities are the schema source of truth
- `sea-orm-migration` is the approved migration mechanism
- `digital_companion` is treated only as a read-only migration source
- the new database is the future runtime database
- QQ bot data is explicitly excluded from migration
- MySQL text and blob type rules are documented
- schema audit is required before baseline
- dependency changes are restricted to CLI-driven modifications
