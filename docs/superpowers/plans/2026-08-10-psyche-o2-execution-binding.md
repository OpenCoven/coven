# Psyche O2 Execution Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved `psyche.execution_binding.v1` contract so Coven atomically stores an immutable Psyche binding, correlates bound launches, exact-matches proofs on input and kill, and exposes additive Rust and TypeScript wire support without adding O3 adoption semantics.

**Architecture:** Put the closed wire type and syntax/comparison rules in a focused Rust module, persist that typed value as deterministic JSON on the session row, and keep route-specific precedence/error mapping in `api.rs`. Bound proof metadata is consumed at the API boundary and stripped before runtime/event delivery. The OpenClaw client gains additive typed support and validation, but its existing unbound runtime behavior remains compatible.

**Tech Stack:** Rust 2021, Serde/serde_json, Chrono, Rusqlite/SQLite, TypeScript, Vitest, Markdown.

---

## Preconditions and scope

- Execute from a fresh worktree based on current `origin/main`, after an implementation issue exists and its issue-keyed Coven claim is available.
- Do not modify the existing unrelated `.gitignore`, `.review-task5.diff`, or `specs/coven-session-permissions/` changes visible in the primary checkout.
- The normative source is `specs/psyche/O2_CONTRACT_DESIGN.md`.
- O2 permits two sessions with identical bindings. Do not add an adoption key, uniqueness index, lookup-by-binding route, replay detection, fencing, cancellation acknowledgement, artifact binding, or production child dispatch.
- Keep `SessionRuntime::send_input` and `SessionRuntime::kill_session` signatures unchanged. Execution-binding proof must not cross that boundary.

## File map

| File | Responsibility |
| --- | --- |
| `crates/coven-cli/src/execution_binding.rs` | Closed wire types, exact member checks, syntax/expiry validation, deterministic comparison, and mismatch paths. |
| `crates/coven-cli/src/main.rs` | Register the new Rust module. |
| `crates/coven-cli/src/store.rs` | Add the nullable SQLite column, typed session field, deterministic serialization, strict readback, and migration tests. |
| `crates/coven-cli/src/session_launch.rs` | Thread the optional binding through the single fresh-session constructor. |
| `crates/coven-cli/src/api.rs` | Enforce launch correlation, operation precedence, redacted errors, external rejection, capability advertisement, and proof stripping. |
| `crates/coven-cli/src/daemon.rs` | Add `execution_binding: None` to existing session fixtures/builders. |
| `crates/coven-cli/src/event_writer.rs` | Add `execution_binding: None` to existing session fixtures/builders. |
| `crates/coven-cli/src/observe.rs` | Add `execution_binding: None` to existing session fixtures/builders. |
| `crates/coven-cli/src/tui/shell.rs` | Add `execution_binding: None` to existing session fixtures/builders. |
| `crates/coven-cli/src/tui/chat/app.rs` | Add `execution_binding: None` to existing session fixtures/builders. |
| `crates/coven-cli/src/tui/chat/render.rs` | Add `execution_binding: None` to existing session fixtures/builders. |
| `packages/openclaw-coven/src/client.ts` | Add typed binding support, exact client-side validation, response normalization, and additive bound mutation methods. |
| `packages/openclaw-coven/src/client.test.ts` | Test binding validation, health normalization, and response normalization. |
| `packages/openclaw-coven/src/compat.test.ts` | Test exact request bodies for bound launch/input/kill and fixture compatibility. |
| `packages/openclaw-coven/src/fixtures/v2026.4/health-available.json` | Advertise the supported execution-binding contract. |
| `packages/openclaw-coven/src/fixtures/v2026.4/health-daemon-null.json` | Advertise the supported execution-binding contract without daemon metadata. |
| `packages/openclaw-coven/src/fixtures/v2026.4/session-running.json` | Include `execution_binding: null` for an unbound session. |
| `packages/openclaw-coven/src/fixtures/v2026.4/session-completed.json` | Include `execution_binding: null` for an unbound session. |
| `packages/openclaw-coven/src/fixtures/v2026.4/sessions-list.json` | Include `execution_binding: null` on legacy-compatible list entries. |
| `docs/API-CONTRACT.md` | Publish the O2 wire contract, precedence, errors, capability, and non-goals. |
| `specs/psyche/RUNTIME_DESIGN.md` | Correct the composite execution-binding description so O3-O7 remain separate. |
| `specs/psyche/O2_CONTRACT_DESIGN.md` | Record implementation status only after every O2 acceptance gate passes. |

### Task 1: Implement the closed execution-binding value object

**Files:**
- Create: `crates/coven-cli/src/execution_binding.rs`
- Modify: `crates/coven-cli/src/main.rs:15-73`
- Test: `crates/coven-cli/src/execution_binding.rs`

- [x] **Step 1: Register an empty module and write the first failing parser test**

Add to `main.rs` with the other module declarations:

```rust
mod execution_binding;
```

Create `execution_binding.rs` with the contract constant and a test that fixes the exact root shape:

```rust
pub const CONTRACT: &str = "psyche.execution_binding.v1";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn root_value() -> serde_json::Value {
        json!({
            "contract": CONTRACT,
            "principalRef": "principal:operator",
            "familiarId": "sage",
            "familiarSnapshotDigest": digest('a'),
            "projectDigest": digest('b'),
            "graphId": "graph-1",
            "nodeId": "node-1",
            "attemptId": "attempt-1",
            "requestDigest": digest('c'),
            "policyRevision": "policy:7",
            "expiresAt": "2099-01-01T00:00:00Z",
            "parent": null,
            "delegationDigest": null
        })
    }

    #[test]
    fn parses_the_exact_root_shape() {
        let binding = parse(&root_value()).expect("root binding should parse");
        assert_eq!(binding.contract, CONTRACT);
        assert_eq!(binding.familiar_id, "sage");
        assert_eq!(binding.parent, None);
        assert_eq!(binding.delegation_digest, None);
    }
}
```

- [x] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo test -p coven-cli execution_binding::tests::parses_the_exact_root_shape
```

Expected: compilation fails because `parse` and `ExecutionBinding` do not exist.

- [x] **Step 3: Add the exact typed shape and closed-object parser**

Implement the complete types and parser. Nullable fields remain required members: the explicit key-set check occurs before Serde so omitted `parent` or `delegationDigest` cannot collapse into `None`.

```rust
use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTRACT: &str = "psyche.execution_binding.v1";

const BINDING_FIELDS: [&str; 13] = [
    "attemptId",
    "contract",
    "delegationDigest",
    "expiresAt",
    "familiarId",
    "familiarSnapshotDigest",
    "graphId",
    "nodeId",
    "parent",
    "policyRevision",
    "principalRef",
    "projectDigest",
    "requestDigest",
];

