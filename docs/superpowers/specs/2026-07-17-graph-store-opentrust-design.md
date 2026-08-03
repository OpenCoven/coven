# Graph Store v1 and OpenTrust Integration Design

Status: design
Date: 2026-07-17
Primary repo: `OpenCoven/coven`
Later consumers: `OpenCoven/coven-cave`, `OpenCoven/coven-docs`, optional OpenClaw plugin/client work

This design files the OpenClaw-migrated board item "Phase 2: Design local graph store and OpenTrust integration path." Do not refer to this work as plain "Phase 2" in Coven docs or issues: `coven-threads` already uses Phase 2 for the Ward daemon integration gate.

## Decision

Use daemon-owned typed SQLite tables in `<COVEN_HOME>/coven.sqlite3` as the first and canonical graph store.

Kuzu, Neo4j, OpenTrust-style artifacts, `.af`, and vector stores are export, reference, or acceleration surfaces only. They must not become sources of authority for Coven daemon state.

The Rust daemon remains the authority boundary. Clients may request graph reads and writes through `/api/v1`, but they do not write graph tables directly and do not enforce policy.

## Existing authority boundaries

The graph store must layer under the existing Coven and `coven-threads` trust model:

- `store::open_store()` initializes daemon-owned SQLite state.
- `coven_threads_core::WARD_AUDIT_SCHEMA_SQL` owns the append-only `ward_audit` ledger.
- `threads_gate::WARD_MANIFEST_SCHEMA_SQL` owns per-familiar protected-surface baselines.
- `threads_gate::gate_protected_edits()` validates protected familiar edits and stages `DegradeToProposal` outcomes in `<COVEN_HOME>/pending/`.
- `Ward::apply` remains the final materialized familiar edit boundary.
- `POST /api/v1/familiars/:id/edits` is the daemon's protected familiar write path.

Graph APIs may observe Ward and threads state. They must not create a parallel authority path around it.

## Repo targets

| Repo | Role |
| --- | --- |
| `OpenCoven/coven` | Primary implementation target: schema, migrations, Rust store module, daemon APIs, API contract docs, tests |
| `OpenCoven/coven-cave` | Later read-only UI consumer for graph, evidence, temporal diff, and provenance views |
| `OpenCoven/coven-docs` | Public documentation mirror after the daemon contract stabilizes |
| `OpenClaw/openclaw` | Later optional client/plugin integration; no direct database writes |

## Storage layout

Add `crates/coven-cli/src/graph_store.rs` with `GRAPH_SCHEMA_SQL`, initialized from `store::open_store()` after the core store schema and Ward/threads schemas are initialized.

### Tables

#### `graph_nodes`

Typed entity records:

- `id INTEGER PRIMARY KEY`
- `kind TEXT NOT NULL`
- `stable_key TEXT NOT NULL`
- `display_name TEXT`
- `properties_json TEXT NOT NULL DEFAULT '{}'`
- `created_at TEXT NOT NULL`
- `updated_at TEXT NOT NULL`
- `UNIQUE(kind, stable_key)`

Examples include familiar, project, session, event, file, claim subject, and external artifact.

#### `graph_edges`

Typed relationships between nodes:

- `id INTEGER PRIMARY KEY`
- `source_node_id INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE`
- `target_node_id INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE`
- `kind TEXT NOT NULL`
- `properties_json TEXT NOT NULL DEFAULT '{}'`
- `valid_from TEXT`
- `valid_until TEXT`
- `status TEXT NOT NULL DEFAULT 'active'`
- `created_at TEXT NOT NULL`
- `updated_at TEXT NOT NULL`

Do not use a table-level `UNIQUE(source_node_id, target_node_id, kind, valid_from)`
constraint: SQLite treats `NULL` values as distinct, so it would allow duplicate
timeless edges. Use the paired partial unique indexes specified below.

#### `graph_claims`

Factual assertions with an object node or literal value:

- `id INTEGER PRIMARY KEY`
- `subject_node_id INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE`
- `predicate TEXT NOT NULL`
- `object_node_id INTEGER REFERENCES graph_nodes(id) ON DELETE SET NULL`
- `object_value_json TEXT`
- `confidence REAL`
- `valid_from TEXT`
- `valid_until TEXT`
- `status TEXT NOT NULL DEFAULT 'active'`
- `created_at TEXT NOT NULL`
- `updated_at TEXT NOT NULL`

