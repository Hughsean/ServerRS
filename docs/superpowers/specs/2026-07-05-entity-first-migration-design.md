# Entity-First Hybrid Persistence Migration Design

## Context

ServerRS currently treats `database/sql/*.sql` as the practical schema source, and `src/infra/repo/entities` is regenerated from the database with `sea-orm-cli generate entity`. This has become unstable after moving to SeaORM 2 dense entities because MySQL-specific column types such as `MEDIUMTEXT` and `LONGBLOB` are generated as custom column types with incorrect `ignore` or `select_as = "text"` attributes.

The project will switch to a hybrid persistence workflow. SeaORM entities define the current logical contract, SeaORM migrations define MySQL physical schema history, independent ETL migrates selected legacy data, Qdrant/vector projections are rebuilt from MySQL facts, and subdomains such as QQ Bot and Fresh Context are governed separately.

This is not a pure "generate the database from entities" architecture. The project uses MySQL-specific physical features and external vector projections that cannot be represented safely by SeaORM entities alone.

## Goals

- Make `src/infra/repo/entities` the authoritative Rust-facing logical schema contract.
- Introduce `sea-orm-migration` as the standard migration mechanism.
- Put MySQL physical schema details under migration ownership, including indexes, triggers, generated columns, precision, engine/collation details, and raw SQL where needed.
- Preserve selected existing data by migrating it from the old `digital_companion` database into a new database.
- Rebuild Qdrant collections and vector-index projections from migrated MySQL facts instead of copying legacy vector state.
- Explicitly separate governance for AI Core, QQ Bot, Fresh Context, and non-AI content subdomains.
- Stop using `digital_companion` as the runtime database after migration.
- Audit live database schema, SQL files, and entity definitions before establishing a baseline.
- Fix MySQL-specific type mappings in entities so dense `ModelEx` and `ActiveModelEx` remain usable.
- Require dependency changes to be made through CLI commands such as `cargo add`, not by directly editing dependency files.

## Non-Goals

- Do not keep `sea-orm-cli generate entity` as the normal schema workflow.
- Do not use "Entity automatically generates the database" as the production architecture.
- Do not migrate QQ bot data.
- Do not copy Qdrant collections, vector points, or old vector projection state as durable facts.
- Do not fold Fresh Context into the AI Core migration by default.
- Do not preserve all legacy non-AI content by default.
- Do not modify the old `digital_companion` database destructively during migration.
- Do not rely on `database/sql/init.sql` as the ongoing schema authority.

## Primary Decisions

- Entity-first means SeaORM entities are the source of truth for the current Rust-facing logical contract: models, fields, semantic Rust types, nullability, and relations.
- Entity-first does not replace migrations. Entities define intent; migrations define timeline and physical database evolution.
- Migrations are the source of truth for MySQL physical schema, including database-specific features that entities cannot express safely.
- ETL migration history is separate from schema migration history.
- Qdrant and vector indexes are derived projections and must be rebuilt from migrated MySQL source data.
- QQ Bot, Fresh Context, AI Core, and non-AI content are separate migration/governance domains.
- The migration system will be `sea-orm-migration`.
- The rollout strategy is audit first, then baseline, then entity-first migrations.
- The old `digital_companion` database remains a read-only source during data migration.
- A new runtime database will be created. The default proposed database name is `server_rs`; implementation can parameterize the name, but it must not be `digital_companion`.
- Existing data preservation is selective: AI-related data is migrated, QQ bot data is not.

## Architecture

### Entities

`src/infra/repo/entities` remains the home for SeaORM dense entities. These files define the Rust-facing schema contract used by repository implementations and by migrations.

Dense entities are retained because this project wants SeaORM 2 `ModelEx` and `ActiveModelEx` capabilities. Generated entities are no longer blindly accepted. If `sea-orm-cli` is used for comparison, its output is treated as an audit input, not the source of truth.

Entities should express semantic types first. For example, text content is `String` or `Option<String>`, and binary media is `Vec<u8>` or `Option<Vec<u8>>`. MySQL storage width such as `MEDIUMTEXT` or `LONGBLOB` is a physical mapping decision that migrations must implement consistently with the entity intent.

Entities must not be treated as a complete physical schema specification. They are allowed to omit details that belong to MySQL physical design, as long as migrations and audit reports cover those details.

### Migrations

A new migration area will be introduced using `sea-orm-migration`, following SeaORM conventions. Baseline and future migrations are written from the approved entity definitions plus explicit physical schema requirements.

For a fresh database, the migration chain creates the complete schema. For an existing database migration, the new database is created from migrations and then selected data is copied from `digital_companion`.

