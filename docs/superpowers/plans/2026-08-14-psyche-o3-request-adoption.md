# Psyche O3 Request Adoption Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `psyche.request_adoption.v1` so every bound launch and input is durably adopted before its runtime side effect, exact retries are side-effect-free, and one Psyche launch attempt cannot create two Coven sessions.

**Architecture:** Add a closed request-adoption value object, a process-independent advisory lock keyed only by SHA-256 digests, and an append-only SQLite adoption ledger. Dedicated adopted routes resolve replay/conflict before mutable admission, repeat that resolution under the adoption gate and transaction, and commit adoption before runtime work. The OpenClaw client negotiates capability before using those routes; existing unbound behavior remains compatible.

**Tech Stack:** Rust 2021, Serde/serde_json, SHA-256, fs2 advisory locks, Rusqlite/SQLite, TypeScript, Vitest, Markdown.

---

## Preconditions and scope

- Execute from a fresh issue-keyed implementation worktree based on current `origin/main`; implementation tracker: [OpenCoven/coven#741](https://github.com/OpenCoven/coven/issues/741).
- The normative source is `specs/psyche/O3_CONTRACT_DESIGN.md` at merge commit `af4df52e56511bd6b0b118275eb3d0bee473bf03`.
- Preserve O2's exact `psyche.execution_binding.v1` parsing and comparison. O3 may defer expiry for replay precedence, but must not normalize binding or adoption bytes.
- Do not add adoption lookup, proven-not-adopted/unknown/fence states, redispatch, cancellation acknowledgement, artifact binding, retention expiry, or production child dispatch. Those remain O4-O8 work.
- Do not hold a SQLite writer transaction while acquiring an adoption lock, canonicalizing paths, resolving a familiar, validating a harness, or waiting for maintenance.
- Do not expose adoption keys, request digests, bindings, ledger session ids, or submitted input in errors, event payloads, runtime payloads, lock names, or diagnostics.

## File map

| File | Responsibility |
| --- | --- |
| `crates/coven-cli/src/request_adoption.rs` | Closed O3 wire value, launch attempt scope, exact identity, validation, and deterministic serialization. |
| `crates/coven-cli/src/adoption_gate.rs` | Process-independent, digest-named, sorted advisory locks for request keys and launch attempt scopes. |
| `crates/coven-cli/src/main.rs` | Register the two new Rust modules and preserve CLI sacrifice rendering. |
| `crates/coven-cli/src/store.rs` | Append-only ledger schema, migration reservations, strict readback, replay/conflict resolution, atomic inserts, stale-created exclusion, and typed retention denial. |
| `crates/coven-cli/src/session_launch.rs` | Keep the single fresh-session constructor; update its status documentation for adopted daemon launches. |
| `crates/coven-cli/src/api.rs` | Capability advertisement, dedicated routes, legacy bound-route rejection, ordering, redacted errors, and adopted response shapes. |
| `crates/coven-cli/src/daemon.rs` | Carry internal adoption correlation through the live input event boundary and allow terminal exit to beat activation. |
| `crates/coven-cli/src/event_writer.rs` | Queue and persist optional internal `request_adoption_id` without serializing it into event payloads or records. |
| `crates/coven-cli/src/tui/chat/client.rs` | Preserve the typed store retention error through the chat-client sacrifice surface. |
| `crates/coven-cli/src/tui/chat/app.rs` | Verify failed adopted-session sacrifice leaves the overlay unchanged and renders the canonical denial. |
| `crates/coven-cli/src/tui/sessions.rs` | Qualify session-browser sacrifice guidance and preserve the typed denial. |
| `crates/coven-cli/src/tui/shell.rs` | Qualify magical-TUI sacrifice help/outcomes for retained sessions. |
| `crates/coven-cli/src/tui/cast/plan.rs` | Make the sacrifice plan describe eligibility rather than unconditional deletion. |
| `crates/coven-cli/src/tui/cast/gate.rs` | Make typed confirmation conditional on retention eligibility. |
| `packages/openclaw-coven/src/client.ts` | Add O3 types, validation, capability negotiation, dedicated adopted methods, and response normalization. |
| `packages/openclaw-coven/src/client.test.ts` | Unit-test O3 validation, negotiation, no-POST failures, and adopted response parsing. |
| `packages/openclaw-coven/src/compat.test.ts` | Assert exact O3 routes/bodies and pre-O3 downgrade rejection behavior. |
| `packages/openclaw-coven/src/runtime.test.ts` | Keep typed runtime client doubles complete after the additive client methods. |
| `packages/openclaw-coven/src/fixtures/v2026.4/health-available.json` | Advertise the O3 contract in the available-daemon fixture. |
| `packages/openclaw-coven/src/fixtures/v2026.4/health-daemon-null.json` | Advertise the same additive contract without daemon metadata. |
| `docs/API-CONTRACT.md` | Publish O3 wire shapes, ordering, errors, capability, and exclusions. |
| `docs/reference/api.md` | Keep the public endpoint and capability reference synchronized. |
| `docs/reference/api-contract.md` | Keep the concise contract/capability reference synchronized. |
| `docs/daemon/socket-api.md` | Document the socket routes and additive O3 capability. |
| `docs/SESSION-LIFECYCLE.md` | Document adopted `created`, terminal-wins activation, stale-reaper exclusion, and retention. |
| `docs/sessions/lifecycle.md` | Keep the client-developer lifecycle graph and recovery text consistent. |
| `docs/reference/cli-sacrifice.md` | Remove the unconditional non-running deletion claim and document the adoption denial. |
| `docs/reference/cli-sessions.md` | Qualify the permanent-delete summary with adoption retention. |
| `docs/rituals/sacrifice.md` | Keep the short ritual summary consistent with adoption retention. |
| `docs/rituals/index.md` | Qualify the sacrifice card with adoption retention. |
| `docs/GLOSSARY.md` | Correct the canonical sacrifice definition. |
| `docs/reference/glossary.md` | Correct the public-reference sacrifice definition. |
| `docs/help/session-stuck.md` | Prevent recovery guidance from recommending forbidden adopted-session deletion. |
| `docs/start/coven-tui.md` | Correct `/sacrifice` command help for adopted sessions. |
| `docs/design/cast-phase6-inspection.md` | Remove the stale unconditional sacrifice detail string. |
| `docs/guides/session-operations.md` | Qualify operational deletion guidance. |
| `docs/reference/cli-archive.md` | Prevent archive guidance from promising adopted-session deletion. |
| `docs/reference/cli-kill.md` | Qualify post-kill cleanup guidance. |
| `docs/harnesses/provider-auth.md` | Explain that retained adopted evidence cannot be removed by sacrifice. |
| `README.md` | Correct the top-level ritual matrix. |
| `scripts/check-api-contract-docs.py` | Add `requestAdoptionContracts` to the synchronized health-capability documentation checks. |
| `scripts/check-api-contract-docs-test.py` | Update the guarded capability count and canonical checker fixtures. |
| `specs/psyche/O3_CONTRACT_DESIGN.md` | Record implementation evidence only after every acceptance gate passes. |
| `specs/psyche/PLAN.md` | Change O3 from approved-design to implemented only after the green implementation merge. |

### Task 0: Create the implementation Bead and claim the work

**Files:**
- No repository files.
- External records: GitHub issue `OpenCoven/coven#741`, Bead `coven-psy-o3`.

- [ ] **Step 1: Create the O3 Bead if it does not already exist**

```bash
bd show coven-psy-o3 || bd create \
  --id coven-psy-o3 \
  --type feature \
  --priority 1 \
  --external-ref gh-741 \
  --title "Psyche O3 stable request adoption" \
  --description "Implement the approved O3 request-adoption contract for bound launch/input, one-attempt/one-session, durable replay/conflict, and retention-safe evidence." \
  --acceptance "All O3 contract section 8 tests and repository gates pass; implementation and evidence PRs merge; issue #741 and this Bead contain checkable receipts."
```

Expected: `bd show coven-psy-o3` displays an open feature linked to `gh-741`.

- [ ] **Step 2: Record the worktree and plan**

```bash
bd update coven-psy-o3 --append-notes \
  "Execution plan: docs/superpowers/plans/2026-08-14-psyche-o3-request-adoption.md"
bd show coven-psy-o3
```

Expected: the Bead remains open and names this exact plan.

### Task 1: Implement the closed request-adoption value object

**Files:**
- Create: `crates/coven-cli/src/request_adoption.rs`
- Modify: `crates/coven-cli/src/main.rs:19-73`
- Test: `crates/coven-cli/src/request_adoption.rs`

- [ ] **Step 1: Register the module and write the failing exact-shape tests**

Add `mod request_adoption;` beside `mod execution_binding;` in `main.rs`. Create the module with these tests:

```rust
pub const CONTRACT: &str = "psyche.request_adoption.v1";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    #[test]
    fn parses_exact_three_member_object() {
        let parsed = parse(&json!({
            "contract": CONTRACT,
            "key": "psyche:graph/node_attempt-1",
            "requestDigest": digest('a'),
        }))
        .expect("valid adoption");
        assert_eq!(parsed.key, "psyche:graph/node_attempt-1");
    }

    #[test]
    fn rejects_missing_unknown_and_non_object_shapes() {
        let valid = json!({
            "contract": CONTRACT,
            "key": "key",
            "requestDigest": digest('a'),
        });
        for field in ["contract", "key", "requestDigest"] {
            let mut value = valid.clone();
            value.as_object_mut().unwrap().remove(field);
            assert!(parse(&value).is_err(), "missing {field} must fail");
        }
        let mut unknown = valid;
        unknown["extra"] = json!(true);
        assert!(parse(&json!(null)).is_err());
        assert!(parse(&unknown).is_err());
    }
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p coven-cli --bin coven request_adoption::tests::
```

Expected: compilation fails because `parse` and `RequestAdoption` do not exist.

- [ ] **Step 3: Add the complete closed parser and static-path errors**

```rust
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTRACT: &str = "psyche.request_adoption.v1";
const FIELDS: [&str; 3] = ["contract", "key", "requestDigest"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAdoption {
    pub contract: String,
    pub key: String,
    pub request_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    Missing { path: &'static str },
    Invalid { path: &'static str },
    Unsupported { path: &'static str },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "missing field at {path}"),
            Self::Invalid { path } => write!(f, "invalid field at {path}"),
            Self::Unsupported { path } => write!(f, "unsupported value at {path}"),
        }
    }
}

impl std::error::Error for ValidationError {}

pub fn parse(value: &Value) -> Result<RequestAdoption, ValidationError> {
    let object = value
        .as_object()
        .ok_or(ValidationError::Invalid { path: "requestAdoption" })?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = FIELDS.into_iter().collect::<BTreeSet<_>>();
    if actual.difference(&expected).next().is_some() {
        return Err(ValidationError::Invalid { path: "requestAdoption" });
    }
    if expected.difference(&actual).next().is_some() {
        return Err(ValidationError::Missing { path: "requestAdoption" });
    }
    let string = |field: &str, invalid_path: &'static str| {
        object
            .get(field)
            .ok_or(ValidationError::Missing { path: "requestAdoption" })?
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or(ValidationError::Invalid { path: invalid_path })
    };
    let adoption = RequestAdoption {
        contract: string("contract", "requestAdoption.contract")?,
        key: string("key", "requestAdoption.key")?,
        request_digest: string("requestDigest", "requestAdoption.requestDigest")?,
    };
    adoption.validate()?;
    Ok(adoption)
}

impl RequestAdoption {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.contract != CONTRACT {
            return Err(ValidationError::Unsupported {
                path: "requestAdoption.contract",
            });
        }
        if !valid_key(&self.key) {
            return Err(ValidationError::Invalid {
                path: "requestAdoption.key",
            });
        }
        if !valid_digest(&self.request_digest) {
            return Err(ValidationError::Invalid {
                path: "requestAdoption.requestDigest",
            });
        }
        Ok(())
    }

    pub fn deterministic_json(&self) -> String {
        serde_json::to_string(self).expect("validated request adoption serializes")
    }
}

fn valid_key(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
```

- [ ] **Step 4: Add boundary, byte-exact, and deterministic-serialization tests**

Add table-driven tests for key lengths `0, 1, 255, 256`; every allowed punctuation character; whitespace, Unicode, and `?` rejection; uppercase/wrong-prefix/short digests; unknown contract; mixed-case key round-trip; and two serializations producing identical bytes.

- [ ] **Step 5: Run, format, and commit**

```bash
cargo test -p coven-cli --bin coven request_adoption::tests::
cargo fmt --all -- --check
git add crates/coven-cli/src/request_adoption.rs crates/coven-cli/src/main.rs
git commit -m "feat(psyche): add request adoption value object"
```

Expected: all focused tests pass and formatting is clean.

### Task 2: Add the process-independent AdoptionGate

**Files:**
- Create: `crates/coven-cli/src/adoption_gate.rs`
- Modify: `crates/coven-cli/src/main.rs:19-73`
- Test: `crates/coven-cli/src/adoption_gate.rs`

- [ ] **Step 1: Write failing lock-name and serialization tests**

Tests must prove that lock paths contain only `key-<64 lowercase hex>.lock` or `scope-<64 lowercase hex>.lock`, never caller values; same-key contenders block; disjoint keys proceed; launch locks are acquired in sorted full-path order; and dropping a guard releases all OS locks.

- [ ] **Step 2: Run the focused tests and verify they fail**

```bash
cargo test -p coven-cli --bin coven adoption_gate::tests::
```

Expected: compilation fails because `AdoptionGate` does not exist.

- [ ] **Step 3: Implement digest-only advisory locking**

Register `mod adoption_gate;` and implement:

```rust
use std::{fs, path::{Path, PathBuf}};

use anyhow::{Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};

const LOCK_DIR: &str = "request-adoption-locks";

pub struct AdoptionGate {
    _files: Vec<std::fs::File>,
}

impl AdoptionGate {
    pub fn acquire(
        coven_home: &Path,
        request_key: &str,
        attempt_scope: Option<&[&str]>,
    ) -> Result<Self> {
        crate::daemon::ensure_private_coven_home(coven_home)?;
        let directory = coven_home.join(LOCK_DIR);
        fs::create_dir_all(&directory)
            .with_context(|| format!("failed to create adoption lock directory {}", directory.display()))?;
        let mut paths = vec![lock_path(&directory, "key", &[request_key])];
        if let Some(scope) = attempt_scope {
            paths.push(lock_path(&directory, "scope", scope));
        }
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let file = crate::state_lock::open_lock_file(&path)?;
            file.lock_exclusive()
                .with_context(|| format!("failed to acquire request-adoption lock {}", path.display()))?;
            files.push(file);
        }
        Ok(Self { _files: files })
    }
}

fn lock_path(directory: &Path, kind: &str, fields: &[&str]) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"opencoven:psyche:o3:");
    hasher.update(kind.as_bytes());
    for field in fields {
        hasher.update([0]);
        hasher.update(field.as_bytes());
    }
    directory.join(format!("{kind}-{:x}.lock", hasher.finalize()))
}
```

- [ ] **Step 4: Add a child-process contention test**

Use the test binary's existing subprocess pattern to hold one gate in a child process while the parent verifies a same-key acquisition waits and a different-key acquisition succeeds. This proves process independence rather than only thread serialization.

- [ ] **Step 5: Run and commit**

```bash
cargo test -p coven-cli --bin coven adoption_gate::tests::
git add crates/coven-cli/src/adoption_gate.rs crates/coven-cli/src/main.rs
git commit -m "feat(psyche): serialize request adoption"
```

### Task 3: Persist the append-only adoption ledger and migrate historical bindings

**Files:**
- Modify: `crates/coven-cli/src/store.rs:58-119,475-1190,2011-2483,2889-3019,3267-3299,7283-end`
- Test: `crates/coven-cli/src/store.rs`

- [ ] **Step 1: Write failing fresh-schema, migration, strict-readback, and rollback tests**

Add tests named:

```text
request_adoptions_fresh_schema_has_required_constraints
request_adoptions_migrate_every_bound_session_once
request_adoptions_repeated_migration_repairs_only_missing_reservations
request_adoptions_duplicate_historical_attempt_scope_fails_startup
request_adoptions_migration_rolls_back_on_corrupt_binding
request_adoptions_strict_readback_rejects_corrupt_rows
request_adoptions_raw_update_and_delete_are_rejected
request_adoption_event_correlation_rejects_invalid_insert_and_rebind
request_adoptions_survive_status_archive_summon_and_event_retention
```

Each migration fixture must create an actual pre-O3 database with `sessions.execution_binding_json`, reopen it through `initialize_store`, and inspect both rows and `PRAGMA foreign_key_list(events)`.

- [ ] **Step 2: Run the migration tests and verify they fail**

```bash
cargo test -p coven-cli --bin coven request_adoptions_
cargo test -p coven-cli --bin coven request_adoption_event_correlation
```

Expected: schema/table assertions fail.

- [ ] **Step 3: Add the logical record and resolution types**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAdoptionRecord {
    pub id: String,
    pub adoption_key: Option<String>,
    pub contract: Option<String>,
    pub operation: RequestAdoptionOperation,
    pub request_digest: String,
    pub session_id: String,
    pub execution_binding_json: String,
    pub principal_ref: Option<String>,
    pub project_digest: Option<String>,
    pub graph_id: Option<String>,
    pub node_id: Option<String>,
    pub attempt_id: Option<String>,
    pub adopted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAdoptionOperation {
    Launch,
    Input,
}

#[derive(Debug)]
pub enum AdoptionResolution {
    Absent,
    Replay { adoption_id: String, session: SessionRecord },
    Conflict { field: &'static str },
}
```

Implement strict row conversion that parses `contract`, `operation`, digest, deterministic O2 binding, and launch/input nullability. Corrupt rows return an internal store error; they never become `Absent`.

- [ ] **Step 4: Add the table, indexes, event column, and immutable triggers**

Implement the exact DDL from `specs/psyche/O3_CONTRACT_DESIGN.md` §4.1. Fresh `events` includes nullable `request_adoption_id REFERENCES request_adoptions(id) ON DELETE RESTRICT`. Legacy stores use:

```rust
ensure_column(
    conn,
    "events",
    "request_adoption_id",
    "ALTER TABLE events ADD COLUMN request_adoption_id TEXT
     REFERENCES request_adoptions(id) ON DELETE RESTRICT",
)?;
```

Create the partial unique indexes and all five triggers only after the legacy column exists. Explicitly test null-to-non-null event rebinding, same-session enforcement, input-only enforcement, one-event-per-adoption, and `PRAGMA foreign_keys = ON`.

- [ ] **Step 5: Add restart-idempotent historical reservations**

After `ensure_execution_binding_column`, query every non-null bound session and
parse its exact O2 binding. If any launch adoption already exists for that
session, validate it byte-for-byte and leave it unchanged, whether it is a
historical null-key reservation or a post-O3 keyed adoption. Only a bound
session with no launch adoption receives a reservation with null key/contract.
Use a deterministic UUID v5 derived from the session id and the session's
`created_at` as `adopted_at`. Let the attempt-scope unique index fail startup
when two historical sessions claim one scope; never use `INSERT OR IGNORE` to
choose a winner.

- [ ] **Step 6: Add replay/conflict and atomic insert helpers**

Expose:

```rust
pub fn resolve_launch_adoption(
    conn: &Connection,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<AdoptionResolution>;

pub fn resolve_input_adoption(
    conn: &Connection,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<AdoptionResolution>;

pub fn insert_launch_adoption(
    conn: &Connection,
    adoption_id: &str,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
    adopted_at: &str,
) -> Result<()>;

pub fn insert_input_adoption(
    conn: &Connection,
    adoption_id: &str,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
    adopted_at: &str,
) -> Result<()>;
```

Resolution compares contract, operation, digest, session for input, deterministic binding JSON, and all five launch-scope fields. It checks both global key and launch scope; any non-identical winner returns the documented static field path.

- [ ] **Step 7: Prove the complete conflict matrix and restart behavior**

Test exact replay, each differing identity member, key reuse across
operations/sessions, different key with same launch scope, different attempt
success, concurrent insertion from separate connections, reopen replay, and no
production update/delete helper. Also run status update, archive, summon, and
bounded event-retention operations and prove the adoption row is byte-identical
before and after each operation.

- [ ] **Step 8: Run and commit**

```bash
cargo test -p coven-cli --bin coven request_adoption
cargo test -p coven-cli --bin coven execution_binding
git add crates/coven-cli/src/store.rs
git commit -m "feat(psyche): persist request adoption ledger"
```

### Task 4: Enforce adopted lifecycle and retention invariants

**Files:**
- Modify: `crates/coven-cli/src/store.rs:2095-2197,2478-2483`
- Modify: `crates/coven-cli/src/event_writer.rs:821-862`
- Modify: `crates/coven-cli/src/session_launch.rs:163-185`
- Modify: `crates/coven-cli/src/main.rs:4683-4710`
- Modify: `crates/coven-cli/src/tui/chat/client.rs:334-343`
- Modify: `crates/coven-cli/src/tui/chat/app.rs:1571-1584,7087-7114`
- Modify: `crates/coven-cli/src/tui/sessions.rs:150-165,365-382`
- Modify: `crates/coven-cli/src/tui/shell.rs:220-232,400-418`
- Modify: `crates/coven-cli/src/tui/cast/plan.rs:414-434`
- Modify: `crates/coven-cli/src/tui/cast/gate.rs:95-115`
- Test: the same Rust modules

- [ ] **Step 1: Write failing terminal-wins, stale-created, and retention tests**

Cover:

```text
terminal_exit_transitions_created_or_running
activation_compare_and_set_never_overwrites_terminal
stale_created_recovery_excludes_launch_adoptions_and_reservations
adopted_session_sacrifice_returns_typed_retention_error
foreign_key_blocks_concurrent_sacrifice_after_preflight
unadopted_non_running_session_remains_sacrificable
failed_sacrifice_keeps_the_session_in_the_overlay_list
sacrifice_help_qualifies_adoption_retention
cast_sacrifice_confirmation_does_not_promise_ineligible_deletion
```

- [ ] **Step 2: Run the focused tests and verify they fail**

```bash
cargo test -p coven-cli --bin coven terminal_exit_transitions
cargo test -p coven-cli --bin coven stale_created_recovery_excludes
cargo test -p coven-cli --bin coven adopted_session_sacrifice
cargo test -p coven-cli --bin coven failed_sacrifice
```

Expected: adopted `created` rows are incorrectly failed/deleted and exit cannot transition `created`.

- [ ] **Step 3: Add terminal-wins and stale-recovery helpers**

```rust
pub fn update_session_terminal_if_active(
    conn: &Connection,
    session_id: &str,
    status: &str,
    exit_code: Option<i32>,
    updated_at: &str,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE sessions
         SET status = ?2, exit_code = ?3, updated_at = ?4
         WHERE id = ?1 AND status IN ('created', 'running')",
        params![session_id, status, exit_code, updated_at],
    )?;
    Ok(affected > 0)
}
```

Use it from `event_writer::record_exit`. Change `mark_stale_created_sessions_failed` to add:

```sql
AND NOT EXISTS (
    SELECT 1 FROM request_adoptions
    WHERE request_adoptions.session_id = sessions.id
      AND request_adoptions.operation = 'launch'
)
```

The API activation path added in Task 5 must call
`update_session_status_if_current(&conn, &record.id, "created", "running", None, &current_timestamp())`;
a false result is success because terminal wins.

- [ ] **Step 4: Add the typed sacrifice denial**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdoptionRetentionError;

impl std::fmt::Display for AdoptionRetentionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session adoption evidence is retained; sacrifice is unavailable until an approved retention/fence contract resolves it"
        )
    }
}

impl std::error::Error for AdoptionRetentionError {}
```

Add `store::ensure_session_sacrificable` for surfaces that prompt before
deletion. It runs the same retained-adoption `EXISTS` query and returns the typed
error. In `store::sacrifice_session`, repeat that preflight
`SELECT EXISTS(SELECT 1 FROM request_adoptions WHERE session_id = ?1)`, return
this typed error, then perform the delete. If a concurrent insert wins after
preflight, translate SQLite foreign-key constraint failure to the same typed
error. Do not add deletion or pruning APIs.

- [ ] **Step 5: Preserve the typed denial across CLI, TUI, and chat client**

Keep `store::sacrifice_session` as the final authority. CLI calls
`ensure_session_sacrificable` before asking for `--yes`, then the delete repeats
the check; `DaemonChatClient` propagates the final typed denial unchanged. The
TUI continues rendering `Sacrifice failed: {error}` and must retain the session
row in its overlay. Change CLI, browser, magical-TUI, cast-plan, and cast-gate
copy from unconditional deletion to “eligible non-running session”; state that
adopted/reserved sessions are retained. Assert the canonical denial and
qualified help at every surface.

- [ ] **Step 6: Run and commit**

```bash
cargo test -p coven-cli --bin coven stale_created
cargo test -p coven-cli --bin coven sacrifice
cargo test -p coven-cli --bin coven terminal_exit
git add crates/coven-cli/src/store.rs crates/coven-cli/src/event_writer.rs \
  crates/coven-cli/src/session_launch.rs crates/coven-cli/src/main.rs \
  crates/coven-cli/src/tui/chat/client.rs crates/coven-cli/src/tui/chat/app.rs \
  crates/coven-cli/src/tui/sessions.rs crates/coven-cli/src/tui/shell.rs \
  crates/coven-cli/src/tui/cast/plan.rs crates/coven-cli/src/tui/cast/gate.rs
git commit -m "feat(psyche): retain adopted lifecycle evidence"
```

### Task 5: Add dedicated adopted launch with replay-first ordering

**Files:**
- Modify: `crates/coven-cli/src/api.rs:239-328,462-890,1911-2195`
- Test: `crates/coven-cli/src/api.rs`

- [ ] **Step 1: Write failing route, precedence, replay, and failure tests**

Use instrumented `SessionRuntime` implementations and real temporary stores to
cover every launch bullet in contract §8.3, including concurrent requests,
mutable drift after adoption, restart replay, committed-without-runtime crash
simulation, runtime failure, deterministic exit-before-launch-return, an
adopted-route request with no `requestAdoption`, a legacy bound launch, and
`requestAdoption` on an unbound launch.

- [ ] **Step 2: Run the launch tests and verify they fail**

```bash
cargo test -p coven-cli --bin coven adopted_launch
cargo test -p coven-cli --bin coven bound_legacy_launch_requires_adoption
```

Expected: the dedicated route is unknown and legacy bound launch still executes.

- [ ] **Step 3: Add structural parsing and redacted O3 errors**

Add:

```rust
fn request_adoption_error(error: crate::request_adoption::ValidationError) -> Result<ApiResponse> {
    let (code, path) = match error {
        crate::request_adoption::ValidationError::Unsupported { path } =>
            ("request_adoption_unsupported", path),
        crate::request_adoption::ValidationError::Missing { path }
        | crate::request_adoption::ValidationError::Invalid { path } =>
            ("request_adoption_invalid", path),
    };
    api_error(400, code, "Request adoption is invalid.", Some(json!({"fields": [path]})))
}

fn adoption_conflict(field: &'static str) -> Result<ApiResponse> {
    api_error(
        409,
        "request_adoption_conflict",
        "Request adoption conflicts with retained evidence.",
        Some(json!({"fields": [field]})),
    )
}

fn post_adoption_error(status: u16, code: &str, message: &str) -> Result<ApiResponse> {
    api_error(
        status,
        code,
        message,
        Some(json!({"adopted": true, "delivery": "not_asserted"})),
    )
}
```

No helper accepts or formats secret identity values.

- [ ] **Step 4: Split legacy and adopted route dispatch**

The public paths remain versioned, but the normalized route matcher uses:

```rust
("POST", "/sessions") =>
    launch_session(coven_home, body, runtime, authority),
("POST", "/adopted-sessions") =>
    launch_adopted_session(coven_home, body, runtime, authority),
```

The adopted handler returns `request_adoption_required` when the top-level
member is absent. The legacy handler rejects valid bound requests with
`request_adoption_required` and rejects `requestAdoption` on unbound/external
locations as `request_adoption_invalid`. Existing unbound requests retain their
current code path and response.

- [ ] **Step 5: Implement the exact adopted launch state machine**

`launch_adopted_session` performs:

```rust
// 1. Parse JSON, launch fields, O2 shape, O3 shape, relationship, and digest equality.
// 2. Resolve key + scope read-only; return Replay/Conflict before mutable admission.
// 3. Acquire AdoptionGate(key, five-field attempt scope); resolve again.
// 4. Run path/harness/familiar/expiry/parent validation and acquire maintenance.
// 5. BEGIN IMMEDIATE; resolve again; revalidate parent; insert `created` session
//    and launch adoption; commit.
// 6. Drop AdoptionGate immediately after commit, before invoking the runtime.
// 7. Invoke runtime with request metadata absent.
// 8. On runtime error, CAS created->failed and return a post-adoption error.
// 9. On runtime success, CAS created->running; a false CAS means terminal won.
// 10. Reload the SessionRecord and return its persisted current status with 201.
//     Exact replay returns the stored SessionRecord with 200.
```

On maintenance acquisition failure, resolve once more while still holding the
gate and return a replay winner before the maintenance error. Release any newly
acquired writer on transaction replay/conflict. Drop the adoption gate after
transaction commit so a concurrent replay can immediately observe truthful
`created`. Reload the row after activation so an exit that won the race is
returned as terminal rather than stale `created`. Emit Coven Calls only after
first-adoption runtime establishment; never on replay.

- [ ] **Step 6: Prove no SQLite writer spans mutable or maintenance work**

Add a test that blocks maintenance while another connection performs a harmless write, then commits an adoption winner and verifies the waiter returns replay rather than the stale maintenance error.

- [ ] **Step 7: Run and commit**

```bash
cargo test -p coven-cli --bin coven adopted_launch
cargo test -p coven-cli --bin coven execution_binding
git add crates/coven-cli/src/api.rs
git commit -m "feat(psyche): adopt bound session launches"
```

### Task 6: Add adopted input and internal event correlation

**Files:**
- Modify: `crates/coven-cli/src/api.rs:283-328,855-862,3139-3302,3862-3917`
- Modify: `crates/coven-cli/src/daemon.rs:709-812`
- Modify: `crates/coven-cli/src/event_writer.rs:80-150,243-310,700-728`
- Modify: `crates/coven-cli/src/store.rs:111-119,2899-3019,3267-3299`
- Test: those modules

- [ ] **Step 1: Write failing adopted-input and event-correlation tests**

Cover every contract §8.4 bullet, including exact replay after expiry/terminal
state, cross-operation key conflicts, pre-commit capacity/handoff failures,
post-commit runtime/event failures, no metadata leakage, raw SQL correlation
attacks, adopted input without `requestAdoption`, legacy bound input without
adoption, `requestAdoption` on an unbound input, and `requestAdoption` on kill.

- [ ] **Step 2: Run the focused tests and verify they fail**

```bash
cargo test -p coven-cli --bin coven adopted_input
cargo test -p coven-cli --bin coven request_adoption_event_correlation
```

Expected: the route is unknown and events cannot carry internal adoption correlation.

- [ ] **Step 3: Carry internal correlation outside payloads**

Extend `SessionRuntime::with_session_event_boundary` with:

```rust
request_adoption_id: Option<&str>,
```

Pass `None` at all existing kill/unbound call sites. Extend `EventWriter::reserve_record` and `PendingEvent::Record` to retain `Option<String>` beside `EventRecord`, not inside `payload_json` or the serialized `EventRecord`. Add:

```rust
pub fn insert_event_with_privacy_and_adoption(
    conn: &Connection,
    coven_home: &Path,
    record: &EventRecord,
    request_adoption_id: Option<&str>,
) -> Result<()>;
```

Keep `insert_event_with_privacy` as a compatibility wrapper passing `None`. Bind the internal value only to the SQL `request_adoption_id` column.

- [ ] **Step 4: Atomically acquire the input lease and adoption**

Add a store helper that begins one `IMMEDIATE` transaction, repeats `resolve_input_adoption`, checks committed handoff fencing, inserts `session_input_leases`, inserts the input adoption, and commits. Return an enum distinguishing `Adopted { adoption_id, lease_id }`, `Replay`, `Conflict`, and `HandoffFenced`. Existing unbound `acquire_session_input_lease` remains unchanged.

- [ ] **Step 5: Implement the dedicated input route and response**

Match normalized `/sessions/:id/adopted-input` before legacy `/input`. Follow this exact order:

```rust
// session lookup -> body/O2/O3/data parse -> exact O2 proof
// -> read-only adoption resolution -> AdoptionGate -> repeat resolution
// -> expiry/liveness/capacity -> atomic lease+adoption commit
// -> strip executionBinding/requestAdoption -> runtime/event boundary
// -> lease release -> 202 {adopted:true,replayed:false,delivery:"not_asserted"}
```

Missing adoption on the adopted route or on a legacy bound input returns
`request_adoption_required`. Adoption on an unbound input or any kill request
returns `request_adoption_invalid` with the documented static field path.
Replay returns:

```json
{"adopted":true,"replayed":true,"delivery":"not_asserted"}
```

Every synchronous runtime, persistence, or lease-release error after commit keeps its concrete code and adds only `{"adopted":true,"delivery":"not_asserted"}`. Replay never calls the runtime or event writer. Legacy bound input returns `request_adoption_required`; unbound input behavior remains unchanged.

Update `kill_session` to inspect a parsed body for the reserved
`requestAdoption` member before existing bound-proof/status processing and
return `request_adoption_invalid` naming `requestAdoption`. Kill otherwise
retains all O2 semantics and never acquires an adoption gate or writes the
ledger.

- [ ] **Step 6: Prove the event boundary remains metadata-free**

Assert the runtime payload, `can_record_session_event` payload, and persisted
`payload_json` equal `{"data":"hello"}` exactly. Assert
`request_adoption_id` exists only in the database column and public
`GET /events` output has no new field.

- [ ] **Step 7: Run and commit**

```bash
cargo test -p coven-cli --bin coven adopted_input
cargo test -p coven-cli --bin coven event_writer
cargo test -p coven-cli --bin coven session_handoff
git add crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs \
  crates/coven-cli/src/event_writer.rs crates/coven-cli/src/store.rs
git commit -m "feat(psyche): adopt bound session input"
```

### Task 7: Advertise and consume O3 capability without downgrade

**Files:**
- Modify: `crates/coven-cli/src/api.rs:118-152,365-395`
- Modify: `packages/openclaw-coven/src/client.ts:7-148,552-715,764-907`
- Modify: `packages/openclaw-coven/src/client.test.ts`
- Modify: `packages/openclaw-coven/src/compat.test.ts`
- Modify: `packages/openclaw-coven/src/runtime.test.ts:90-112`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/health-available.json`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/health-daemon-null.json`
- Test: Rust API tests and OpenClaw Vitest suites

- [ ] **Step 1: Write failing health and client negotiation tests**

Test additive camelCase health serialization, snake_case normalization, exact O3 object validation, exact dedicated paths, first/replay response normalization, and zero POST calls when health fails or capability is absent/non-array/unsupported.

- [ ] **Step 2: Run the tests and verify they fail**

```bash
cargo test -p coven-cli --bin coven health_request_adoption_contracts
(cd packages/openclaw-coven && pnpm test -- client.test.ts compat.test.ts runtime.test.ts)
```

Expected: health omits the field and adopted client methods do not exist.

- [ ] **Step 3: Add additive Rust health advertisement**

Add a defaulted `request_adoption_contracts: Vec<String>` field and populate it with `crate::request_adoption::CONTRACT`. Preserve every existing field.

- [ ] **Step 4: Add exact TypeScript types and validation**

```typescript
export const PSYCHE_REQUEST_ADOPTION_V1 = "psyche.request_adoption.v1" as const;

export type CovenRequestAdoption = {
  contract: typeof PSYCHE_REQUEST_ADOPTION_V1;
  key: string;
  requestDigest: string;
};

export type CovenAdoptionResult = {
  adopted: true;
  replayed: boolean;
  delivery: "not_asserted";
};
```

Add `requestAdoptionContracts?: unknown` to health capabilities. Implement `normalizeRequestAdoption` with exact keys, ASCII key validation, and lowercase digest validation, using one snapshot of caller input before serialization.

- [ ] **Step 5: Add explicit adopted methods with mandatory health negotiation**

Add:

```typescript
launchAdoptedSession(
  input: LaunchCovenSessionInput & {
    executionBinding: CovenExecutionBinding;
    requestAdoption: CovenRequestAdoption;
  },
  signal?: AbortSignal,
): Promise<CovenSessionRecord>;

sendAdoptedInput(
  sessionId: string,
  data: string,
  executionBinding: CovenExecutionBinding,
  requestAdoption: CovenRequestAdoption,
  signal?: AbortSignal,
): Promise<CovenAdoptionResult>;
```

Each method first awaits `health(signal)`, requires an array containing the exact O3 literal, then sends only `/api/v1/adopted-sessions` or `/api/v1/sessions/:id/adopted-input`. Health errors and malformed/unsupported capability values throw locally before POST. Retain old bound methods as explicitly legacy methods; do not silently call them from adopted methods.

Extend the typed `fakeClient` in `runtime.test.ts` with:

```typescript
launchAdoptedSession: vi.fn(async () => session()),
sendAdoptedInput: vi.fn(async () => ({
  adopted: true,
  replayed: false,
  delivery: "not_asserted" as const,
})),
```

- [ ] **Step 6: Add replacement/downgrade compatibility coverage**

In `compat.test.ts`, use one stateful socket server whose first request returns
an O3 health response and then atomically switches its route table to pre-O3
behavior before reading the next request. Invoke an adopted method and assert
its subsequent dedicated-route POST receives unknown-route failure without any
legacy mutation. Assert exact request bodies contain only supported launch/input
fields plus `executionBinding` and `requestAdoption`.

- [ ] **Step 7: Run and commit**

```bash
cargo test -p coven-cli --bin coven health_request_adoption_contracts
(cd packages/openclaw-coven && pnpm test -- client.test.ts compat.test.ts runtime.test.ts)
(cd packages/openclaw-coven && pnpm typecheck)
git add crates/coven-cli/src/api.rs packages/openclaw-coven/src
git commit -m "feat(psyche): negotiate request adoption"
```

### Task 8: Publish O3 API, lifecycle, and retention documentation

**Files:**
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/reference/api-contract.md`
- Modify: `docs/daemon/socket-api.md`
- Modify: `docs/SESSION-LIFECYCLE.md`
- Modify: `docs/sessions/lifecycle.md`
- Modify: `docs/reference/cli-sacrifice.md`
- Modify: `docs/reference/cli-sessions.md`
- Modify: `docs/rituals/sacrifice.md`
- Modify: `docs/rituals/index.md`
- Modify: `docs/GLOSSARY.md`
- Modify: `docs/reference/glossary.md`
- Modify: `docs/help/session-stuck.md`
- Modify: `docs/start/coven-tui.md`
- Modify: `docs/design/cast-phase6-inspection.md`
- Modify: `docs/guides/session-operations.md`
- Modify: `docs/reference/cli-archive.md`
- Modify: `docs/reference/cli-kill.md`
- Modify: `docs/harnesses/provider-auth.md`
- Modify: `README.md`
- Modify: `scripts/check-api-contract-docs.py:117-155`
- Modify: `scripts/check-api-contract-docs-test.py:85-128`
- Test: `scripts/check-api-contract-docs.py`
- Test: `scripts/check-api-contract-docs-test.py`

- [ ] **Step 1: Extend the documentation checker first**

Add `"executionBindingContracts"` and `"requestAdoptionContracts"` to `HEALTH_CAPABILITY_FIELDS`, then add O3 route/error literals to the required API contract assertions:

```python
O3_REQUIRED_LITERALS = (
    "/api/v1/adopted-sessions",
    "/api/v1/sessions/:id/adopted-input",
    "psyche.request_adoption.v1",
    "requestAdoptionContracts",
    "request_adoption_required",
    "request_adoption_invalid",
    "request_adoption_unsupported",
    "request_adoption_conflict",
)
```

Require each literal in `docs/API-CONTRACT.md`. Because
`HEALTH_CAPABILITY_FIELDS` applies to synchronized health tables, update
`docs/reference/api.md`, `docs/reference/api-contract.md`, and
`docs/daemon/socket-api.md` with both `executionBindingContracts` and
`requestAdoptionContracts`. Update the checker test's canonical count from
`all 14` to `all 16`, keep its stale-count mutation test, and add a fixture
mutation proving removal of `requestAdoptionContracts` is detected.

- [ ] **Step 2: Run the checker and verify it fails**

```bash
python3 scripts/check-api-contract-docs.py
python3 scripts/check-api-contract-docs-test.py
```

Expected: the contract checker and checker unit tests fail because the public
documents still expose the pre-O3 capability set. This is the red phase; rerun
both commands after Steps 3-4 update the documents.

- [ ] **Step 3: Document the exact API contract**

Add closed request shape, dedicated routes, first/replay statuses, input
response, global key and launch scope rules, error table and static field paths,
replay-before-mutable precedence, capability negotiation, metadata stripping,
and O4-O8 exclusions. State that adoption is durable responsibility, not
delivery. Add the dedicated routes and additive capability to each synchronized
API/socket reference without duplicating the normative prose.

- [ ] **Step 4: Correct lifecycle and sacrifice wording**

Document adopted daemon launch as `created -> running` only after runtime
ownership, terminal-wins activation, and stale-reaper exclusion. Replace every
unconditional “any non-running row” or “permanently delete a non-running
session” claim across both lifecycle pages, both glossaries, sacrifice
reference/ritual pages, sessions reference, stuck-session guidance, TUI help,
the cast inspection design, session-operations guide, archive/kill references,
provider-auth cleanup advice, and README ritual matrix. State that unadopted
non-running sessions remain deletable while adopted/reserved sessions return
the canonical `AdoptionRetentionError`.

- [ ] **Step 5: Run and commit**

```bash
python3 scripts/check-api-contract-docs.py
python3 scripts/check-api-contract-docs-test.py
git diff --check
git add docs/API-CONTRACT.md docs/reference/api.md \
  docs/reference/api-contract.md docs/daemon/socket-api.md \
  docs/SESSION-LIFECYCLE.md docs/sessions/lifecycle.md \
  docs/reference/cli-sacrifice.md docs/reference/cli-sessions.md \
  docs/rituals/sacrifice.md docs/rituals/index.md \
  docs/GLOSSARY.md docs/reference/glossary.md \
  docs/help/session-stuck.md docs/start/coven-tui.md \
  docs/design/cast-phase6-inspection.md \
  docs/guides/session-operations.md docs/reference/cli-archive.md \
  docs/reference/cli-kill.md docs/harnesses/provider-auth.md README.md \
  scripts/check-api-contract-docs.py scripts/check-api-contract-docs-test.py
git commit -m "docs(psyche): publish O3 request adoption"
```

### Task 9: Run full O3 gates and record completion evidence

**Files:**
- Modify in a post-merge evidence branch: `specs/psyche/O3_CONTRACT_DESIGN.md:1-15,784-799`
- Modify in the same evidence branch: `specs/psyche/PLAN.md:19-26`

- [ ] **Step 1: Run all focused O3 tests together**

```bash
cargo test -p coven-cli --bin coven request_adoption
cargo test -p coven-cli --bin coven adopted_launch
cargo test -p coven-cli --bin coven adopted_input
cargo test -p coven-cli --bin coven sacrifice
cargo test -p coven-cli --bin coven stale_created
cargo test -p coven-cli --bin coven event_writer
(cd packages/openclaw-coven && pnpm test -- client.test.ts compat.test.ts runtime.test.ts)
(cd packages/openclaw-coven && pnpm typecheck)
python3 scripts/check-api-contract-docs.py
python3 scripts/check-api-contract-docs-test.py
```

Expected: every focused command passes.

- [ ] **Step 2: Run repository gates**

```bash
cargo test -p coven-cli --bin coven
cargo test -p coven-cli --test smoke
cargo clippy -p coven-cli --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: all commands pass with no warnings or whitespace errors.

- [ ] **Step 3: Perform fault-injection and contract review**

Map every bullet in `specs/psyche/O3_CONTRACT_DESIGN.md` §8 to a named passing test. Verify committed-without-runtime, exit-before-activation, maintenance waiter, concurrent insert, concurrent sacrifice, restart replay, and client no-POST cases are present. Search:

```bash
rg -n 'lookup|proven.not.adopted|redispatch|retention expiry|cancellation acknowledgement|artifact binding' \
  crates/coven-cli packages/openclaw-coven docs/API-CONTRACT.md
```

Expected: no code claims O4-O8 behavior; documentation mentions appear only as exclusions.

- [ ] **Step 4: Open, verify, and merge the implementation PR before claiming completion**

Push the implementation branch and open a PR whose body links issue #741, the
approved design, this plan, every validation command, and all O4-O8 exclusions.
Wait for required CI, require a blocking code review, and merge only when both
are green. Capture the immutable receipts:

```bash
implementation_pr=$(gh pr view --json number --jq .number)
gh pr checks "$implementation_pr" --watch
gh pr merge "$implementation_pr" --squash --delete-branch
merge_sha=$(gh pr view "$implementation_pr" --json mergeCommit --jq .mergeCommit.oid)
merge_url=$(gh pr view "$implementation_pr" --json url --jq .url)
```

- [ ] **Step 5: Create and merge a post-implementation evidence update**

Fetch the merged implementation, create `docs/psyche-o3-implementation-evidence`
from `origin/main`, and change O3 status to implemented using the actual
implementation PR URL, `merge_sha`, CI run URL, focused test counts, and
explicit O4-O8 exclusions. Update `PLAN.md` from “O3 approved” to “O3 complete /
O4 pending.” Then commit and open the evidence PR:

```bash
git fetch origin main
git switch -c docs/psyche-o3-implementation-evidence origin/main
git add specs/psyche/O3_CONTRACT_DESIGN.md specs/psyche/PLAN.md
git commit -m "docs(psyche): record O3 implementation evidence"
git push -u origin docs/psyche-o3-implementation-evidence
gh pr create --base main --head docs/psyche-o3-implementation-evidence \
  --title "docs(psyche): record O3 implementation evidence" \
  --body "Records the verified O3 implementation merge and CI receipts. Refs #741."
```

- [ ] **Step 6: Verify the evidence PR and close the tracker**

Run the documentation checker on the evidence branch, merge its green PR, then
append both PR URLs, both merge SHAs, the implementation CI URL, focused test
counts, and final contract path to issue #741 and Bead `coven-psy-o3`. Use:

```bash
bd update coven-psy-o3 --append-notes \
  "O3 implementation and evidence merged; receipts are recorded on GitHub issue #741."
bd close coven-psy-o3 \
  --reason "O3 implementation, green CI, and evidence merge are recorded on issue #741."
bd show coven-psy-o3
```

Close #741 only after the evidence commit is visible on `main` and `bd show`
reports the Bead closed with the same receipt scope.

## Plan self-review checklist

- [x] Every O3 contract §8 bullet maps to a named test in Tasks 1-7.
- [x] Replay/conflict resolution occurs before mutable admission and repeats after gate acquisition and inside the transaction.
- [x] Launch adoption and session creation share one transaction; input lease and adoption share one transaction.
- [x] Runtime/event side effects occur only after adoption commit.
- [x] `request_adoption_id` is internal SQL correlation, never public JSON or runtime payload.
- [x] Historical reservations are deterministic, restart-idempotent, and fail closed on duplicate scopes.
- [x] Adopted/reserved `created` rows survive generic stale recovery.
- [x] Terminal exit can beat `created -> running` without being overwritten.
- [x] Dedicated client routes and mandatory health negotiation prevent downgrade.
- [x] Sacrifice retention is typed, race-safe, and consistent across CLI/TUI/chat.
- [x] O4-O8 remain explicit non-goals.