Exactly one of `object_node_id` or `object_value_json` must be present. Enforce this with a SQLite `CHECK` constraint and typed Rust validation.

#### `graph_evidence`

Source material supporting claims and edges:

- `id INTEGER PRIMARY KEY`
- `source_type TEXT NOT NULL`
- `source_id TEXT`
- `source_uri TEXT`
- `session_id TEXT`
- `event_id TEXT`
- `excerpt TEXT`
- `content_hash TEXT NOT NULL`
- `observed_at TEXT NOT NULL`
- `metadata_json TEXT NOT NULL DEFAULT '{}'`
- `created_at TEXT NOT NULL`

`source_type` examples include `session_event`, `ward_audit`, `file`, `manual_note`, and `opentrust_export`.

#### `graph_claim_evidence`

Many-to-many links from claims to evidence:

- `claim_id INTEGER NOT NULL REFERENCES graph_claims(id) ON DELETE CASCADE`
- `evidence_id INTEGER NOT NULL REFERENCES graph_evidence(id) ON DELETE CASCADE`
- `relation TEXT NOT NULL DEFAULT 'supports'`
- `created_at TEXT NOT NULL`
- `PRIMARY KEY(claim_id, evidence_id, relation)`

#### `graph_edge_evidence`

Many-to-many links from edges to evidence:

- `edge_id INTEGER NOT NULL REFERENCES graph_edges(id) ON DELETE CASCADE`
- `evidence_id INTEGER NOT NULL REFERENCES graph_evidence(id) ON DELETE CASCADE`
- `relation TEXT NOT NULL DEFAULT 'supports'`
- `created_at TEXT NOT NULL`
- `PRIMARY KEY(edge_id, evidence_id, relation)`

#### `graph_provenance`

Append-only ledger for graph writes:

- `id INTEGER PRIMARY KEY`
- `target_type TEXT NOT NULL`
- `target_id INTEGER NOT NULL`
- `action TEXT NOT NULL`
- `actor_type TEXT NOT NULL`
- `actor_id TEXT`
- `session_id TEXT`
- `event_id TEXT`
- `request_id TEXT`
- `payload_hash TEXT NOT NULL`
- `details_json TEXT NOT NULL DEFAULT '{}'`
- `created_at TEXT NOT NULL`

Required actions:

- `node_upserted`
- `edge_upserted`
- `claim_asserted`
- `claim_retracted`
- `evidence_attached`
- `embedding_ref_updated`
- `export_generated`

Like `ward_audit`, this table must be append-only. SQLite triggers must abort `UPDATE` and `DELETE`.

#### `graph_ward_links`

Links graph targets to Ward/threads authority records:

- `id INTEGER PRIMARY KEY`
- `graph_target_type TEXT NOT NULL`
- `graph_target_id INTEGER NOT NULL`
- `ward_audit_id INTEGER NOT NULL REFERENCES ward_audit(id) ON DELETE RESTRICT`
- `relation TEXT NOT NULL`
- `created_at TEXT NOT NULL`
- `UNIQUE(graph_target_type, graph_target_id, ward_audit_id, relation)`

Use this when a graph fact derives from or records a protected familiar identity or memory surface. The graph target can be a node, edge, claim, or evidence row.

#### `graph_embedding_refs`

Pointers to derived vector indexes. This table stores no vector payloads:

- `id INTEGER PRIMARY KEY`
- `target_type TEXT NOT NULL`
- `target_id INTEGER NOT NULL`
- `model TEXT NOT NULL`
- `dimensions INTEGER NOT NULL`
- `vector_store TEXT NOT NULL`
- `vector_key TEXT NOT NULL`
- `content_hash TEXT NOT NULL`
- `created_at TEXT NOT NULL`
- `updated_at TEXT NOT NULL`
- `UNIQUE(target_type, target_id, model, vector_store)`

#### `graph_exports`

Metadata for generated export and reference artifacts:

- `id INTEGER PRIMARY KEY`
- `format TEXT NOT NULL`
- `destination_uri TEXT`
- `scope_json TEXT NOT NULL`
- `content_hash TEXT NOT NULL`
- `created_at TEXT NOT NULL`