Migrations are the only approved way to mutate persistent shared databases. They preserve schema history and make changes repeatable across environments.

Migrations own MySQL physical schema details, including:

- table engine, charset, and collation choices
- column precision, scale, unsigned-ness, generated columns, and default expressions
- primary keys, foreign keys, unique keys, regular indexes, prefix indexes, and fulltext indexes
- triggers and database-specific raw SQL
- migration-only operational tables such as ETL ID mapping records
- compatibility fixes needed to move from old schema shape to new schema shape

SeaQuery DDL should be used when it expresses the physical requirement clearly. `execute_unprepared` or `execute_raw` is acceptable for MySQL-specific features that SeaQuery or SeaORM entities cannot represent safely.

### ETL

Historical data migration is a separate ETL process, not a schema migration. Schema migrations create the target database shape. ETL copies and transforms selected data from `digital_companion` into the new database, records ID mappings, and emits migration reports.

The ETL process is domain-aware. It must not infer "all tables" from the schema. It migrates only approved domains and explicitly skips excluded domains.

### Qdrant And Vector Projections

Qdrant collections, vector points, payload indexes, and MySQL tables that represent vector projection state are treated as derived artifacts. They are not durable source-of-truth data.

After MySQL facts are migrated, vector collections and indexes are rebuilt by the normal indexing pipeline using the target database, current embedding configuration, current chunking policy, and current collection definitions. Old Qdrant data is not copied.

This avoids mismatches between migrated relational IDs, chunk manifests, embedding model versions, Qdrant point IDs, and payload schemas.

### Subdomain Governance

The migration is split into explicit governance domains:

- AI Core: users, auth, conversations, summaries, memories, persona/context, RAG source facts, web ingestion facts, workflow audit, and risk audit.
- Vector Projection: Qdrant collections and vector-index projection tables, always rebuilt.
- QQ Bot: all `qq_*` tables and QQ runtime state, excluded from this migration.
- Fresh Context: `fresh_*` tables and short-lived news/trend context, governed separately because of TTL, source freshness, and expiration semantics.
- Non-AI Content: community, music, psychology, mock data, and other content domains, excluded unless a later implementation plan explicitly opts in.

Each domain has its own migration decision. A table can move between domains only through an explicit spec update or implementation-plan decision.

### Intent Versus Timeline

SeaORM entities and migrations have different responsibilities:

- Entity: the current desired schema intent and Rust-facing data contract.
- Migration: the ordered history that moves a database from one valid state to the next.

Every intentional entity change that affects database shape requires a matching migration. A migration without an entity change is allowed only for operational database changes that do not alter the Rust-facing model, such as index tuning or metadata cleanup.

### Schema Sync Guardrails

Automatic schema sync is not part of the normal workflow.

- Development databases use migrations by default.
- Production and staging databases use migrations only.
- Schema sync is allowed only for disposable scratch databases, must require an explicit guard such as `ALLOW_SCHEMA_SYNC=1`, and must never target `digital_companion`, staging, production, or any shared developer database.
- Runtime startup must not auto-sync schema.
- CI or local verification should detect drift by comparing entities, migrations, and live schema audit output.

This prevents the failure mode where a developer syncs a dev database directly, the database drifts from the migration timeline, and later migrations fail or behave differently from production.

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
-> write or update SeaORM migration with required MySQL physical details
-> run compile, migration, and drift checks
-> optionally export SQL reference
-> deploy migration
```

The development workflow is the same as production in principle: apply migrations to move the database forward. A disposable scratch database may be rebuilt from migrations at any time, but it must not become the source for entity or migration generation.

The new workflow for historical data movement is:

```text
create target schema with migrations
-> run domain-aware ETL from read-only digital_companion
-> record ID mappings and row-count reports
-> rebuild Qdrant/vector projections from target MySQL data
-> validate application reads against the new database and rebuilt vector collections
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
4. Fix entity definitions for accepted logical semantics.
5. Define physical schema requirements that entities cannot express, including triggers, generated columns, index details, precision, and MySQL-specific raw SQL.
6. Create a baseline migration that expresses the corrected logical schema and physical MySQL schema.
7. Create the new database and apply baseline migrations there.
8. Selectively migrate approved data from `digital_companion`.
9. Rebuild Qdrant collections and vector projection state from the migrated target database.
10. Point `.env` and `config.toml` to the new database after validation.

The old database is never dropped by this process.

## Data Migration Scope

Data to migrate:

- user and authentication data, including users and refresh tokens
- conversations, conversation messages, and conversation summaries
- long-term user context such as memories, memory evidence, persona snapshots, and context versions
- RAG and knowledge source-of-truth tables with `knowledge_*`, excluding vector projection tables that can be rebuilt
- web ingestion tables with `web_*`
- AI workflow records such as domain event outbox, publish records, non-vector manifests, and audit logs
- risk audit data related to AI conversations