const PARENT_FIELDS: [&str; 4] = ["attemptId", "graphId", "nodeId", "sessionId"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBindingParent {
    pub session_id: String,
    pub graph_id: String,
    pub node_id: String,
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBinding {
    pub contract: String,
    pub principal_ref: String,
    pub familiar_id: String,
    pub familiar_snapshot_digest: String,
    pub project_digest: String,
    pub graph_id: String,
    pub node_id: String,
    pub attempt_id: String,
    pub request_digest: String,
    pub policy_revision: String,
    pub expires_at: String,
    pub parent: Option<ExecutionBindingParent>,
    pub delegation_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Missing { path: &'static str },
    Invalid { path: &'static str },
    Unsupported { path: &'static str },
    Expired { path: &'static str },
}

fn require_exact_fields(
    value: &Value,
    expected: &[&str],
    path: &'static str,
) -> Result<(), ValidationError> {
    let object = value
        .as_object()
        .ok_or(ValidationError::Invalid { path })?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual.difference(&expected).next().is_some() {
        return Err(ValidationError::Invalid { path });
    }
    if expected.difference(&actual).next().is_some() {
        return Err(ValidationError::Missing { path });
    }
    Ok(())
}

pub fn parse(value: &Value) -> Result<ExecutionBinding, ValidationError> {
    require_exact_fields(value, &BINDING_FIELDS, "executionBinding")?;
    if let Some(parent) = value.get("parent").filter(|parent| !parent.is_null()) {
        require_exact_fields(parent, &PARENT_FIELDS, "executionBinding.parent")?;
    }
    let binding: ExecutionBinding = serde_json::from_value(value.clone())
        .map_err(|_| ValidationError::Invalid { path: "executionBinding" })?;
    binding.validate_shape()?;
    Ok(binding)
}
```

- [x] **Step 4: Add exact syntax, version, timestamp, and expiry validation**

Add these methods/helpers. The timestamp check round-trips through Chrono to reject fractional seconds and non-`Z` offsets. Expiry is elapsed when it is less than or equal to the comparison instant.

```rust
fn valid_opaque(value: &str) -> bool {
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

fn parse_expiry(value: &str) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(value).ok()?.with_timezone(&Utc);
    (parsed.to_rfc3339_opts(SecondsFormat::Secs, true) == value).then_some(parsed)
}

impl ExecutionBinding {
    pub fn validate_shape(&self) -> Result<(), ValidationError> {
        if self.contract != CONTRACT {
            return Err(ValidationError::Unsupported {
                path: "executionBinding.contract",
            });
        }
        for (path, value) in [
            ("executionBinding.principalRef", self.principal_ref.as_str()),
            ("executionBinding.familiarId", self.familiar_id.as_str()),
            ("executionBinding.graphId", self.graph_id.as_str()),
            ("executionBinding.nodeId", self.node_id.as_str()),
            ("executionBinding.attemptId", self.attempt_id.as_str()),
            ("executionBinding.policyRevision", self.policy_revision.as_str()),
        ] {
            if !valid_opaque(value) {
                return Err(ValidationError::Invalid { path });
            }
        }
        for (path, value) in [
            (
                "executionBinding.familiarSnapshotDigest",
                self.familiar_snapshot_digest.as_str(),
            ),
            ("executionBinding.projectDigest", self.project_digest.as_str()),
            ("executionBinding.requestDigest", self.request_digest.as_str()),
        ] {
            if !valid_digest(value) {
                return Err(ValidationError::Invalid { path });
            }
        }
        if parse_expiry(&self.expires_at).is_none() {
            return Err(ValidationError::Invalid {
                path: "executionBinding.expiresAt",
            });
        }
        if let Some(parent) = &self.parent {
            for (path, value) in [
                ("executionBinding.parent.sessionId", parent.session_id.as_str()),
                ("executionBinding.parent.graphId", parent.graph_id.as_str()),
                ("executionBinding.parent.nodeId", parent.node_id.as_str()),
                ("executionBinding.parent.attemptId", parent.attempt_id.as_str()),
            ] {
                if !valid_opaque(value) {
                    return Err(ValidationError::Invalid { path });
                }
            }
        }
        if self
            .delegation_digest
            .as_deref()
            .is_some_and(|value| !valid_digest(value))
        {
            return Err(ValidationError::Invalid {
                path: "executionBinding.delegationDigest",
            });
        }
        Ok(())
    }

    pub fn validate_not_expired(&self, now: DateTime<Utc>) -> Result<(), ValidationError> {
        let expires_at = parse_expiry(&self.expires_at).ok_or(ValidationError::Invalid {
            path: "executionBinding.expiresAt",
        })?;
        if expires_at <= now {
            return Err(ValidationError::Expired {
                path: "executionBinding.expiresAt",
            });
        }
        Ok(())
    }

    pub fn first_mismatch_path(&self, supplied: &Self) -> Option<&'static str> {
        let top_level = [
            (self.contract != supplied.contract, "executionBinding.contract"),
            (
                self.principal_ref != supplied.principal_ref,
                "executionBinding.principalRef",
            ),
            (
                self.familiar_id != supplied.familiar_id,
                "executionBinding.familiarId",
            ),
            (
                self.familiar_snapshot_digest != supplied.familiar_snapshot_digest,
                "executionBinding.familiarSnapshotDigest",
            ),
            (
                self.project_digest != supplied.project_digest,
                "executionBinding.projectDigest",
            ),
            (self.graph_id != supplied.graph_id, "executionBinding.graphId"),
            (self.node_id != supplied.node_id, "executionBinding.nodeId"),
            (
                self.attempt_id != supplied.attempt_id,
                "executionBinding.attemptId",
            ),
            (
                self.request_digest != supplied.request_digest,
                "executionBinding.requestDigest",
            ),
            (
                self.policy_revision != supplied.policy_revision,
                "executionBinding.policyRevision",
            ),
            (
                self.expires_at != supplied.expires_at,
                "executionBinding.expiresAt",
            ),
        ]
        .into_iter()
        .find_map(|(different, path)| different.then_some(path));
        if top_level.is_some() {
            return top_level;
        }

        match (&self.parent, &supplied.parent) {
            (None, None) => {}
            (Some(expected), Some(actual)) => {
                for (different, path) in [
                    (
                        expected.session_id != actual.session_id,
                        "parent.sessionId",
                    ),
                    (
                        expected.graph_id != actual.graph_id,
                        "parent.graphId",
                    ),
                    (
                        expected.node_id != actual.node_id,
                        "parent.nodeId",
                    ),
                    (
                        expected.attempt_id != actual.attempt_id,
                        "parent.attemptId",
                    ),
                ] {
                    if different {
                        return Some(path);
                    }
                }
            }
            _ => return Some("parent"),
        }

        (self.delegation_digest != supplied.delegation_digest)
            .then_some("executionBinding.delegationDigest")
    }
}
```

- [x] **Step 5: Add table-driven negative and child tests**

Add tests that mutate one field at a time and assert the exact `ValidationError` path:

```rust
#[test]
fn rejects_unknown_and_missing_members_at_both_levels() {
    let mut extra = root_value();
    extra["extra"] = json!(true);
    assert_eq!(
        parse(&extra),
        Err(ValidationError::Invalid {
            path: "executionBinding"
        })
    );

    let mut missing = root_value();
    missing.as_object_mut().unwrap().remove("parent");
    assert_eq!(
        parse(&missing),
        Err(ValidationError::Missing {
            path: "executionBinding"
        })
    );

    let mut child = root_value();
    child["parent"] = json!({
        "sessionId": "parent-session",
        "graphId": "graph-1",
        "nodeId": "node-parent",
        "attemptId": "attempt-parent",
        "extra": true
    });
    child["delegationDigest"] = json!(digest('d'));
    assert_eq!(
        parse(&child),
        Err(ValidationError::Invalid {
            path: "executionBinding.parent"
        })
    );
}

#[test]
fn rejects_noncanonical_values_without_normalizing_them() {
    for (field, invalid) in [
        ("principalRef", " leading".to_string()),
        ("graphId", String::new()),
        ("nodeId", "snowman-\u{2603}".to_string()),
        ("policyRevision", "x".repeat(256)),
    ] {
        let mut value = root_value();
        value[field] = json!(invalid);
        assert!(matches!(parse(&value), Err(ValidationError::Invalid { .. })));
    }

    for (field, invalid) in [
        ("familiarSnapshotDigest", format!("sha256:{}", "A".repeat(64))),
        ("projectDigest", "sha256:1234".to_string()),
        ("requestDigest", format!("sha512:{}", "a".repeat(64))),
    ] {
        let mut value = root_value();
        value[field] = json!(invalid);
        assert!(matches!(parse(&value), Err(ValidationError::Invalid { .. })));
    }

    for invalid in [
        "2099-01-01T00:00:00.000Z",
        "2099-01-01T00:00:00+00:00",
        "2099-01-01 00:00:00Z",
    ] {
        let mut value = root_value();
        value["expiresAt"] = json!(invalid);
        assert!(matches!(parse(&value), Err(ValidationError::Invalid { .. })));
    }
}
```

Also add:

- a valid complete child object test;
- unknown contract -> `Unsupported`;
- `expiresAt == now` and past -> `Expired`;
- future expiry -> success;
- mixed-case valid IDs round-trip byte-exact;
- every top-level field and each `parent` subfield reports a mismatch path;
- serialized field order is stable by comparing `serde_json::to_string` to one literal JSON string.

- [x] **Step 6: Run the focused module tests**

Run:

```bash
cargo test -p coven-cli execution_binding::tests::
```

Expected: all execution-binding unit tests pass.

- [x] **Step 7: Commit the value object**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/execution_binding.rs
git commit -m "feat(api): define Psyche execution binding"
```

### Task 2: Persist the immutable binding on session rows

**Files:**
- Modify: `crates/coven-cli/src/store.rs:58-99,522-540,802-812,940-978,1991-2055,2165-2393`
- Modify: `crates/coven-cli/src/session_launch.rs:163-205`
- Modify: every existing `SessionRecord { ... }` literal in:
  - `crates/coven-cli/src/api.rs`
  - `crates/coven-cli/src/daemon.rs`
  - `crates/coven-cli/src/event_writer.rs`
  - `crates/coven-cli/src/main.rs`
  - `crates/coven-cli/src/observe.rs`
  - `crates/coven-cli/src/store.rs`
  - `crates/coven-cli/src/tui/chat/app.rs`
  - `crates/coven-cli/src/tui/chat/render.rs`
  - `crates/coven-cli/src/tui/shell.rs`
- Test: `crates/coven-cli/src/store.rs`
- Test: `crates/coven-cli/src/session_launch.rs`

- [x] **Step 1: Write failing migration and round-trip tests**

Add store tests beside the existing `familiar_id` migration tests:

```rust
#[test]
fn execution_binding_column_defaults_to_null_and_migrates_legacy_rows() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("legacy.sqlite3");
    {
        let legacy = Connection::open(&path)?;
        legacy.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY NOT NULL,
                project_root TEXT NOT NULL,
                harness TEXT NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
            VALUES ('legacy-1', '/tmp', 'codex', 'old', 'completed', '2026-01-01', '2026-01-01');",
        )?;
    }

    let conn = open_store(&path)?;
    assert!(table_columns(&conn, "sessions")?
        .iter()
        .any(|column| column == "execution_binding_json"));
    let binding: Option<String> = conn.query_row(
        "SELECT execution_binding_json FROM sessions WHERE id='legacy-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(binding, None);
    assert_eq!(
        get_session(&conn, "legacy-1")?.unwrap().execution_binding,
        None
    );
    Ok(())
}
```

Add a `binding()` fixture that calls `crate::execution_binding::parse`, then test insert/get/list/reopen all return the same typed value and raw JSON:

```rust
#[test]
fn execution_binding_round_trips_deterministically_across_reopen() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("test.sqlite3");
    let mut session = session_record("bound", "2026-08-10T00:00:00Z");
    session.execution_binding = Some(binding());
    {
        let conn = open_store(&path)?;
        insert_session(&conn, &session)?;
        assert_eq!(get_session(&conn, "bound")?, Some(session.clone()));
    }
    let reopened = open_store(&path)?;
    assert_eq!(get_session(&reopened, "bound")?, Some(session));
    Ok(())
}
```

- [x] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p coven-cli store::tests::execution_binding_
```

Expected: compilation fails because `SessionRecord.execution_binding` and the SQLite column do not exist.

- [x] **Step 3: Add the typed session field and migration**

Add to `SessionRecord` after `familiar_id`:

```rust
#[serde(default)]
pub execution_binding: Option<crate::execution_binding::ExecutionBinding>,
```

Add `execution_binding_json TEXT` to the fresh `sessions` schema after `familiar_id`, then add:

```rust
fn ensure_execution_binding_column(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "sessions",
        "execution_binding_json",
        "ALTER TABLE sessions ADD COLUMN execution_binding_json TEXT",
    )
}
```

Call `ensure_execution_binding_column(conn)?` immediately after `ensure_familiar_id_column(conn)?`.

- [x] **Step 4: Serialize atomically during both insert paths**

In `insert_session` and `insert_session_if_absent`, serialize only a present typed value:

```rust
let execution_binding_json = record
    .execution_binding
    .as_ref()
    .map(serde_json::to_string)
    .transpose()
    .context("failed to serialize session execution binding")?;
```

Append `execution_binding_json` to both column lists and parameter lists. Do not add an update function for this column.

- [x] **Step 5: Parse non-null stored values strictly**

Append `execution_binding_json` to `SESSION_COLUMNS`. In `session_record_from_row`, convert non-null text through Serde and `validate_shape`; map either failure to `rusqlite::Error::FromSqlConversionFailure`. Expiry is not rechecked on read because historical bindings remain readable after expiration.

```rust
fn execution_binding_from_sql(
    index: usize,
    value: Option<String>,
) -> rusqlite::Result<Option<crate::execution_binding::ExecutionBinding>> {
    value
        .map(|json| {
            let value: serde_json::Value = serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            crate::execution_binding::parse(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(anyhow::anyhow!("{error:?}")),
                )
            })
        })
        .transpose()
}
```

Populate `SessionRecord.execution_binding` from the appended column. A malformed non-null row must make `get_session` and list operations return a store error; it must never deserialize as unbound.

- [x] **Step 6: Thread the field through fresh-session construction**

Add to `NewSessionParams`:

```rust
pub execution_binding: Option<crate::execution_binding::ExecutionBinding>,
```

Set the corresponding field in `new_session_record`:

```rust
execution_binding: params.execution_binding,
```

Update its invariant test to prove both `Some(binding)` and `None` are preserved.

For every other `SessionRecord` literal reported by:

```bash
rg -n 'SessionRecord \{' crates/coven-cli/src
```

add:

```rust
execution_binding: None,
```

Do not change unrelated fixture values.

- [x] **Step 7: Complete persistence negative tests**

Add tests proving:

- raw invalid JSON in `execution_binding_json` causes `get_session` to error;
- a valid JSON object with an unsupported `contract` causes `get_session` to error;
- a valid JSON object with an invalid digest causes `get_session` to error;
- unbound `SessionRecord` serializes `"execution_binding":null`;
- bound `SessionRecord` serializes the complete typed object;
- two distinct session rows with byte-identical bindings both insert successfully;
- no SQL update helper exists and normal status/archive updates leave binding bytes unchanged.

- [x] **Step 8: Run focused persistence and constructor tests**

Run:

```bash
cargo test -p coven-cli store::tests::execution_binding_
cargo test -p coven-cli session_launch::tests::new_session_record_sets_launch_invariants
```

Expected: all focused tests pass.

- [x] **Step 9: Commit session persistence**

```bash
git add crates/coven-cli/src/store.rs crates/coven-cli/src/session_launch.rs \
  crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs \
  crates/coven-cli/src/event_writer.rs crates/coven-cli/src/main.rs \
  crates/coven-cli/src/observe.rs crates/coven-cli/src/tui/shell.rs \
  crates/coven-cli/src/tui/chat/app.rs crates/coven-cli/src/tui/chat/render.rs
git commit -m "feat(store): persist immutable execution bindings"
```

### Task 3: Enforce bound launch correlation and exact error precedence

**Files:**
- Modify: `crates/coven-cli/src/api.rs:230-262,1855-2025,2407-2474`
- Test: `crates/coven-cli/src/api.rs` test module near existing launch tests

- [x] **Step 1: Add reusable launch fixtures and the first failing root test**

In the API test module add:

```rust
fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn root_binding(familiar_id: &str) -> Value {
    json!({
        "contract": crate::execution_binding::CONTRACT,
        "principalRef": "principal:operator",
        "familiarId": familiar_id,
        "familiarSnapshotDigest": digest('a'),
        "projectDigest": digest('b'),
        "graphId": "graph-1",
        "nodeId": "node-1",
        "attemptId": "attempt-1",
        "requestDigest": digest('c'),
        "policyRevision": "policy:7",
        "expiresAt": "2099-01-01T00:00:00Z",
        "parent": null,
        "delegationDigest": null
    })
}
```

Add a bound root launch test that seeds `sage`, submits top-level `familiarId: "sage"` plus `executionBinding`, and asserts:

```rust
assert_eq!(response.status, 201);
let response: Value = serde_json::from_str(&response.body)?;
assert_eq!(response["familiar_id"], "sage");
assert_eq!(response["execution_binding"], root_binding("sage"));
assert_eq!(runtime.launches.borrow().len(), 1);
```

- [x] **Step 2: Run the root launch test and verify it fails**

Run:

```bash
cargo test -p coven-cli api::tests::bound_root_launch_persists_the_exact_binding
```

Expected: the response lacks `execution_binding` or launch parsing ignores it.

- [x] **Step 3: Parse binding after existing launch fields but before familiar resolution**

Keep `SessionLaunch` free of proof metadata. Change `session_launch_from_payload` to borrow `&Value`, preserving all current project/cwd/harness parsing:

```rust
fn session_launch_from_payload(payload: &Value) -> Result<SessionLaunch>
```

In `launch_session`:

```rust
let payload = match parse_body(body) { /* existing mapping */ };
let mut launch = match session_launch_from_payload(&payload) { /* existing mapping */ };
let execution_binding = match payload.get("executionBinding") {
    Some(value) => match crate::execution_binding::parse(value)
        .and_then(|binding| {
            binding.validate_not_expired(Utc::now())?;
            Ok(binding)
        }) {
        Ok(binding) => Some(binding),
        Err(error) => return execution_binding_error(error),
    },
    None => None,
};
```

Map module errors only through a dedicated helper:

```rust
fn execution_binding_error(
    error: crate::execution_binding::ValidationError,
) -> Result<ApiResponse> {
    use crate::execution_binding::ValidationError;
    match error {
        ValidationError::Invalid { path } => api_error(
            400,
            "execution_binding_invalid",
            "Execution binding is invalid.",
            Some(json!({ "fields": [path] })),
        ),
        ValidationError::Missing { path } => api_error(
            400,
            "execution_binding_invalid",
            "Execution binding is invalid.",
            Some(json!({ "fields": [path] })),
        ),
        ValidationError::Unsupported { path } => api_error(
            400,
            "execution_binding_unsupported",
            "Execution binding contract is unsupported.",
            Some(json!({ "fields": [path] })),
        ),
        ValidationError::Expired { path } => api_error(
            409,
            "execution_binding_expired",
            "Execution binding has expired.",
            Some(json!({ "fields": [path] })),
        ),
    }
}
```

No error details or message may contain caller values or digests.

- [x] **Step 4: Enforce root/child cross-field rules**

Add a route-level validator because `callerFamiliarId` is outside the binding:

```rust
fn validate_binding_relationship(
    binding: &crate::execution_binding::ExecutionBinding,
    caller_familiar_id: Option<&str>,
) -> std::result::Result<(), &'static str> {
    match (&binding.parent, &binding.delegation_digest, caller_familiar_id) {
        (None, None, None) => Ok(()),
        (Some(_), Some(_), Some(_)) => Ok(()),
        (None, _, _) => Err("executionBinding.parent"),
        (Some(_), None, _) => Err("executionBinding.delegationDigest"),
        (Some(_), Some(_), None) => Err("callerFamiliarId"),
    }
}
```

Call this in launch precedence step 2. Map any returned path to `400 execution_binding_invalid`.

- [x] **Step 5: Correlate the canonical familiar and parent before maintenance**

After existing `resolve_familiar`:

```rust
if let Some(binding) = execution_binding.as_ref() {
    let Some(familiar) = familiar_ctx.as_ref() else {
        return api_error(
            400,
            "execution_binding_invalid",
            "Bound launch requires familiarId.",
            Some(json!({ "fields": ["familiarId"] })),
        );
    };
    if binding.familiar_id != familiar.id {
        return execution_binding_mismatch("executionBinding.familiarId");
    }
}
```

Open the store before acquiring the maintenance writer. For child bindings:

1. Look up `parent.session_id`; absent -> existing `404 session_not_found`.
2. Require `parent.execution_binding`; null -> `409 execution_binding_mismatch` with only `parent.sessionId`.
3. Compare stored `familiar_id` to `callerFamiliarId`.
4. Compare the stored parent binding's `graph_id`, `node_id`, and `attempt_id` to the submitted `parent`.
5. Return the first mismatch path only; do not infer topology or delegation authority.

Freeze the familiar-correlation mismatch path as `callerFamiliarId`. Use
`parent.sessionId`, `parent.graphId`, `parent.nodeId`, and `parent.attemptId`
for the four parent-object comparisons. Use these same strings in tests and
`docs/API-CONTRACT.md`.

Use:

```rust
fn execution_binding_mismatch(path: &'static str) -> Result<ApiResponse> {
    api_error(
        409,
        "execution_binding_mismatch",
        "Execution binding does not match the stored session.",
        Some(json!({ "fields": [path] })),
    )
}
```

Move store opening and child-parent lookup ahead of the existing maintenance
gate block so the implemented order is parse -> binding validation ->
`resolve_familiar` -> canonical familiar/parent correlation -> maintenance
gate -> insert. Pass `execution_binding` into `NewSessionParams` only after
every validation and the maintenance gate succeed.

- [x] **Step 6: Reject bindings on external registration**

Immediately after successful JSON parsing in `register_external_session`, before reading registration fields:

```rust
if payload.get("executionBinding").is_some() {
    return api_error(
        400,
        "execution_binding_invalid",
        "External sessions cannot carry execution bindings.",
        Some(json!({ "fields": ["executionBinding"] })),
    );
}
```

- [x] **Step 7: Add the complete launch matrix**

Add focused tests for:

- valid root launch;
- valid child launch referencing an existing bound parent;
- missing top-level `familiarId`;
- canonical resolved familiar mismatch;
- root with `parent`, `delegationDigest`, or `callerFamiliarId`;
- child missing any one of `parent`, `delegationDigest`, or `callerFamiliarId`;
- missing parent -> `session_not_found`;
- existing unbound parent -> `execution_binding_mismatch` with only `parent.sessionId`;
- parent familiar/graph/node/attempt mismatch, one test per path;
- unknown contract, unknown top-level member, unknown parent member;
- empty/too-long/invalid-character opaque fields;
- malformed digest and timestamp;
- elapsed launch expiry;
- invalid launch creates no row and invokes no runtime;
- maintenance lock occurs after binding/familiar/parent errors;
- two identical root bindings on distinct launches both succeed;
- external registration with a binding -> `execution_binding_invalid`;
- messages/details never contain a submitted digest or identifier value.

The parent familiar mismatch must report `callerFamiliarId`; every parent
object mismatch must report the bare `parent.<field>` path.

- [x] **Step 8: Run the launch matrix**

Run:

```bash
cargo test -p coven-cli api::tests::bound_
cargo test -p coven-cli api::tests::external_session_rejects_execution_binding
```

Expected: every O2 launch/correlation test passes and existing unbound launch tests remain green.

- [x] **Step 9: Commit launch admission**

```bash
git add crates/coven-cli/src/api.rs
git commit -m "feat(api): bind Psyche session launches"
```

### Task 4: Require exact proof on bound input and kill without metadata leakage

**Files:**
- Modify: `crates/coven-cli/src/api.rs:714-721,2537-2705`
- Test: `crates/coven-cli/src/api.rs` test module near input/kill tests

- [x] **Step 1: Write failing bound-operation and isolation tests**

Change `RecordingRuntime` to retain the full input payload while preserving existing string assertions:

```rust
#[derive(Default)]
struct RecordingRuntime {
    launches: RefCell<Vec<SessionLaunch>>,
    inputs: RefCell<Vec<String>>,
    input_payloads: RefCell<Vec<Value>>,
    kills: RefCell<Vec<String>>,
}

fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
    self.input_payloads.borrow_mut().push(payload.clone());
    let data = payload.get("data").and_then(Value::as_str).unwrap_or_default();
    self.inputs.borrow_mut().push(format!("{session_id}:{data}"));
    Ok(())
}
```

Add a test that inserts a bound session, posts:

```rust
let binding = root_binding("sage");
let body = json!({
    "data": "hello",
    "executionBinding": binding
})
.to_string();
```

and asserts:

```rust
assert_eq!(response.status, 202);
assert_eq!(runtime.input_payloads.borrow().as_slice(), &[json!({"data": "hello"})]);
let events = store::list_events(&conn, "bound")?;
let input: Value = serde_json::from_str(&events.last().unwrap().payload_json)?;
assert_eq!(input, json!({"data": "hello"}));
```

Add a bound kill test that posts `{"executionBinding": binding}` and proves the runtime receives only the session id and the event remains `{"status":"killed"}`.

- [x] **Step 2: Run the isolation tests and verify they fail**

Run:

```bash
cargo test -p coven-cli api::tests::bound_input_strips_proof_from_runtime_and_event
cargo test -p coven-cli api::tests::bound_kill_consumes_proof_without_event_leak
```

Expected: bound proof is not enforced and/or the input payload still includes `executionBinding`.

- [x] **Step 3: Pass kill bodies through the router**

Change the kill route from:

```rust
kill_session(coven_home, session_id, runtime)
```

to:

```rust
kill_session(coven_home, session_id, body, runtime)
```

and add `body: Option<&str>` to `kill_session`.

- [x] **Step 4: Add context-aware proof parsing**

Add:

```rust
#[derive(Debug)]
enum BoundProofError {
    Required { path: &'static str },
    Validation(crate::execution_binding::ValidationError),
    Mismatch { path: &'static str },
}

fn require_bound_proof(
    payload: &Value,
    stored: &crate::execution_binding::ExecutionBinding,
) -> std::result::Result<crate::execution_binding::ExecutionBinding, BoundProofError> {
    let Some(value) = payload.get("executionBinding") else {
        return Err(BoundProofError::Required {
            path: "executionBinding",
        });
    };
    let supplied = crate::execution_binding::parse(value).map_err(|error| match error {
        crate::execution_binding::ValidationError::Missing { path } => {
            BoundProofError::Required { path }
        }
        other => BoundProofError::Validation(other),
    })?;
    if let Some(path) = stored.first_mismatch_path(&supplied) {
        return Err(BoundProofError::Mismatch { path });
    }
    Ok(supplied)
}
```

Map `BoundProofError` in each route without panicking:

```rust
let supplied = match require_bound_proof(&payload, stored) {
    Ok(supplied) => supplied,
    Err(BoundProofError::Required { path }) => {
        return api_error(
            400,
            "execution_binding_required",
            "Bound operation requires a complete executionBinding.",
            Some(json!({ "fields": [path] })),
        );
    }
    Err(BoundProofError::Validation(error)) => return execution_binding_error(error),
    Err(BoundProofError::Mismatch { path }) => {
        return execution_binding_mismatch(path);
    }
};
```

Preserve the exact outcomes:

- absent or incomplete proof -> `400 execution_binding_required`;
- malformed proof -> `400 execution_binding_invalid`;
- unknown contract -> `400 execution_binding_unsupported`;
- exact mismatch -> `409 execution_binding_mismatch`.

- [x] **Step 5: Freeze bound input precedence and strip proof**

For a bound session:

1. Look up session.
2. Parse the JSON body.
3. Require/parse exact proof.
4. Reject expired proof.
5. Apply existing status/external/liveness checks.
6. Validate `data`.
7. Build a new payload with only `data`.
8. Use that payload for capacity preflight, lease/event boundary, runtime call, and event recording.

The post-proof payload must be:

```rust
let data = payload
    .get("data")
    .and_then(Value::as_str)
    .ok_or_else(|| anyhow::anyhow!("input payload requires string field `data`"))?;
let action_payload = json!({ "data": data });
```

For an unbound session, retain current precedence: status/liveness remains ahead of body parsing, and existing requests continue to use `{"data": ...}`.

- [x] **Step 6: Freeze bound kill precedence and expiry exception**

For a bound session:

1. Look up session.
2. Parse body.
3. Require/parse exact proof.
4. Do **not** reject elapsed `expiresAt`.
5. Apply existing status and external checks.
6. Call `SessionRuntime::kill_session(session_id)`.
7. Persist the unchanged `{"status":"killed"}` event.

For an unbound session, preserve the current no-body behavior and error precedence.

- [x] **Step 7: Add the exhaustive mismatch and precedence matrix**

Use one table-driven test to clone a valid binding and substitute each of:

```text
principalRef
familiarId
familiarSnapshotDigest
projectDigest
graphId
nodeId
attemptId
requestDigest
policyRevision
expiresAt
parent
delegationDigest
parent.sessionId
parent.graphId
parent.nodeId
parent.attemptId
```

For every syntactically valid substitution, assert both input and kill return:

```rust
assert_eq!(body["error"]["code"], "execution_binding_mismatch");
assert_eq!(body["error"]["details"]["fields"], json!([expected_path]));
```

Add separate tests for:

- missing/incomplete proof on bound input and kill;
- malformed proof and unknown members;
- unknown contract -> `execution_binding_unsupported` (not mismatch);
- expired exact input -> `execution_binding_expired`;
- expired exact kill -> accepted;
- unknown session wins before malformed proof;
- valid proof mismatch wins before `session_not_live`;
- unbound completed/orphaned behavior remains `session_not_live`;
- input and kill rejection do not call the runtime or write events;
- mismatch details include field paths only and omit all caller values/digests;
- mixed-case opaque IDs persist unchanged, while input and kill proofs that
  differ only by ASCII letter case return `execution_binding_mismatch`;
- writer-backed input capacity checks see only `{"data": ...}`.

- [x] **Step 8: Run focused mutation tests**

Run:

```bash
cargo test -p coven-cli api::tests::bound_input_
cargo test -p coven-cli api::tests::bound_kill_
cargo test -p coven-cli api::tests::bound_operations_
cargo test -p coven-cli api::tests::input_and_kill_reject_
```

Expected: bound operations fail closed, unbound behavior is unchanged, and proof metadata never reaches runtime/event persistence.

- [x] **Step 9: Commit bound operations**

```bash
git add crates/coven-cli/src/api.rs
git commit -m "feat(api): enforce bound session mutations"
```

### Task 5: Advertise and consume the contract in the TypeScript client

**Files:**
- Modify: `crates/coven-cli/src/api.rs:118-143,336-360`
- Modify: `packages/openclaw-coven/src/client.ts:7-67,421-451,481-557`
- Modify: `packages/openclaw-coven/src/client.test.ts`
- Modify: `packages/openclaw-coven/src/compat.test.ts`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/health-available.json`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/health-daemon-null.json`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/session-running.json`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/session-completed.json`
- Modify: `packages/openclaw-coven/src/fixtures/v2026.4/sessions-list.json`

- [x] **Step 1: Write failing Rust health tests**

Extend the named-contract health test:

```rust
assert_eq!(
    body["capabilities"]["executionBindingContracts"],
    json!(["psyche.execution_binding.v1"])
);
```

Run:

```bash
cargo test -p coven-cli api::tests::health_is_the_named_contract_handshake
```

Expected: the capability is absent.

- [x] **Step 2: Add the additive health field**

Add to `HealthCapabilities`:

```rust
#[serde(default)]
pub execution_binding_contracts: Vec<String>,
```

Initialize it in `health_response`:

```rust
execution_binding_contracts: vec![crate::execution_binding::CONTRACT.to_string()],
```

Update every `HealthCapabilities` literal found by:

```bash
rg -n 'HealthCapabilities \{' crates/coven-cli/src
```

- [x] **Step 3: Write failing TypeScript normalization and request tests**

Define one `binding` fixture in `client.test.ts`. Add tests that prove:

- health preserves `executionBindingContracts` as an untrusted wire value;
- `execution_binding` normalizes to `executionBinding`;
- null remains null;
- an absent pre-O2 `execution_binding` field normalizes to null;
- bound launch sends the exact object;
- bound input sends exactly `{ data, executionBinding }`;
- bound kill sends exactly `{ executionBinding }`;
- unknown members at binding and parent level throw before an HTTP request;
- existing unbound input/kill request bodies remain unchanged.

- [x] **Step 4: Add exact TypeScript types and validator**

Add:

```typescript
export const PSYCHE_EXECUTION_BINDING_V1 = "psyche.execution_binding.v1" as const;

export type CovenExecutionBindingParent = {
  sessionId: string;
  graphId: string;
  nodeId: string;
  attemptId: string;
};

export type CovenExecutionBinding = {
  contract: typeof PSYCHE_EXECUTION_BINDING_V1;
  principalRef: string;
  familiarId: string;
  familiarSnapshotDigest: string;
  projectDigest: string;
  graphId: string;
  nodeId: string;
  attemptId: string;
  requestDigest: string;
  policyRevision: string;
  expiresAt: string;
  parent: CovenExecutionBindingParent | null;
  delegationDigest: string | null;
};

const EXECUTION_BINDING_KEYS = [
  "attemptId",
  "contract",
  "delegationDigest",
  "expiresAt",
  "familiarId",
  "familiarSnapshotDigest",
  "graphId",
  "nodeId",
  "parent",
  "policyRevision",
  "principalRef",
  "projectDigest",
  "requestDigest",
] as const;

const EXECUTION_BINDING_PARENT_KEYS = [
  "attemptId",
  "graphId",
  "nodeId",
  "sessionId",
] as const;

function requireExactKeys(
  record: JsonRecord,
  expected: readonly string[],
  label: string,
): void {
  const actual = Object.keys(record).sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new Error(`${label} has missing or unknown fields`);
  }
}

function requireBindingString(record: JsonRecord, key: string): string {
  const value = record[key];
  if (typeof value !== "string") {
    throw new Error(`executionBinding.${key} must be a string`);
  }
  return value;
}

function validOpaque(value: string): boolean {
  return Buffer.byteLength(value, "ascii") === value.length
    && value.length >= 1
    && value.length <= 255
    && /^[A-Za-z0-9._:/-]+$/.test(value);
}

function validDigest(value: string): boolean {
  return /^sha256:[0-9a-f]{64}$/.test(value);
}

function validCanonicalExpiry(value: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(value)) {
    return false;
  }
  const parsed = new Date(value);
  return Number.isFinite(parsed.valueOf())
    && parsed.toISOString().replace(".000Z", "Z") === value;
}

