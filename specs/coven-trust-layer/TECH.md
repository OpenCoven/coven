# Coven Trust Layer — TECH

**Status:** Draft v1 · 2026-05-26
**Companion to:** [PRODUCT.md](./PRODUCT.md)

This document maps the PRODUCT contract onto the current code and lists the deltas needed to enforce it end-to-end.

## Current code mapped to PRODUCT

| PRODUCT element | Code location | State |
|---|---|---|
| Redaction filter (regex + field-name) | `crates/coven-cli/src/privacy.rs` | Implemented |
| Privacy config (`~/.coven/privacy.toml`) | `privacy.rs` (config loader) | Implemented |
| `persist_raw_artifacts` default false | `privacy.rs` config defaults | Implemented |
| At-rest encryption (XChaCha20Poly1305) | `crates/coven-cli/src/encrypted_artifacts.rs` | Implemented |
| Per-session key file | `~/.coven/keys/session-artifacts.key`, mode 0600 | Implemented |
| `sensitive_artifacts` table | `crates/coven-cli/src/store.rs:84-143` | Implemented |
| `events.sensitive` flag | `store.rs` | **Half-built**: flag is set but the "raw-when-sensitive routes to encrypted artifact" branch is not guaranteed |
| Retention pruning | (none) | **Missing** |
| `coven doctor` privacy checks | `crates/coven-cli/src/main.rs` doctor subcommand | **Partial** — exists but doesn't check the PRODUCT contract row-by-row |
| TCP gateway suppression of artifact bodies | (none; no TCP listener yet) | **Missing** — depends on coven-gateway-wire spec |
| Provenance columns on `events` | (none) | **Missing** — see coven-session-artifacts spec |

## Schema changes

### `events` table additions

```sql
ALTER TABLE events ADD COLUMN producer_harness TEXT;       -- 'claude' | 'codex' | future
ALTER TABLE events ADD COLUMN producer_run_id TEXT;        -- harness-supplied conversation/run id
ALTER TABLE events ADD COLUMN producer_cwd TEXT;           -- cwd at time of event
ALTER TABLE events ADD COLUMN redaction_version INTEGER NOT NULL DEFAULT 1;
                                                            -- bumps when the redaction filter changes;
                                                            -- old rows are NOT re-redacted, but consumers can tell
ALTER TABLE events ADD COLUMN expires_at INTEGER;           -- epoch-seconds; NULL = never (use class default)
```

All three `producer_*` fields are nullable for backfill compatibility, but new inserts MUST populate them. The session record already carries `harness`; events carry it again because long-lived sessions can change `producer_cwd` mid-flight, and a stream-mode session can interleave events from sub-processes spawned by the agent.

### `sensitive_artifacts` table additions

```sql
ALTER TABLE sensitive_artifacts ADD COLUMN byte_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sensitive_artifacts ADD COLUMN sha256 BLOB;     -- of plaintext, for "did this change?" without decrypt
```

The hash is computed before encryption, stored alongside ciphertext. CastCodes can use it to dedupe and to show "changed file" lists without ever decrypting.

### New table: `prune_runs`

```sql
CREATE TABLE prune_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    events_deleted INTEGER NOT NULL DEFAULT 0,
    artifacts_deleted INTEGER NOT NULL DEFAULT 0,
    bytes_reclaimed INTEGER NOT NULL DEFAULT 0,
    trigger TEXT NOT NULL   -- 'startup' | 'schedule' | 'manual'
);
```

`coven session list --include-pruned-counts` reads from this table to satisfy PRODUCT acceptance #4.

## Redaction enforcement: write-time only

Today, redaction can be applied at either write or read time depending on the call site. PRODUCT requires write-time only for the "Stored — yes, redacted" classes.

**Change:** `store::insert_event()` becomes the single entrypoint, and it:

1. Looks up the event's `kind` against a static `REDACTION_POLICY` map:
   - `transcript`, `tool_call`, `verification`, `handoff`, `summary` → apply `privacy::redact_value()` to `payload_json` before insert.
   - `raw_artifact` → route to `encrypted_artifacts::encrypt_and_store()` instead; insert an `events` row with a placeholder body referencing the artifact id.