Export generation must append `graph_provenance(action='export_generated')`.

## Indexes

Create these indexes in v1:

- `idx_graph_nodes_kind_key ON graph_nodes(kind, stable_key)`
- unique `uq_graph_edges_timeless ON graph_edges(source_node_id, target_node_id, kind) WHERE valid_from IS NULL`
- unique `uq_graph_edges_versioned ON graph_edges(source_node_id, target_node_id, kind, valid_from) WHERE valid_from IS NOT NULL`
- `idx_graph_edges_source_kind ON graph_edges(source_node_id, kind, status)`
- `idx_graph_edges_target_kind ON graph_edges(target_node_id, kind, status)`
- `idx_graph_edges_validity ON graph_edges(valid_from, valid_until, status)`
- `idx_graph_claims_subject_predicate ON graph_claims(subject_node_id, predicate, status)`
- `idx_graph_claims_object_node ON graph_claims(object_node_id, predicate, status)`
- `idx_graph_claims_validity ON graph_claims(valid_from, valid_until, status)`
- `idx_graph_evidence_source ON graph_evidence(source_type, source_id)`
- `idx_graph_evidence_session_event ON graph_evidence(session_id, event_id)`
- `idx_graph_claim_evidence_evidence ON graph_claim_evidence(evidence_id)`
- `idx_graph_edge_evidence_evidence ON graph_edge_evidence(evidence_id)`
- `idx_graph_provenance_target ON graph_provenance(target_type, target_id, created_at)`
- `idx_graph_provenance_session_event ON graph_provenance(session_id, event_id)`
- `idx_graph_ward_links_target ON graph_ward_links(graph_target_type, graph_target_id)`
- `idx_graph_ward_links_audit ON graph_ward_links(ward_audit_id)`
- `idx_graph_embedding_refs_target ON graph_embedding_refs(target_type, target_id)`

## Migration plan

Use Coven's existing idempotent SQLite initialization style:

1. Add `GRAPH_SCHEMA_SQL` to `graph_store.rs`.
2. Execute it from `store::open_store()` after Ward/threads schema initialization.
3. Record `store_meta.graph_schema_version = 1`.
4. Keep v1 migrations additive and idempotent.
5. Add future `ensure_graph_*` migrations only after v1 is in use.
6. Fail graph writes closed if schema initialization fails.

Do not rewrite existing session, event, Ward, or threads tables as part of this work.

## Graph and vector separation

SQLite graph tables are authoritative. Vector indexes are rebuildable accelerators.

- Graph query, evidence lookup, and temporal diff APIs must work without vector availability.
- `graph_embedding_refs` stores only target references, model metadata, vector store identity, vector key, and content hash.
- Missing or stale vector references may degrade retrieval ranking, but never graph truth.
- Rebuilding or deleting vector indexes must not delete graph facts.
- Vector generation must record provenance when it updates `graph_embedding_refs`.

## Ward and graph authority rules

Every graph write must append a `graph_provenance` row in the same transaction as the graph mutation.

If a graph fact derives from or mutates a protected familiar surface, it must either:

1. Link to an existing `ward_audit` row through `graph_ward_links`.
2. Route the mutation through `POST /api/v1/familiars/:id/edits` first, allowing `threads_gate` and `Ward::apply` to permit, stage, or reject it.

Graph APIs must not write protected familiar identity or memory files directly. A graph write that would imply a protected surface mutation must fail closed unless it can prove the Ward path authorized the change.

## API surface

The endpoint labels below are fully qualified `/api/v1/*` routes.

### `POST /api/v1/graph/query`

Bounded graph traversal and claim lookup.

Request fields:

- `start`: node selector by `id` or `(kind, stableKey)`
- `edgeKinds`: optional allowlist
- `claimPredicates`: optional allowlist
- `direction`: `out`, `in`, or `both`
- `maxDepth`: bounded integer with a conservative daemon default
- `asOf`: optional timestamp for temporal filtering
- `includeEvidence`: boolean
- `limit`: bounded integer

Response fields:

- `nodes`
- `edges`
- `claims`
- `evidence` when requested
- `provenanceCursor` when more provenance is available

### `GET /api/v1/graph/:targetType/:targetId/evidence`