function normalizeExecutionBindingParent(value: unknown): CovenExecutionBindingParent {
  const record = requireRecord(value, "executionBinding.parent");
  requireExactKeys(record, EXECUTION_BINDING_PARENT_KEYS, "executionBinding.parent");
  const parent = {
    sessionId: requireBindingString(record, "sessionId"),
    graphId: requireBindingString(record, "graphId"),
    nodeId: requireBindingString(record, "nodeId"),
    attemptId: requireBindingString(record, "attemptId"),
  };
  for (const [key, field] of Object.entries(parent)) {
    if (!validOpaque(field)) {
      throw new Error(`executionBinding.parent.${key} is invalid`);
    }
  }
  return parent;
}

function normalizeExecutionBinding(value: unknown): CovenExecutionBinding {
  const record = requireRecord(value, "executionBinding");
  requireExactKeys(record, EXECUTION_BINDING_KEYS, "executionBinding");
  const contract = requireBindingString(record, "contract");
  if (contract !== PSYCHE_EXECUTION_BINDING_V1) {
    throw new Error("executionBinding.contract is unsupported");
  }

  const binding: CovenExecutionBinding = {
    contract,
    principalRef: requireBindingString(record, "principalRef"),
    familiarId: requireBindingString(record, "familiarId"),
    familiarSnapshotDigest: requireBindingString(record, "familiarSnapshotDigest"),
    projectDigest: requireBindingString(record, "projectDigest"),
    graphId: requireBindingString(record, "graphId"),
    nodeId: requireBindingString(record, "nodeId"),
    attemptId: requireBindingString(record, "attemptId"),
    requestDigest: requireBindingString(record, "requestDigest"),
    policyRevision: requireBindingString(record, "policyRevision"),
    expiresAt: requireBindingString(record, "expiresAt"),
    parent:
      record.parent === null ? null : normalizeExecutionBindingParent(record.parent),
    delegationDigest:
      record.delegationDigest === null
        ? null
        : requireBindingString(record, "delegationDigest"),
  };

  for (const [key, field] of [
    ["principalRef", binding.principalRef],
    ["familiarId", binding.familiarId],
    ["graphId", binding.graphId],
    ["nodeId", binding.nodeId],
    ["attemptId", binding.attemptId],
    ["policyRevision", binding.policyRevision],
  ] as const) {
    if (!validOpaque(field)) {
      throw new Error(`executionBinding.${key} is invalid`);
    }
  }
  for (const [key, field] of [
    ["familiarSnapshotDigest", binding.familiarSnapshotDigest],
    ["projectDigest", binding.projectDigest],
    ["requestDigest", binding.requestDigest],
  ] as const) {
    if (!validDigest(field)) {
      throw new Error(`executionBinding.${key} is invalid`);
    }
  }
  if (!validCanonicalExpiry(binding.expiresAt)) {
    throw new Error("executionBinding.expiresAt is invalid");
  }
  if (binding.delegationDigest !== null && !validDigest(binding.delegationDigest)) {
    throw new Error("executionBinding.delegationDigest is invalid");
  }
  if ((binding.parent === null) !== (binding.delegationDigest === null)) {
    throw new Error("executionBinding parent/delegationDigest relationship is invalid");
  }
  return binding;
}
```

Client validation is defense in depth; Rust remains authoritative.

- [x] **Step 5: Extend client response and health shapes**

Add:

```typescript
export type CovenSessionRecord = {
  id: string;
  projectRoot: string;
  harness: string;
  title: string;
  status: string;
  exitCode: number | null;
  createdAt: string;
  updatedAt: string;
  executionBinding: CovenExecutionBinding | null;
};