2. Sets `redaction_version = privacy::CURRENT_REDACTION_VERSION`.
3. Sets `sensitive` based on the policy, not on the caller.

Read paths (`store::get_events`, etc.) no longer redact — the data is already clean. This eliminates the failure mode where a read path forgot to redact.

## Pruning implementation

New module `crates/coven-cli/src/retention.rs`:

```rust
pub struct Retention {
    log_retention: Duration,
    raw_artifact_retention: Duration,
}

impl Retention {
    pub fn run_once(&self, store: &Store, trigger: PruneTrigger) -> Result<PruneStats>;
    pub fn spawn_scheduler(&self, store: Arc<Store>) -> JoinHandle<()>;
}
```

- `run_once` deletes expired `events` and `sensitive_artifacts`, then `PRAGMA incremental_vacuum`, then writes a `prune_runs` row.
- `spawn_scheduler` ticks every 6 hours.
- Called from `daemon::start()` once on boot, then the scheduler takes over.
- Archived sessions (`sessions.archived_at IS NOT NULL`) are skipped — see PRODUCT.

## `coven doctor` checks

Extend `crates/coven-cli/src/main.rs` `doctor` subcommand to assert each PRODUCT row. Output format:

```
Coven trust layer:
  [ok]   redaction filter loaded (version 1)
  [ok]   persist_raw_artifacts = false (default)
  [ok]   session-artifacts.key present, mode 0600
  [ok]   ~/.coven/token present, mode 0600  (or "not configured" if absent)
  [warn] last prune was 7d ago (expected ≤ 6h)
  [ok]   schema version 7, redaction_version columns present
  [ok]   no raw_artifact events without a sensitive_artifacts row (referential integrity)
```

Each check has a stable id (`trust.redaction_loaded`, `trust.key_perms`, etc.) so CI / CastCodes can consume the report.

## Test plan

Three layers:

1. **Unit tests** (`privacy.rs`, `encrypted_artifacts.rs`, new `retention.rs`):
   - Every regex pattern catches the value it claims to catch and doesn't catch a benign neighbour.
   - Encrypt → decrypt round-trips with correct AAD; wrong AAD fails; tampered ciphertext fails.
   - `run_once` deletes only expired rows; archived sessions are skipped.

2. **Store integration tests** (`crates/coven-cli/tests/store_redaction.rs`, new):
   - `insert_event` of every `kind` in `REDACTION_POLICY` produces a redacted on-disk row for known-bad inputs (private key, `Authorization: Bearer`, JSON `password` field).
   - `raw_artifact` kind never produces a plaintext row in `events.payload_json`.

3. **End-to-end** (driven by `coven doctor` + `coven session show --json`):
   - Boot a daemon, run a synthetic session that emits a known-bad event, fetch over Unix socket, assert the bytes never appeared.
   - Boot a daemon with TCP enabled, fetch over `/v1/sessions/:id/events`, assert artifact bodies are absent and only references are returned.

## Migration

`store::ensure_migrations()` already runs IF-NOT-EXISTS DDL. Add a `migrations/008_trust_layer.sql` (or whatever the next number is) with the column adds and the `prune_runs` table. Existing rows get NULL provenance and `redaction_version = 1`; new inserts populate them.

No data is rewritten — old events keep their old redaction (consumers read `redaction_version` to know what filter applied).

## Dependencies on other specs

- **`coven-gateway-wire`** owns the TCP-side enforcement of "no artifact bodies, no key material." This spec assumes that filter is implemented there, not duplicated here.
- **`coven-session-artifacts`** owns the per-artifact JSON shape. This spec only cares which class each artifact falls into.
- **`coven-handoff-packet`** owns the handoff structure. This spec just says "redact like any other text event."

## Out of scope for v1

- OS keychain for the session-artifacts key.
- Per-session keys (one key per machine for now).
- `excluded_paths` config.
- Decrypt audit log.

Each of those gets a follow-up spec once v1 lands.