Data not migrated by default:

- all `qq_*` tables
- QQ bot runtime data
- all `fresh_*` tables and Fresh Context runtime state
- Qdrant collections, vector points, payload indexes, and old vector projection state
- vector index jobs, vector index records, vector manifests, and embedding projection records that can be rebuilt
- community data
- music data
- psychology content
- mock or fixture data

If a later implementation plan decides to migrate a non-default table, it must name the table and explain why it is needed for AI runtime continuity.

Vector-related source facts are different from vector projections. Knowledge documents, chunks, source pages, and publish records may be migrated because they are relational source facts. Qdrant points and projection records are rebuilt because they depend on embedding model, chunking policy, target IDs, and collection payload schema.

## Data Migration DAG

Selective data migration must follow an explicit dependency graph. The initial DAG is:

1. Users and base identity tables.
2. Authentication tables such as refresh tokens.
3. Conversations.
4. Conversation messages and conversation summaries.
5. User context tables such as memories, memory evidence, persona snapshots, and context versions.
6. Knowledge and web ingestion root tables such as sources, source URLs, pages, crawl jobs, ingestion runs, documents, chunks, publish records, and non-vector manifests.
7. Workflow and audit tables such as domain event outbox, web ingestion audit logs, and post-conversation risk audits.
8. Vector rebuild inputs are validated, then vector projection tables and Qdrant collections are rebuilt rather than migrated.

Each group is migrated after its parent groups have been inserted and validated. Tables with foreign keys use the database constraints as the hard dependency order. Tables with logical references but no foreign key still follow this DAG.

## ID Mapping Strategy

The default migration strategy is to preserve primary key values when copying rows into the new empty database. Preserving IDs minimizes foreign key rewrites and reduces migration risk.

Even when IDs are preserved, the migration process must maintain an ID mapping record for every migrated table:

```text
table_name, legacy_id, new_id
```

If a row cannot preserve its legacy ID because of a collision, reserved range, type change, or schema correction, the migration assigns a new ID and records the mapping. All child rows resolve foreign keys through the mapping, never by assuming `legacy_id == new_id`.

Mapping records can be in a dedicated migration table in the new database or in an external migration report, but they must be durable enough to audit a completed migration and resume or debug a failed one.

ID mappings are also inputs to vector rebuild jobs. Any payload that includes relational IDs must use target IDs resolved through the mapping, not legacy IDs.

## Vector Rebuild Strategy

Vector rebuild happens after relational ETL validates the source facts in the target database.

The rebuild process:

1. Drops or recreates target Qdrant collections according to current collection configuration.
2. Reads target MySQL source facts such as published chunks, memory records, summaries, and enabled knowledge records.
3. Recomputes embeddings using the current embedding provider, model, and dimension.
4. Writes new Qdrant points with payloads based on target database IDs.
5. Recreates payload indexes required by retrieval filters.
6. Rebuilds any MySQL projection tables that track vector indexing status or Qdrant point references.
7. Validates counts and sample retrievals before runtime cutover.

Old Qdrant collections and old vector projection records are never treated as source-of-truth data. They can be kept as backup for debugging, but they are not copied into the new runtime state.

## Type Rules

Entity types must reflect semantic data intent, not only what `sea-orm-cli` generated and not only the physical MySQL storage width.

- MySQL `TEXT`, `MEDIUMTEXT`, and `LONGTEXT` map to `String` or `Option<String>`.
- MySQL text width is a migration-layer physical mapping. Use the best typed API available in the locked SeaQuery/SeaORM version; when no typed API exists, use an explicit custom type such as `custom("MEDIUMTEXT")`.
- Text columns must not use `#[sea_orm(ignore)]` when they are real database columns.
- Text columns must not use `select_as = "text"` as a workaround for real text fields.
- MySQL `BLOB`, `MEDIUMBLOB`, and `LONGBLOB` map to `Vec<u8>` or `Option<Vec<u8>>`.
- MySQL blob width is a migration-layer physical mapping. Use typed MySQL extension APIs where the locked SeaQuery version provides them; otherwise use explicit custom types.
- Blob columns must not be represented as `String`.
- Blob columns must not use `select_as = "text"`.
- JSON columns remain `Json` unless a later change introduces explicit typed newtypes.
- Date and time fields continue using `chrono`, matching the current project configuration.

For the currently locked project dependencies, `sea-query 1.0.1` exposes core `Text` and `Blob` column types and MySQL extension blob variants such as `LongBlob`, but it does not expose core `MediumText` or `LongText` variants. That means migrations may need custom MySQL text types unless a later dependency upgrade, performed through CLI tooling, provides first-class typed APIs.