export type CovenHealthCapabilities = {
  sessions?: unknown;
  events?: unknown;
  eventCursor?: unknown;
  structuredErrors?: unknown;
  executionBindingContracts?: unknown;
};
```

In `normalizeHealthResponse` preserve:

```typescript
executionBindingContracts:
  capabilities.executionBindingContracts ?? capabilities.execution_binding_contracts,
```

In `normalizeSessionRecord`, preserve rolling-upgrade compatibility by treating
an absent pre-O2 field as unbound and normalize a present non-null value:

```typescript
const rawExecutionBinding =
  record.executionBinding ?? record.execution_binding ?? null;

return {
  id: requireStringField(record, "id", "id"),
  projectRoot: requireStringField(record, "projectRoot", "project_root"),
  harness: requireStringField(record, "harness", "harness"),
  title: requireStringField(record, "title", "title"),
  status: requireStringField(record, "status", "status"),
  exitCode: requireNullableNumberField(record, "exitCode", "exit_code"),
  createdAt: requireStringField(record, "createdAt", "created_at"),
  updatedAt: requireStringField(record, "updatedAt", "updated_at"),
  executionBinding:
    rawExecutionBinding === null
      ? null
      : normalizeExecutionBinding(rawExecutionBinding),
};
```

- [x] **Step 6: Add bound request support without breaking legacy callers**

Extend launch input:

```typescript
export type LaunchCovenSessionInput = {
  projectRoot: string;
  cwd: string;
  harness: string;
  prompt: string;
  title: string;
  familiarId?: string;
  callerFamiliarId?: string;
  executionBinding?: CovenExecutionBinding;
};
```

Keep existing `sendInput` and `killSession` unchanged. Add explicit bound methods:

```typescript
sendBoundInput(
  sessionId: string,
  data: string,
  executionBinding: CovenExecutionBinding,
  signal?: AbortSignal,
): Promise<void>;
killBoundSession(
  sessionId: string,
  executionBinding: CovenExecutionBinding,
  signal?: AbortSignal,
): Promise<void>;
```

Validate before sending:

```typescript
const binding = normalizeExecutionBinding(executionBinding);
body: { data, executionBinding: binding }
```

and:

```typescript
body: { executionBinding: normalizeExecutionBinding(executionBinding) }
```

`launchSession` must validate `input.executionBinding` when present before calling `requestJson`.

- [x] **Step 7: Update compatibility fixtures**

Add:

```json
"executionBindingContracts": ["psyche.execution_binding.v1"]
```

to both health capability fixtures.

Add:

```json
"execution_binding": null
```

to every unbound session object in `session-running.json`, `session-completed.json`, and `sessions-list.json`.

Do not change event fixtures.

- [x] **Step 8: Run Rust and TypeScript client tests**

Run:

```bash
cargo test -p coven-cli api::tests::health_is_the_named_contract_handshake
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test -- src/client.test.ts src/compat.test.ts
```

Expected: health advertises the contract, TypeScript validates/normalizes exact bindings, bound methods send exact bodies, and legacy methods remain compatible.

- [x] **Step 9: Commit client parity**

```bash
git add crates/coven-cli/src/api.rs packages/openclaw-coven/src/client.ts \
  packages/openclaw-coven/src/client.test.ts packages/openclaw-coven/src/compat.test.ts \
  packages/openclaw-coven/src/fixtures/v2026.4