Evidence and provenance lookup for a graph target.

Response fields:

- `target`
- `evidence`
- `wardLinks`
- `provenance`

For protected familiar surfaces, include linked `ward_audit` metadata when present.

### `POST /api/v1/graph/diff`

Temporal graph diff between two points in time.

Request fields:

- `scope`: node selector, kind selector, or graph-wide with strict limits
- `from`
- `to`
- `includeEvidence`: boolean
- `includeWardLinks`: boolean
- `limit`

Response fields:

- `added`
- `changed`
- `removed`
- `retracted`
- `provenance`
- `wardAudit`

Temporal diff composes graph provenance with `ward_audit` where graph targets link to Ward-controlled surfaces.

### `POST /api/v1/graph/exports`

Generate an export artifact for Kuzu, Neo4j, `.af`, or OpenTrust-style interchange.

Exports are snapshots or derivations, not authoritative stores. Export creation records `graph_exports` and appends `graph_provenance(action='export_generated')`.

## Capability negotiation

Expose graph support in `GET /api/v1/health` and `/api/v1/capabilities` only after the API is implemented.

Suggested capability shape:

```json
{
  "graph": {
    "query": true,
    "evidence": true,
    "temporalDiff": true,
    "exports": ["kuzu", "neo4j", "opentrust"]
  }
}
```

Clients must branch on daemon capabilities instead of assuming graph endpoints exist.

## OpenTrust integration path

OpenTrust integration is a mapping and export layer over Coven authority, not a replacement for it.

- Coven graph nodes map to OpenTrust entities.
- Coven graph edges map to OpenTrust relationships.
- Coven graph claims map to factual assertions.
- `graph_evidence`, `graph_provenance`, and linked `ward_audit` rows map to provenance and evidence trails.
- Protected familiar identity state remains governed by Ward and threads in Coven.

Round-trip imports from OpenTrust-style artifacts must produce proposals by default for protected surfaces. They may insert non-protected graph facts directly only through daemon graph write APIs with provenance.

## Cave integration path

`coven-cave` remains a read-only graph client until daemon writes and capability negotiation stabilize.

Initial UI surfaces:

- graph query explorer
- evidence and provenance side panel
- temporal diff view
- Ward-linked protected-surface history
- export trigger and status view

Cave must call daemon APIs over the local socket and must not read or write `coven.sqlite3` directly.

## Test plan

Add targeted Rust tests for:

- schema initialization in a fresh temporary `COVEN_HOME`
- idempotent `GRAPH_SCHEMA_SQL`
- append-only triggers for `graph_provenance`
- node upsert uniqueness by `(kind, stable_key)`
- timeless and versioned edge uniqueness under SQLite `NULL` semantics
- edge traversal indexes and bounded query behavior
- edge evidence linkage and reverse evidence lookup
- claim assertion with evidence and provenance in one transaction
- protected-surface graph writes requiring Ward linkage or Ward edit routing
- vector reference deletion and rebuild not deleting graph facts
- temporal diff filtering by validity and provenance timestamps
- API structured errors for invalid selectors, invalid limits, and missing capabilities

Docs-only filing of this design does not require Rust tests. Implementation pull requests must run the relevant Coven Rust checks.

## Implementation sequence

1. Add `graph_store.rs` with schema, indexes, append-only triggers, and schema-version initialization.
2. Add typed Rust helpers for graph node, edge, claim, claim/edge evidence links, provenance, Ward link, embedding reference, and export writes.
3. Add transaction helpers that require provenance on every graph mutation.
4. Add daemon API request and response types and handlers.
5. Add capability negotiation.
6. Update `docs/API-CONTRACT.md` and `docs/CLIENT-INTEGRATION.md`.
7. Add Cave read-only UI work in a separate repository and pull request after the Coven API is stable.
8. Add Kuzu, Neo4j, and OpenTrust export generation after the core graph query and evidence APIs land.

## Non-goals for v1

- No direct Kuzu or Neo4j authority store.
- No vector payload storage in SQLite.
- No client-side direct database writes.
- No bypass of `threads_gate`, `Ward::apply`, `ward_manifest`, or `ward_audit`.
- No graph-based mutation of protected familiar surfaces without Ward authorization.
- No public docs promise until the daemon API contract and capabilities are implemented.