Entity examples:

```rust
#[sea_orm(column_type = "custom(\"MEDIUMTEXT\")", nullable)]
pub clean_text: Option<String>,
```

```rust
#[sea_orm(column_type = "custom(\"LONGBLOB\")")]
pub media_data: Vec<u8>,
```

These attributes are mechanical reflections of the physical mapping where SeaORM needs them. The design rule remains semantic-first: `String` means text content, and `Vec<u8>` means bytes. The migration layer owns the exact MySQL storage width.

## Error Handling

The schema audit must not silently correct differences. Differences are reported and classified.

Blocking differences include:

- a required entity column missing from live schema
- a live column that would be lost by the baseline without an explicit decision
- incompatible binary/text mappings
- nullable mismatches that can break inserts or reads
- missing primary keys or foreign keys needed by repository logic
- missing MySQL physical objects that affect behavior, such as triggers, generated columns, expression defaults, required indexes, or precision/scale
- cross-domain table coupling that would migrate excluded QQ, Fresh Context, or non-AI content data through an AI Core path

During data migration:

- source reads from `digital_companion` are read-only
- target writes go only to the new database
- migration can be retried by rebuilding the new database
- large text and blob fields are copied without text casts
- foreign keys are resolved through the migration DAG and ID mapping records
- row counts and key relationship checks are recorded after each migrated table group
- vector rebuild failures do not trigger fallback copying from old Qdrant; they block cutover until rebuild or source data issues are fixed
- excluded subdomains remain excluded even if their tables have foreign keys or logical references from migrated domains; those references require explicit handling or omission decisions

## Testing And Verification

Required verification before switching runtime config:

- `cargo check` passes.
- Entity definitions no longer produce missing `ActiveModel` fields for real `MEDIUMTEXT` columns.
- `LONGBLOB` fields compile as `Vec<u8>` where they are still modeled in entities.
- The new database can be created from migrations from an empty state.
- The selected data migration runs against a test or local copy and reports expected row counts.
- The migration DAG is documented in the migration report and each group validates before the next group runs.
- ID mapping records exist for every migrated table with primary keys, even when IDs are preserved.
- QQ Bot, Fresh Context, and non-AI content tables are absent from the default ETL output unless explicitly opted in.
- Qdrant collections and vector projection tables are rebuilt from target MySQL source facts, not copied from legacy state.
- Retrieval smoke tests pass against rebuilt Qdrant collections.
- Users can log in against the new database.
- Existing conversations, summaries, memories, and persona data can be queried.
- Knowledge, web ingestion, rebuilt vector retrieval, and risk audit data needed by AI workflows can be read.
- The application starts with `.env` and `config.toml` pointed at the new database.

## Rollout Plan

1. Add migration tooling using CLI commands only.
2. Add schema audit tooling and report generation.
3. Add guardrails that prevent schema sync outside disposable scratch databases.
4. Audit `digital_companion`, SQL files, entities, and MySQL physical objects.
5. Fix entity definitions for accepted logical semantics.
6. Add baseline migration for the corrected logical and physical schema.
7. Create the new database and apply migrations.
8. Run selective data migration from `digital_companion` using the approved DAG and ID mapping.
9. Rebuild Qdrant collections and vector projection tables from target MySQL facts.
10. Validate application behavior against the new database and rebuilt vector collections.
11. Update configuration to use the new database.
12. Update documentation to state hybrid entity/migration/ETL/vector ownership.

## Dependency Rule

Dependency files are not edited directly. Any dependency change must be performed through command-line tooling, for example:

```text
cargo add sea-orm-migration
```

The resulting `Cargo.toml` and `Cargo.lock` changes can be reviewed and committed after the command modifies them.

## Acceptance Criteria

This design is complete when:

- the team agrees that entities are the logical schema contract, not a physical schema generator
- `sea-orm-migration` is the approved migration mechanism
- MySQL physical schema is owned by migrations
- ETL history is owned by dedicated migration tooling and reports
- `digital_companion` is treated only as a read-only migration source
- the new database is the future runtime database
- QQ bot data is explicitly excluded from migration
- Fresh Context data is explicitly governed separately from AI Core
- Qdrant and vector projection state is rebuilt instead of copied
- MySQL text and blob type rules are documented
- schema sync is guarded and excluded from normal dev, staging, and production workflows
- entities and migrations are documented as intent versus timeline
- selective data migration has an explicit DAG and ID mapping strategy
- schema audit is required before baseline
- dependency changes are restricted to CLI-driven modifications