git commit -m "feat(openclaw): support execution bindings"
```

### Task 6: Publish the O2 contract and reconcile canonical wording

**Files:**
- Modify: `docs/API-CONTRACT.md`
- Modify: `specs/psyche/RUNTIME_DESIGN.md`
- Modify: `specs/psyche/O2_CONTRACT_DESIGN.md`

- [x] **Step 1: Document capability negotiation and session response**

In `docs/API-CONTRACT.md`, add `executionBindingContracts` to the health example and capability table:

```markdown
| `executionBindingContracts` | string[] | Exact execution-binding contracts accepted by bound session launch/input/kill; currently `["psyche.execution_binding.v1"]`. |
```

Document that unbound session responses always include:

```json
"execution_binding": null
```

and bound responses include the complete typed object.

- [x] **Step 2: Document exact launch and mutation bodies**

Add the full 13-field root and child examples from `O2_CONTRACT_DESIGN.md`, then document:

```json
{
  "data": "hello",
  "executionBinding": {
    "contract": "psyche.execution_binding.v1",
    "principalRef": "principal:operator",
    "familiarId": "sage",
    "familiarSnapshotDigest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "projectDigest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "graphId": "graph-1",
    "nodeId": "node-1",
    "attemptId": "attempt-1",
    "requestDigest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "policyRevision": "policy:7",
    "expiresAt": "2099-01-01T00:00:00Z",
    "parent": null,
    "delegationDigest": null
  }
}
```

and:

```json
{
  "executionBinding": {
    "contract": "psyche.execution_binding.v1",
    "principalRef": "principal:operator",
    "familiarId": "sage",
    "familiarSnapshotDigest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "projectDigest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "graphId": "graph-1",
    "nodeId": "node-1",
    "attemptId": "attempt-1",
    "requestDigest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "policyRevision": "policy:7",
    "expiresAt": "2099-01-01T00:00:00Z",
    "parent": null,
    "delegationDigest": null
  }
}
```

State explicitly:

- closed objects reject unknown members;
- opaque values are byte-exact and unnormalized;
- launch and input reject elapsed expiry;
- exact kill may proceed after expiry;
- read/list/events require no proof;
- proof metadata is stripped before runtime/event delivery;
- O2 is correlation, not authentication or replay prevention.

Freeze mismatch detail paths as `callerFamiliarId` for stored-parent familiar
correlation and bare `parent.<field>` for parent object fields. Fully qualified
`executionBinding.parent.<field>` paths remain reserved for shape-validation
errors, not exact-match mismatches.

- [x] **Step 3: Publish the six exact errors and precedence**

Add the six rows:

```markdown
| `execution_binding_invalid` | 400 | Malformed, incomplete-at-launch, extra-member, cross-field-invalid, or external registration binding. |
| `execution_binding_unsupported` | 400 | Unknown `contract`. |
| `execution_binding_required` | 400 | Bound input/kill omitted or supplied incomplete proof. |
| `execution_binding_expired` | 409 | Launch/input binding has elapsed. |
| `execution_binding_mismatch` | 409 | Exact comparison or parent correlation failed. |
| `session_not_found` | 404 | Current or referenced parent session is absent. |
```

Document field-path-only details and the launch/input/kill precedence from the design.

- [x] **Step 4: Correct the runtime contract inventory**

Replace the stale `RUNTIME_DESIGN.md` row:

```markdown
| `psyche.execution_binding.v1` | Stable attempt and request IDs, payload digest, Coven adoption resolution, event cursor, and terminal correlation. |
```

with:

```markdown
| `psyche.execution_binding.v1` | O2 immutable opaque launch/session binding and exact mismatch correlation; O3-O7 separately own adoption, lookup/fencing, cancellation acknowledgement, artifacts, and recovery. |
```

Do not add O3-O7 fields to the O2 object.

- [x] **Step 5: Record approval without claiming final verification**

After Tasks 1-5 are green, change the O2 header to:

```markdown
**Status:** Approved; implementation complete pending final repository verification.
```

Do not mark the contract fully implemented yet. Leave issue/Bead evidence unchecked until those records contain final merge and verification links.

- [x] **Step 6: Validate terminology**

Run:

```bash
rg -n 'psyche\.execution_binding\.v1|executionBindingContracts|execution_binding_' \
  docs/API-CONTRACT.md specs/psyche/RUNTIME_DESIGN.md specs/psyche/O2_CONTRACT_DESIGN.md
rg -n 'adoption resolution|event cursor|terminal correlation' specs/psyche/RUNTIME_DESIGN.md
```

Expected: O2 descriptions are limited to immutable correlation/exact mismatch; later lifecycle capabilities remain assigned to O3-O7.

- [x] **Step 7: Commit documentation**

```bash
git add docs/API-CONTRACT.md specs/psyche/RUNTIME_DESIGN.md \
  specs/psyche/O2_CONTRACT_DESIGN.md
git commit -m "docs: publish Psyche O2 execution binding"
```

### Task 7: Run complete O2 verification and prepare review evidence

**Files:**
- Verify: all files listed in the file map

- [x] **Step 1: Run formatting and focused tests**

Run:

```bash
cargo fmt --check
cargo test -p coven-cli execution_binding::tests::
cargo test -p coven-cli store::tests::execution_binding_
cargo test -p coven-cli api::tests::bound_
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test -- src/client.test.ts src/compat.test.ts
```

Expected: all commands exit 0.

- [x] **Step 2: Run the full repository gates**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
npm --prefix packages/openclaw-coven test
```

Expected: all commands exit 0 with no warnings, test failures, or secret findings.

- [x] **Step 3: Scan the complete O2 branch diff and run diff checks**

Run:

```bash
python3 scripts/check-coven-privacy.py --range origin/main...HEAD
git diff origin/main...HEAD --check
git diff origin/main...HEAD --stat
```

Expected: privacy and whitespace checks pass; the branch stat contains only the
approved plan and O2 files from the file map.

- [x] **Step 4: Audit O2 non-goals mechanically**

Run:

```bash
git diff origin/main...HEAD -- \
  crates/coven-cli/src packages/openclaw-coven/src docs/API-CONTRACT.md specs/psyche |
  rg -n 'adoptionKey|adoption_state|lookup-by-binding|uniqueness|UNIQUE.*execution_binding|cancel.*ack|artifact.*binding'
```

Expected: matches occur only in documentation that explicitly states these are O3-O7 non-goals; no production code or SQL implements them.

- [x] **Step 5: Record the readiness packet**

Attach to the implementation issue/PR:

```text
Changed files:
- Rust binding type/validation, session persistence, API enforcement
- TypeScript client parity and fixtures
- API and Psyche contract documentation

Focused verification:
- cargo test -p coven-cli execution_binding::tests::
- cargo test -p coven-cli store::tests::execution_binding_
- cargo test -p coven-cli api::tests::bound_
- npm --prefix packages/openclaw-coven run typecheck
- npm --prefix packages/openclaw-coven test -- src/client.test.ts src/compat.test.ts

Full verification:
- cargo fmt --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace --locked
- python scripts/check-secrets.py
- python3 scripts/check-coven-privacy.py --staged
- npm --prefix packages/openclaw-coven test

Known limitations:
- O2 does not provide adoption uniqueness, replay prevention, lookup/fencing,
  cancellation acknowledgement, artifact binding, or crash-matrix recovery.
```

- [x] **Step 6: Mark the contract implemented after every gate passes**

Change the O2 header to:

```markdown
**Status:** Approved and implemented; merge/verification evidence is recorded by the implementation issue and PR.
```

Check every acceptance item satisfied by the final diff. Leave merge-specific evidence unchecked until the PR merges.

- [x] **Step 7: Commit final verification corrections**

If formatting or documentation corrections were required:

```bash
git add -u
git commit -m "test: close Psyche O2 conformance gaps"
```

The status update in Step 6 always requires this final commit. The dedicated O2 worktree must contain no unrelated tracked modifications, so `git add -u` stages only files already tracked by the O2 commits.

After the final commit, rerun:

```bash
python3 scripts/check-coven-privacy.py --range origin/main...HEAD
git diff origin/main...HEAD --check
```

Expected: both final branch-diff checks pass.
