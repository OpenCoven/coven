# Psyche W2 G2 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Psyche W2 and produce terminal G2 evidence for canonical versioned records, durable SQLite migrations, behavior-level fake boundaries, and migration/state-machine/property/crash tests.

**Architecture:** Extend the merged four-crate bootstrap with the focused crates named by the canonical architecture. `psyche-core` owns policy-free identifiers, digests, schema names, and minimum record shapes; `psyche-store` owns SQLite, migrations, immutable records, append-only transitions, quarantine, and retention; `psyche-coven` and `psyche-surfaces` own behavior-level traits for their respective boundaries; `psyche-test-support` owns deterministic scripted fakes and crash fixtures. `psyche-runtime` remains the only composition root. W3-W9 retain identity resolution, graph admission, orchestration policy, real Coven integration, Telegram behavior, verification, and add-on trust.

**Tech Stack:** Rust 2024 on 1.85, serde/serde_json, RFC 8785 JSON canonicalization, SHA-256, ULID, rusqlite with bundled SQLite, Tokio, proptest, tempfile, assert_cmd, cargo-deny, gitleaks.

---

## Scope boundary

This is the second and final W2 child plan. It begins only after
`OpenCoven/psyche#1` is squash-merged and `main` is restored as the default
branch.

| In scope | Explicitly deferred |
|---|---|
| Stable schema identifiers and strict decoding | Identity-file resolution and principal mapping (W3) |
| Minimum typed records frozen by W0 | Graph admission, dependency-cycle policy, budgets, and orchestration (W4) |
| Canonical JSON and content digests | Real Coven endpoints, adoption, cancellation, and artifacts (W5/G4) |
| SQLite migrations, immutable records, append-only transitions | Telegram normalization, routing, effects, and delivery (W6/G8) |
| Quarantine, retention, reopen, and crash atomicity | Verdict policy and independent verification (W7/G5) |
| Behavior-level ports and deterministic fakes | Production child dispatch (W8/G6) |
| Reusable fake conformance fixtures | Add-on trust and execution (W9/G7) |

This plan does not set `psyche.graphs.v1` until the final G2 evidence review
passes. It does not set any G4-G12 capability.

## File map

### Existing files modified

- `Cargo.toml` - add workspace crates and pinned dependencies.
- `crates/psyche-core/Cargo.toml` - add JSON, digest, time, and ULID dependencies.
- `crates/psyche-core/src/lib.rs` - expose contract modules.
- `crates/psyche-runtime/Cargo.toml` - depend on `psyche-store`.
- `crates/psyche-runtime/src/lib.rs` - open the store at startup and checkpoint it during drain.
- `.github/workflows/ci.yml` - run G2 property and crash suites on every supported host.
- `docs/ARCHITECTURE.md` - record crate ownership and dependency direction.
- `docs/SCHEMAS.md` - document G2 record and quarantine contracts.
- `docs/TESTING.md` - document deterministic fake and crash commands.

### New `psyche-core` files

- `crates/psyche-core/src/id.rs` - validated prefixed ULID identifiers.
- `crates/psyche-core/src/digest.rs` - RFC 8785 canonicalization and SHA-256 digests.
- `crates/psyche-core/src/contracts/mod.rs` - schema registry and strict decode dispatch.
- `crates/psyche-core/src/contracts/identity.rs` - `psyche.identity_snapshot.v1`.
- `crates/psyche-core/src/contracts/intent.rs` - `psyche.intent.v1`.
- `crates/psyche-core/src/contracts/graph.rs` - minimum graph/node records and state names.
- `crates/psyche-core/src/contracts/execution.rs` - policy-free execution binding fields.
- `crates/psyche-core/src/contracts/foundation.rs` - minimum delegation, budget, approval, evidence, verdict, recovery, add-on, and delivery records.
- `crates/psyche-core/src/contracts/surface.rs` - canonical surface event/effect envelopes.
- `crates/psyche-core/src/contracts/error.rs` - redacted `psyche.error.v1`.

### New crates

- `crates/psyche-store/` - migrations, connection, records, transitions, quarantine, retention.
- `crates/psyche-coven/` - behavior-level Coven trait and request/result types.
- `crates/psyche-surfaces/` - behavior-level surface trait and acceptance/delivery types.
- `crates/psyche-test-support/` - scripted fakes and crash helper binary.

## Fixed public boundaries

```rust
// psyche-core
pub trait VersionedRecord: serde::Serialize {
    fn schema_version(&self) -> SchemaVersion;
    fn record_id(&self) -> &RecordId;
}

pub fn canonical_bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ContractError>;
pub fn digest<T: serde::Serialize>(value: &T) -> Result<Sha256Digest, ContractError>;
pub fn decode_document(bytes: &[u8]) -> Result<CanonicalDocument, ContractError>;

// psyche-store
pub struct Store;
impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError>;
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<IngestOutcome, StoreError>;
    pub fn insert(&mut self, document: &CanonicalDocument) -> Result<(), StoreError>;
    pub fn append_transition(&mut self, transition: &Transition) -> Result<(), StoreError>;
    pub fn load(&self, kind: SchemaKind, id: &RecordId)
        -> Result<Option<CanonicalDocument>, StoreError>;
    pub fn load_canonical_bytes(&self, kind: SchemaKind, id: &RecordId)
        -> Result<Option<Vec<u8>>, StoreError>;
    pub fn quarantine(&mut self, rejected: RejectedDocument)
        -> Result<QuarantineId, StoreError>;
    pub fn prune(&mut self, cutoff: time::OffsetDateTime)
        -> Result<PruneReport, StoreError>;
    pub fn checkpoint(&mut self) -> Result<(), StoreError>;
}

// psyche-coven
#[async_trait::async_trait]
pub trait CovenPort: Send + Sync {
    async fn negotiate(&self, request: NegotiateRequest)
        -> Result<CapabilityProfile, PortError>;
    async fn adopt(&self, request: AdoptionRequest)
        -> Result<AdoptionDisposition, PortError>;
    async fn lookup(&self, request_id: &RequestId)
        -> Result<AdoptionDisposition, PortError>;
    async fn inspect(&self, session_id: &str)
        -> Result<SessionSnapshot, PortError>;
    async fn events(&self, cursor: EventCursor)
        -> Result<EventPage, PortError>;
    async fn result(&self, session_id: &str)
        -> Result<ResultBundle, PortError>;
    async fn terminate(&self, request: TerminationRequest)
        -> Result<TerminationDisposition, PortError>;
}

// psyche-surfaces
#[async_trait::async_trait]
pub trait SurfacePort: Send + Sync {
    async fn accept(&self, event: SurfaceEvent)
        -> Result<SurfaceAcceptance, PortError>;
    async fn apply(&self, effect: SurfaceEffect)
        -> Result<DeliveryDisposition, PortError>;
}
```

`RecordKind::Attempt` validates `att_` references embedded in execution,
evidence, and recovery records. There is no `psyche.attempt.v1`,
`SchemaKind::Attempt`, or `CanonicalDocument::Attempt`.

These traits name behavior, not Coven endpoints or Telegram methods. The fake
and future real adapters must run the same behavior-level assertions.

## Security and privacy failure cases

Every item below has a named automated test:

1. Unknown schema major versions are quarantined before dispatch or state mutation.
2. Unknown fields are rejected unless that exact schema opts into additive fields.
3. Canonicalization rejects non-finite numbers and duplicate JSON keys.
4. Record digests exclude no mutable field: changing any serialized field changes the digest.
5. Immutable records reject same-ID/different-digest reinsertion.
6. Transition rows append atomically; no API updates or deletes historical rows.
7. Raw rejected payloads are not written to logs or error messages.
8. Quarantine retains bounded bytes and a digest, not an unbounded diagnostic echo.
9. SQLite enables foreign keys, WAL, secure delete, and a busy timeout on every connection.
10. Unknown database schema versions fail before migrations or application reads.
11. A crash before commit exposes no partial record or transition after reopen.
12. Retention never deletes unresolved quarantine or the newest state transition.
13. Fake services never advertise behavior that their script cannot execute.
14. No fake result is described as current real-Coven conformance.

## Gate mapping

- **G2 Contract foundation:** this plan supplies canonical records, strict
  version denial, migrations, fake ports, state-machine/property tests, and
  crash/restart evidence.
- **G4 Single-node conformance:** not claimed. W5 must run the unchanged Coven
  behavior suite against a pinned real daemon.
- **G8 Adapter reliability:** not claimed. W6 supplies a real surface adapter
  and later runs the unchanged surface suite.

---

### Task 0: Reconcile the bootstrap merge and claim the unit

**Files:**
- None (coordination only)

- [ ] **Step 1: Prove the bootstrap landed**

```bash
gh pr view 1 --repo OpenCoven/psyche \
  --json state,mergedAt,mergeCommit \
  --jq '{state,mergedAt,sha:.mergeCommit.oid}'
```

Expected: `state` is `MERGED`, `mergedAt` is non-null, and `sha` is non-null.
Stop if PR #1 is open.

- [ ] **Step 2: Prove `main` is the default branch**

```bash
gh repo view OpenCoven/psyche --json defaultBranchRef \
  --jq .defaultBranchRef.name
```

Expected: `main`. Stop and ask the repository owner to restore it if another
branch is returned.

- [ ] **Step 3: Create the implementation issue and worktree**

```bash
test "$(gh repo view --json nameWithOwner --jq .nameWithOwner)" = "OpenCoven/psyche"
git fetch origin main
issue_url=$(gh issue create \
  --title "feat: complete Psyche W2 contract foundation" \
  --body "Implements the approved follow-on W2 plan: canonical records, SQLite migrations, fake boundaries, and G2 property/crash evidence. No W3-W9 behavior.")
issue_number=${issue_url##*/}
git worktree add -b feat/psyche-g2-foundation \
  .worktrees/psyche-g2-foundation origin/main
cd .worktrees/psyche-g2-foundation
coven claim acquire "issue-${issue_number}"
```

Expected: a fresh worktree at `origin/main` and an active shared issue claim;
do not use a branch-keyed claim.

- [ ] **Step 4: Record the pre-implementation gate**

```bash
git status --short --branch
```

Expected: `## feat/psyche-g2-foundation...origin/main` and no changed files.

---

### Task 1: Add the G2 crates and pinned dependencies

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/psyche-store/Cargo.toml`
- Create: `crates/psyche-store/src/lib.rs`
- Create: `crates/psyche-coven/Cargo.toml`
- Create: `crates/psyche-coven/src/lib.rs`
- Create: `crates/psyche-surfaces/Cargo.toml`
- Create: `crates/psyche-surfaces/src/lib.rs`
- Create: `crates/psyche-test-support/Cargo.toml`
- Create: `crates/psyche-test-support/src/lib.rs`

- [ ] **Step 1: Write the failing workspace membership check**

Create `scripts/check-g2-workspace.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
metadata=$(cargo metadata --no-deps --format-version 1)
for crate in psyche-store psyche-coven psyche-surfaces psyche-test-support; do
  jq -e --arg crate "$crate" '.packages[] | select(.name == $crate)' \
    <<<"$metadata" >/dev/null
done
```

- [ ] **Step 2: Run it and verify RED**

```bash
bash scripts/check-g2-workspace.sh
```

Expected: non-zero because `psyche-store` is absent.

- [ ] **Step 3: Add exact workspace dependencies**

Add these members and dependencies to root `Cargo.toml`:

```toml
members = [
    "crates/psyche-core",
    "crates/psyche-config",
    "crates/psyche-runtime",
    "crates/psyche-cli",
    "crates/psyche-store",
    "crates/psyche-coven",
    "crates/psyche-surfaces",
    "crates/psyche-test-support",
]

[workspace.dependencies]
psyche-store = { path = "crates/psyche-store" }
psyche-coven = { path = "crates/psyche-coven" }
psyche-surfaces = { path = "crates/psyche-surfaces" }
psyche-test-support = { path = "crates/psyche-test-support" }
async-trait = "0.1"
rusqlite = { version = "0.32", features = ["bundled"] }
serde_json_canonicalizer = "0.3"
sha2 = "0.10"
time = { version = "0.3", features = ["formatting", "parsing", "serde"] }
ulid = { version = "1", features = ["serde"] }
proptest = "1"
```

Create each crate manifest with inherited package metadata, `publish.workspace
= true`, workspace lints, and only the dependencies required by its public
boundary. Verify root `[workspace.package] publish = false` before doing so;
additionally set `publish = false` directly in `psyche-test-support` so test
fakes and crash binaries cannot become publishable if workspace policy changes.

- [ ] **Step 4: Add minimal compiling libraries**

```rust
// crates/psyche-store/src/lib.rs
//! Durable SQLite substrate for Psyche contracts.

// crates/psyche-coven/src/lib.rs
//! Behavior-level Coven execution boundary.

// crates/psyche-surfaces/src/lib.rs
//! Behavior-level surface acceptance and delivery boundary.

// crates/psyche-test-support/src/lib.rs
//! Deterministic fakes and crash fixtures for Psyche conformance tests.
```

- [ ] **Step 5: Run the workspace check and compile**

```bash
bash scripts/check-g2-workspace.sh
cargo check --workspace
```

Expected: both exit 0.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock scripts/check-g2-workspace.sh crates/psyche-{store,coven,surfaces,test-support}
git commit -m "chore: add Psyche G2 foundation crates"
```

---

### Task 2: Add validated identifiers, schema names, and canonical digests

**Files:**
- Modify: `crates/psyche-core/Cargo.toml`
- Modify: `crates/psyche-core/src/lib.rs`
- Create: `crates/psyche-core/src/id.rs`
- Create: `crates/psyche-core/src/digest.rs`
- Create: `crates/psyche-core/src/contracts/mod.rs`

- [ ] **Step 1: Write failing identifier and digest tests**

In `id.rs` and `digest.rs`, add:

```rust
#[test]
fn record_id_rejects_the_wrong_prefix() {
    assert!(RecordId::parse(RecordKind::Intent, "grf_01J00000000000000000000000").is_err());
}

#[test]
fn canonical_digest_ignores_object_key_order() {
    let left: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
    let right: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
    assert_eq!(digest(&left).unwrap(), digest(&right).unwrap());
}

proptest::proptest! {
    #[test]
    fn any_value_change_changes_digest(value in "[a-zA-Z0-9]{1,64}") {
        let left = serde_json::json!({"value": value});
        let right = serde_json::json!({"value": format!("{value}x")});
        proptest::prop_assert_ne!(digest(&left).unwrap(), digest(&right).unwrap());
    }
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-core canonical_digest_ -- --nocapture
```

Expected: compile failure because `RecordId` and `digest` do not exist.

- [ ] **Step 3: Implement the public primitives**

Use these exact shapes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    IdentitySnapshot, Intent, Graph, GraphNode, Attempt, Delegation, Budget,
    Approval, ExecutionBinding, Evidence, Verdict, Recovery, Addon,
    SurfaceEvent, SurfaceEffect, Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RecordId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaKind {
    IdentitySnapshot, Intent, SurfaceEvent, Graph, GraphNode, Delegation,
    Budget, Approval, ExecutionBinding, Evidence, Verdict, Recovery, Addon,
    SurfaceEffect, Delivery, Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SchemaVersion {
    pub kind: SchemaKind,
    pub major: u16,
}
```

`RecordId::parse` must enforce the W0 prefixes (`ids_`, `int_`, `grf_`,
`nod_`, `att_`, `del_`, `bud_`, `apr_`, `req_`, `evd_`, `vrd_`, `rcv_`,
`adn_`, `sev_`, `sfx_`, `dly_`) and parse the suffix as a ULID.
`canonical_bytes` must call `serde_json_canonicalizer::to_vec`; `digest` must
return the `sha256:` prefix followed by exactly 64 lowercase hexadecimal
characters using `sha2::Sha256`.

`TryFrom<String>` validates every deserialization path for IDs, digests, and
schema versions. Each record's `validate()` additionally checks that its ID
fields use the field-specific `RecordKind`; `decode_document` calls
`validate()` before returning a `CanonicalDocument`. A generic recognized
prefix in the wrong field is therefore rejected.

- [ ] **Step 4: Add the complete schema registry**

`SchemaVersion::parse` accepts exactly the 15 W0 domain contract names from
`specs/psyche/TECH.md`, plus the boundary envelope `psyche.error.v1`, at major
`1`. Unknown kinds return
`ContractError::UnknownSchema`; known kinds with another major return
`ContractError::UnsupportedMajor`.

- [ ] **Step 5: Run focused and crate tests**

```bash
cargo test -p psyche-core
cargo clippy -p psyche-core --all-targets -- -D warnings
```

Expected: all tests pass and no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-core Cargo.toml Cargo.lock
git commit -m "feat(core): add canonical contract primitives"
```

---

### Task 3: Add policy-free minimum canonical records

**Files:**
- Create: `crates/psyche-core/src/contracts/identity.rs`
- Create: `crates/psyche-core/src/contracts/intent.rs`
- Create: `crates/psyche-core/src/contracts/graph.rs`
- Create: `crates/psyche-core/src/contracts/execution.rs`
- Create: `crates/psyche-core/src/contracts/foundation.rs`
- Create: `crates/psyche-core/src/contracts/surface.rs`
- Create: `crates/psyche-core/src/contracts/error.rs`
- Modify: `crates/psyche-core/src/contracts/mod.rs`

- [ ] **Step 1: Write strict-decoding tests**

```rust
#[test]
fn intent_rejects_unknown_fields() {
    let json = br#"{
      "schema_version":"psyche.intent.v1",
      "intent_id":"int_01J00000000000000000000000",
      "principal_id":"principal:val",
      "familiar_snapshot_id":"ids_01J00000000000000000000000",
      "project_id":"project:sha256:abc",
      "requested_outcome":"Review the change.",
      "constraints":{},
      "required_evidence":["tests"],
      "surface_event_id":null,
      "created_at":"2026-08-05T00:00:00Z",
      "digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "unexpected":true
    }"#;
    assert!(matches!(decode_document(json), Err(ContractError::InvalidShape { .. })));
}

#[test]
fn graph_and_node_accept_only_the_two_frozen_nullable_bindings() {
    // surface_event_id and delegation_id may be null; required IDs may not.
    assert!(decode_document(include_bytes!("../../../tests/fixtures/intent-local.json")).is_ok());
    assert!(decode_document(include_bytes!("../../../tests/fixtures/node-root.json")).is_ok());
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-core intent_rejects_unknown_fields
```

Expected: compile failure because the record types are absent.

- [ ] **Step 3: Implement the frozen minimum records**

All wire structs derive `Serialize` and `Deserialize` with
`#[serde(deny_unknown_fields)]`. Use `time::OffsetDateTime` with RFC 3339 serde.
Implement:

```rust
pub struct IdentitySnapshot {
    pub schema_version: SchemaVersion,
    pub snapshot_id: RecordId,
    pub familiar_id: String,
    pub principal_id: String,
    pub revision: u64,
    pub declaration_digest: Sha256Digest,
    pub identity_file_digest: Sha256Digest,
    pub identity_digest: Sha256Digest,
    pub soul_digest: Sha256Digest,
    pub role_skill_digest: Sha256Digest,
    pub provenance: IdentityProvenance,
    pub resolved_at: time::OffsetDateTime,
}

pub struct Intent {
    pub schema_version: SchemaVersion,
    pub intent_id: RecordId,
    pub principal_id: String,
    pub familiar_snapshot_id: RecordId,
    pub project_id: String,
    pub requested_outcome: String,
    pub constraints: serde_json::Map<String, serde_json::Value>,
    pub required_evidence: Vec<String>,
    pub surface_event_id: Option<RecordId>,
    pub created_at: time::OffsetDateTime,
    pub digest: Sha256Digest,
}

pub struct Graph {
    pub schema_version: SchemaVersion,
    pub graph_id: RecordId,
    pub root_intent_id: RecordId,
    pub owner_principal_id: String,
    pub policy_revision: String,
    pub state: GraphState,
    pub version: u64,
}

pub struct GraphNode {
    pub schema_version: SchemaVersion,
    pub node_id: RecordId,
    pub graph_id: RecordId,
    pub familiar_snapshot_id: RecordId,
    pub dependencies: Vec<RecordId>,
    pub delegation_id: Option<RecordId>,
    pub budget_id: RecordId,
    pub required_evidence: Vec<String>,
    pub state: NodeState,
    pub version: u64,
}
```

Define the W0 `GraphState`, `NodeState`, `AdoptionState`, and
`CancellationState` spellings exactly as listed in `TECH.md`; do not add
transition methods. Add `ExecutionBinding` with the exact fields from the
`psyche.execution_binding.v1` example. Add `SurfaceEvent` and `SurfaceEffect`
as strict canonical envelopes whose adapter-owned payload is a bounded
`serde_json::Value`; adapters interpret it later.

- [ ] **Step 4: Add policy-free shapes for the remaining registry**

Use identifier/digest/string fields rather than enforcing later gate policy:

```rust
pub struct Delegation {
    pub schema_version: SchemaVersion,
    pub delegation_id: RecordId,
    pub parent_node_id: RecordId,
    pub child_node_id: RecordId,
    pub scope_digest: Sha256Digest,
    pub budget_id: RecordId,
    pub evidence_scope_digest: Sha256Digest,
    pub cancellation_policy: String,
}

pub struct Budget {
    pub schema_version: SchemaVersion,
    pub budget_id: RecordId,
    pub graph_id: RecordId,
    pub resource_class: String,
    pub limit: u64,
    pub reserved: u64,
    pub consumed: u64,
    pub released: u64,
}

pub struct Approval {
    pub schema_version: SchemaVersion,
    pub approval_id: RecordId,
    pub node_id: RecordId,
    pub requester_principal_id: String,
    pub decision: Option<String>,
    pub expires_at: time::OffsetDateTime,
}

pub struct Evidence {
    pub schema_version: SchemaVersion,
    pub evidence_id: RecordId,
    pub node_id: RecordId,
    pub attempt_id: RecordId,
    pub content_digest: Sha256Digest,
    pub producer: String,
    pub collection_method: String,
    pub media_type: String,
    pub size: u64,
    pub created_at: time::OffsetDateTime,
    pub retention_policy: String,
}

pub struct Verdict {
    pub schema_version: SchemaVersion,
    pub verdict_id: RecordId,
    pub node_id: RecordId,
    pub sealed_evidence_digest: Sha256Digest,
    pub policy_revision: String,
    pub verdict_type: String,
    pub reviewer_id: String,
    pub outcome: String,
    pub reason_codes: Vec<String>,
    pub created_at: time::OffsetDateTime,
}

pub struct Recovery {
    pub schema_version: SchemaVersion,
    pub recovery_id: RecordId,
    pub attempt_id: RecordId,
    pub lease_id: String,
    pub fence_token: Option<String>,
    pub ambiguity: String,
    pub reconciliation_count: u64,
    pub operator_disposition: Option<String>,
}

pub struct Addon {
    pub schema_version: SchemaVersion,
    pub addon_id: RecordId,
    pub package: String,
    pub version: String,
    pub package_digest: Sha256Digest,
    pub provenance_digest: Sha256Digest,
    pub contributions_digest: Sha256Digest,
    pub allowlist_digest: Sha256Digest,
    pub revocation_state: String,
}

pub struct Delivery {
    pub schema_version: SchemaVersion,
    pub delivery_id: RecordId,
    pub surface_effect_id: RecordId,
    pub surface_decision_digest: Sha256Digest,
    pub state: String,
    pub attempts: Vec<DeliveryAttempt>,
}

pub struct DeliveryAttempt {
    pub attempt_id: RecordId,
    pub request_digest: Sha256Digest,
    pub disposition: String,
    pub created_at: time::OffsetDateTime,
}
```

These structs freeze names and wire types only. W4/W6/W7/W9 replace their
free-form state/policy strings with approved enums and validators in
forward-compatible child migrations; G2 code does not evaluate them.

- [ ] **Step 5: Add typed error envelopes**

Implement `ErrorEnvelope`, `ErrorBody`, and the G2-owned codes:
`config_invalid`, `storage_unavailable`, `event_schema_unsupported`, and
`graph_invalid`. `details` accepts only string-to-string public classifications;
it must not accept arbitrary JSON.

- [ ] **Step 6: Run contract and serialization tests**

```bash
cargo test -p psyche-core contracts::
cargo test -p psyche-core --doc
```

Expected: strict fixtures round-trip byte-for-byte after canonicalization;
unknown fields, unknown enum values, and unknown major versions fail.

- [ ] **Step 7: Commit**

```bash
git add crates/psyche-core
git commit -m "feat(core): define minimum Psyche v1 records"
```

---

### Task 4: Implement strict decode and quarantine inputs

**Files:**
- Modify: `crates/psyche-core/src/contracts/mod.rs`
- Create: `crates/psyche-core/tests/decode.rs`

- [ ] **Step 1: Write failing unknown-version tests**

```rust
#[test]
fn unknown_major_never_decodes_as_a_known_record() {
    let bytes = br#"{"schema_version":"psyche.intent.v2","intent_id":"int_01J00000000000000000000000"}"#;
    let error = decode_document(bytes).unwrap_err();
    assert!(matches!(error, ContractError::UnsupportedMajor { found: 2, .. }));
    assert!(!error.to_string().contains("intent_id"));
}

#[test]
fn malformed_payload_is_bounded_before_quarantine() {
    let bytes = vec![b'x'; MAX_DOCUMENT_BYTES + 1];
    assert!(matches!(decode_document(&bytes), Err(ContractError::TooLarge { .. })));
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-core --test decode
```

Expected: failure because `MAX_DOCUMENT_BYTES` and bounded decode are absent.

- [ ] **Step 3: Implement two-stage decoding**

Set `MAX_DOCUMENT_BYTES` to 1 MiB. First deserialize only:

```rust
#[derive(serde::Deserialize)]
struct VersionProbe {
    schema_version: String,
}
```

Then dispatch to the exact typed record. Return:

```rust
pub enum CanonicalDocument {
    IdentitySnapshot(IdentitySnapshot),
    Intent(Intent),
    Graph(Graph),
    GraphNode(GraphNode),
    ExecutionBinding(ExecutionBinding),
    Delegation(Delegation),
    Budget(Budget),
    Approval(Approval),
    Evidence(Evidence),
    Verdict(Verdict),
    Recovery(Recovery),
    Addon(Addon),
    SurfaceEvent(SurfaceEvent),
    SurfaceEffect(SurfaceEffect),
    Delivery(Delivery),
}

pub struct RejectedDocument {
    pub schema_version: Option<String>,
    pub payload_digest: Sha256Digest,
    pub bounded_payload: Vec<u8>,
    pub reason: RejectionReason,
}
```

`RejectedDocument::from_bytes` retains at most 64 KiB and always records the
full payload digest. Its `Debug` implementation prints byte count and digest,
never payload bytes.

Before the version probe, parse with a recursive map visitor that rejects a key
already present in the current object. This covers duplicate keys inside
`constraints` and adapter payloads, not only duplicate top-level fields.

- [ ] **Step 4: Run tests and privacy scan**

```bash
cargo test -p psyche-core --test decode
rg 'payload.*\\{:?\\}|from_utf8_lossy' crates/psyche-core/src
```

Expected: tests pass; the search returns no payload logging.

- [ ] **Step 5: Commit**

```bash
git add crates/psyche-core
git commit -m "feat(core): fail closed on unsupported records"
```

---

### Task 5: Add SQLite open and forward-only migrations

**Files:**
- Modify: `crates/psyche-store/Cargo.toml`
- Create: `crates/psyche-store/src/error.rs`
- Create: `crates/psyche-store/src/connection.rs`
- Create: `crates/psyche-store/src/migrations.rs`
- Create: `crates/psyche-store/migrations/001_foundation.sql`
- Modify: `crates/psyche-store/src/lib.rs`
- Create: `crates/psyche-store/tests/migrations.rs`

- [ ] **Step 1: Write failing fresh/open/unknown tests**

```rust
#[test]
fn fresh_store_applies_v1_once_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("psyche.sqlite3");
    drop(Store::open(&path).unwrap());
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 1);
}

#[test]
fn future_database_version_fails_before_any_migration() {
    let fixture = fixture_db("future-v99.sqlite3");
    let error = Store::open(&fixture).unwrap_err();
    assert!(matches!(error, StoreError::UnsupportedDatabaseVersion { found: 99 }));
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --test migrations
```

Expected: compile failure because `Store::open` is absent.

- [ ] **Step 3: Add the exact migration**

`001_foundation.sql` creates:

```sql
CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
) STRICT;
CREATE TABLE canonical_records (
  kind TEXT NOT NULL,
  record_id TEXT NOT NULL,
  schema_version TEXT NOT NULL,
  digest TEXT NOT NULL,
  canonical_json BLOB NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (kind, record_id),
  UNIQUE (kind, record_id, digest)
) STRICT;
CREATE TABLE transitions (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  record_id TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT NOT NULL,
  record_version INTEGER NOT NULL,
  transition_digest TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (kind, record_id, record_version)
) STRICT;
CREATE TABLE quarantine_records (
  quarantine_id TEXT PRIMARY KEY,
  schema_version TEXT,
  payload_digest TEXT NOT NULL,
  bounded_payload BLOB NOT NULL,
  reason TEXT NOT NULL,
  discovered_at TEXT NOT NULL,
  resolved_at TEXT
) STRICT;
CREATE TABLE audit_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_code TEXT NOT NULL,
  correlation_id TEXT NOT NULL,
  public_details_json BLOB NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
```

The generic tables are the W2 storage substrate, not a replacement for the
domain indexes listed in `TECH.md`. W3-W9 add indexed projections in their own
forward-only migrations.

- [ ] **Step 4: Configure every connection**

`Store::open` creates the parent directory with mode `0700` on Unix, opens
SQLite, then executes:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA secure_delete = ON;
PRAGMA busy_timeout = 5000;
```

Acquire `BEGIN EXCLUSIVE`, read `PRAGMA user_version`, deny any version greater
than `CURRENT_DATABASE_VERSION`, apply missing migrations, set `user_version`,
and commit. Never migrate after an unknown version.

- [ ] **Step 5: Add migration fixtures**

Commit binary-free SQL fixture constructors in
`crates/psyche-store/tests/support/mod.rs`; tests create databases at runtime.
Cover version 0, version 1, version 99, and a partially applied v1 transaction.

- [ ] **Step 6: Run migration tests**

```bash
cargo test -p psyche-store --test migrations -- --nocapture
```

Expected: fresh/open/reopen are idempotent; v99 returns the stable error;
partial migration rolls back.

- [ ] **Step 7: Commit**

```bash
git add crates/psyche-store
git commit -m "feat(store): add forward-only foundation migration"
```

---

### Task 6: Persist immutable records and append-only transitions

**Files:**
- Create: `crates/psyche-store/src/records.rs`
- Create: `crates/psyche-store/src/transitions.rs`
- Create: `crates/psyche-store/tests/records.rs`
- Modify: `crates/psyche-store/src/lib.rs`

- [ ] **Step 1: Write failing immutability tests**

```rust
#[test]
fn same_id_same_digest_is_idempotent_but_changed_payload_conflicts() {
    let (mut store, _dir) = test_store();
    let intent = fixture_intent("Review A");
    store.insert(&intent).unwrap();
    store.insert(&intent).unwrap();
    let changed = fixture_intent_with_same_id("Review B");
    assert!(matches!(store.insert(&changed), Err(StoreError::RecordConflict { .. })));
}

#[test]
fn transition_versions_are_monotonic_and_append_only() {
    let (mut store, _dir) = test_store();
    store.append_transition(&transition(1, "draft", "admitted")).unwrap();
    assert!(matches!(
        store.append_transition(&transition(1, "admitted", "running")),
        Err(StoreError::TransitionConflict { .. })
    ));
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --test records
```

Expected: compile failure because record APIs are absent.

- [ ] **Step 3: Implement one-transaction insertion**

`insert` canonicalizes and digests the typed document inside the method. Use
`INSERT ... ON CONFLICT DO NOTHING`, then load the existing digest:

- equal digest: return `Ok(())`;
- different digest: return `StoreError::RecordConflict`;
- never update canonical bytes.

`ingest` is the byte boundary: it calls `decode_document`, invokes `insert` for
known records, and invokes `quarantine` for unknown majors or kinds. It returns
`IngestOutcome::Inserted`, `IngestOutcome::AlreadyPresent`, or
`IngestOutcome::Quarantined`; it never returns unknown bytes as dispatchable.

`append_transition` uses an immediate transaction, verifies the last version is
exactly `record_version - 1`, inserts one row, and commits. The store does not
decide whether `draft -> admitted` is valid; W4 owns that policy.

- [ ] **Step 4: Add property tests**

```rust
proptest::proptest! {
    #[test]
    fn reinsertion_never_changes_stored_bytes(outcome in "[a-zA-Z0-9 ]{1,80}") {
        let (mut store, _dir) = test_store();
        let intent = fixture_intent(&outcome);
        let id = intent.record_id().clone();
        let before = psyche_core::digest::canonical_bytes(&intent).unwrap();
        store.insert(&CanonicalDocument::Intent(intent.clone())).unwrap();
        store.insert(&CanonicalDocument::Intent(intent)).unwrap();
        let after = store.load_canonical_bytes(SchemaKind::Intent, &id).unwrap().unwrap();
        proptest::prop_assert_eq!(before, after);
    }
}
```

- [ ] **Step 5: Run record and property tests**

```bash
cargo test -p psyche-store --test records
```

Expected: all tests pass with no ignored cases.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-store
git commit -m "feat(store): enforce immutable records and transitions"
```

---

### Task 7: Add quarantine, retention, and checkpoint behavior

**Files:**
- Create: `crates/psyche-store/src/quarantine.rs`
- Create: `crates/psyche-store/src/retention.rs`
- Create: `crates/psyche-store/tests/retention.rs`
- Modify: `crates/psyche-store/src/lib.rs`

- [ ] **Step 1: Write failing quarantine and retention tests**

```rust
#[test]
fn unknown_major_is_quarantined_without_dispatchable_record() {
    let (mut store, _dir) = test_store();
    let rejected = RejectedDocument::from_bytes(br#"{"schema_version":"psyche.intent.v2"}"#).unwrap();
    let id = store.quarantine(rejected).unwrap();
    assert!(store.quarantine_record(&id).unwrap().is_some());
    assert_eq!(store.count_records(SchemaKind::Intent).unwrap(), 0);
}

#[test]
fn pruning_preserves_unresolved_quarantine_and_latest_transition() {
    let (mut store, _dir) = old_fixture_store();
    let report = store.prune(time::OffsetDateTime::now_utc()).unwrap();
    assert_eq!(report.unresolved_quarantine_deleted, 0);
    assert!(store.latest_transition(&fixture_graph_id()).unwrap().is_some());
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --test retention
```

Expected: compile failure because quarantine and prune APIs are absent.

- [ ] **Step 3: Implement bounded quarantine**

`quarantine` inserts the schema string, full digest, at most 64 KiB of payload,
stable reason code, and discovery time. Repeated identical digest/reason pairs
return the existing `QuarantineId`. `Debug` and errors never render bytes.

- [ ] **Step 4: Implement conservative retention**

`prune` may delete only:

- resolved quarantine rows older than the cutoff;
- audit rows older than the cutoff when no unresolved record references their
  correlation ID;
- non-latest transitions only when a later transition for the same record is
  retained.

Return counts in `PruneReport`; do not vacuum automatically.

- [ ] **Step 5: Run retention tests**

```bash
cargo test -p psyche-store --test retention
```

Expected: unresolved and newest state survive; eligible old rows are removed.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-store
git commit -m "feat(store): add quarantine and safe retention"
```

---

### Task 8: Define behavior-level ports and deterministic fakes

**Files:**
- Create: `crates/psyche-coven/src/port.rs`
- Create: `crates/psyche-coven/src/error.rs`
- Modify: `crates/psyche-coven/src/lib.rs`
- Create: `crates/psyche-surfaces/src/port.rs`
- Create: `crates/psyche-surfaces/src/error.rs`
- Modify: `crates/psyche-surfaces/src/lib.rs`
- Create: `crates/psyche-test-support/src/coven.rs`
- Create: `crates/psyche-test-support/src/surface.rs`
- Modify: `crates/psyche-test-support/src/lib.rs`
- Create: `crates/psyche-test-support/tests/fakes.rs`

- [ ] **Step 1: Write failing fake honesty tests**

```rust
#[tokio::test]
async fn advertised_adoption_requires_a_scripted_adoption_step() {
    let fake = FakeCoven::builder()
        .capability(Capability::StableAdoption)
        .build();
    assert!(matches!(fake, Err(FakeBuildError::UnscriptedCapability { .. })));
}

#[tokio::test]
async fn unknown_contract_fails_before_adoption() {
    let fake = FakeCoven::builder()
        .contract("coven.daemon.v1")
        .adoption(AdoptionDisposition::Adopted { session_id: "session-1".into() })
        .build()
        .unwrap();
    let result = fake.negotiate(NegotiateRequest::new("coven.daemon.v2")).await;
    assert!(matches!(result, Err(PortError::ContractUnsupported { .. })));
    assert_eq!(fake.calls(), vec![FakeCall::Negotiate]);
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-test-support --test fakes
```

Expected: compile failure because port and fake types are absent.

- [ ] **Step 3: Implement behavior-level port types**

Use the public boundaries at the top of this plan. Define typed dispositions:

```rust
pub struct NegotiateRequest {
    pub required_api_version: String,
    pub required_capabilities: std::collections::BTreeSet<String>,
}

pub struct CapabilityProfile {
    pub api_version: String,
    pub capabilities: std::collections::BTreeSet<String>,
}

pub struct AdoptionRequest {
    pub request_id: RequestId,
    pub request_digest: Sha256Digest,
    pub familiar_snapshot_id: RecordId,
    pub graph_id: RecordId,
    pub node_id: RecordId,
    pub attempt_id: RecordId,
    pub project_id: String,
    pub operation: ExecutionOperation,
}

pub enum ExecutionOperation {
    CreateSession { canonical_cwd: String, harness: String },
    Input { session_id: String, input_digest: Sha256Digest },
}

pub enum AdoptionDisposition {
    Adopted { session_id: String },
    ProvenNotAdopted,
    Unknown,
    Fenced { fence_token: String },
}

pub struct SessionSnapshot {
    pub session_id: String,
    pub familiar_snapshot_id: RecordId,
    pub project_id: String,
    pub attempt_id: RecordId,
    pub terminal_state: Option<String>,
}

pub struct EventCursor {
    pub session_id: String,
    pub after_sequence: u64,
}

pub struct EventPage {
    pub events: Vec<CovenEvent>,
    pub next_cursor: EventCursor,
}

pub struct CovenEvent {
    pub sequence: u64,
    pub event_digest: Sha256Digest,
    pub terminal_state: Option<String>,
}

pub struct ResultBundle {
    pub session_id: String,
    pub attempt_id: RecordId,
    pub result_digest: Sha256Digest,
    pub artifacts: Vec<ArtifactReference>,
}

pub struct ArtifactReference {
    pub artifact_id: String,
    pub digest: Sha256Digest,
    pub media_type: String,
    pub size: u64,
}

pub struct TerminationRequest {
    pub request_id: RequestId,
    pub session_id: String,
    pub reason_code: String,
}

pub enum TerminationDisposition {
    Acknowledged { terminal_state: String },
    Unknown,
}

pub struct SurfaceAcceptance {
    pub surface_event_id: RecordId,
    pub accepted: bool,
}

pub enum DeliveryDisposition {
    Applied { external_id: String },
    Rejected { code: String },
    Unknown,
}
```

These types represent required behavior without claiming current Coven or
surface support. `RequestId` is a validated `req_` ID newtype. Fields bind the
C-S1-C-S12 assertions without naming transport endpoints; W5 maps the
W1-classified current Coven profile into these types rather than changing the
suite.

- [ ] **Step 4: Implement scripted fakes**

Builders accept a `VecDeque<ScriptStep>`. Each call consumes exactly one
matching step or returns `FakeError::UnexpectedCall`. Provide fault steps:
`Return`, `Error`, `DisconnectBeforeCommit`, `DisconnectAfterCommit`, and
`Stall`. Record calls using redacted typed enums, not raw request debug output.

- [ ] **Step 5: Run fake tests**

```bash
cargo test -p psyche-test-support --test fakes
```

Expected: contract mismatch stops before mutation; scripts are consumed in
order; unscripted advertised capabilities fail construction.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-coven crates/psyche-surfaces crates/psyche-test-support
git commit -m "feat(test): add honest scripted Psyche ports"
```

---

### Task 9: Add reusable conformance and persistence-model suites

**Files:**
- Create: `crates/psyche-test-support/src/suites/mod.rs`
- Create: `crates/psyche-test-support/src/suites/coven.rs`
- Create: `crates/psyche-test-support/src/suites/surface.rs`
- Create: `crates/psyche-test-support/tests/state_machine.rs`

- [ ] **Step 1: Write failing persistence state-machine properties**

```rust
proptest::proptest! {
    #[test]
    fn model_and_store_agree_after_any_foundation_operation_sequence(
        operations in proptest::collection::vec(any::<FoundationOperation>(), 1..64)
    ) {
        let (mut store, _dir) = test_store();
        let mut model = FoundationModel::default();
        for operation in operations {
            let expected = model.apply(operation.clone());
            let actual = apply_to_store(&mut store, operation);
            proptest::prop_assert_eq!(expected, actual);
        }
    }
}

proptest::proptest! {
    #[test]
    fn unknown_schema_operations_never_create_dispatchable_records(
        payload in proptest::collection::vec(any::<u8>(), 0..8192)
    ) {
        let (mut store, _dir) = test_store();
        quarantine_as_unknown_major(&mut store, payload).unwrap();
        proptest::prop_assert_eq!(store.total_record_count().unwrap(), 0);
    }
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-test-support --test state_machine
```

Expected: compile failure because `FoundationOperation` and its model are absent.

- [ ] **Step 3: Implement a policy-free reference model**

Define generated operations for insert, identical reinsert, conflicting
reinsert, append-next-transition, append-duplicate-version, quarantine,
resolve-quarantine, prune, checkpoint, and reopen. The reference model tracks
only durable keys, digests, transition versions, and quarantine resolution.
It must not encode graph/node transition legality, admission, delegation,
budget, verification, delivery, or recovery policy.

- [ ] **Step 4: Add reusable async suite entry points**

```rust
pub async fn assert_c_s1_contract_negotiation(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s2_session_lifecycle(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s3_snapshot_attempt_binding(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s4_stable_adoption(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s5_non_adoption_proof(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s6_ambiguity_fence(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s7_ordered_cursor(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s8_terminal_authority(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s9_cancellation_acknowledgement(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s10_result_artifact_binding(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s11_restart_persistence(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_c_s12_structured_denial(fixture: &mut dyn CovenConformanceFixture);
pub async fn assert_surface_unknown_delivery(port: &dyn SurfacePort);
```

Define a test-only `CovenConformanceFixture` with `port()` and `restart()`;
the C-S functions take `&mut dyn CovenConformanceFixture`, not a fake concrete
type. Each function consumes only public behavior types and runs against the
scripted fixture now. Together they cover every fault point in
`COVEN_PREREQUISITES.md`: before/after request adoption, input adoption,
lookup, cursor consumption, cancellation acknowledgement, terminal/result/
artifact persistence, and ambiguity fencing. W5 must call these same functions
against its real adapter; it may not copy, skip, or relax assertions. The
surface function establishes the shared fake boundary but does not claim G8.

- [ ] **Step 5: Run property tests with deterministic seeds**

```bash
PROPTEST_CASES=2048 PROPTEST_RNG_SEED=00000000000000000000000000000000 \
  cargo test -p psyche-test-support --test state_machine -- --nocapture
```

Expected: all 2,048 cases pass; failures print a reproducible seed.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-test-support
git commit -m "test(g2): add reusable state-machine suites"
```

---

### Task 10: Prove migration and crash atomicity

**Files:**
- Create: `crates/psyche-test-support/src/bin/crash_writer.rs`
- Create: `crates/psyche-store/tests/crash.rs`
- Create: `crates/psyche-store/tests/fixtures/v1.sql`

- [ ] **Step 1: Write failing crash test**

```rust
#[test]
fn killed_writer_exposes_only_committed_state_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(assert_cmd::cargo::cargo_bin!("crash_writer"))
        .arg(dir.path())
        .arg("exit-before-commit")
        .status()
        .unwrap();
    assert!(!status.success());
    let store = Store::open(&dir.path().join("psyche.sqlite3")).unwrap();
    assert_eq!(store.count_records(SchemaKind::Intent).unwrap(), 1);
    assert_eq!(store.count_transitions().unwrap(), 0);
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --test crash -- --nocapture
```

Expected: failure because `crash_writer` is absent.

- [ ] **Step 3: Implement deterministic crash points**

`crash_writer` accepts exactly:

- `exit-before-commit`;
- `exit-after-record-before-transition`;
- `exit-after-commit-before-checkpoint`;
- `exit-during-migration`.

It opens a real store, writes the committed baseline, begins the named
operation, then calls `std::process::abort()` at the declared point. Fault
selection exists only in the test-support binary; production store APIs contain
no test hooks.

- [ ] **Step 4: Add reopen assertions**

For every crash point, prove:

- SQLite integrity check returns `ok`;
- only committed rows are visible;
- migration version is either wholly old or wholly new;
- no same-ID/different-digest record exists;
- quarantine and latest transition invariants hold;
- reopening twice is idempotent.

- [ ] **Step 5: Run crash and migration suites repeatedly**

```bash
for run in 1 2 3 4 5; do
  cargo test -p psyche-store --test crash -- --nocapture || exit 1
done
cargo test -p psyche-store --test migrations -- --nocapture
```

Expected: all five runs and migration tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-store crates/psyche-test-support
git commit -m "test(g2): prove store crash and migration atomicity"
```

---

### Task 11: Wire the store into the runtime lifecycle

**Files:**
- Modify: `crates/psyche-runtime/Cargo.toml`
- Modify: `crates/psyche-runtime/src/lib.rs`
- Modify: `crates/psyche-runtime/tests/lifecycle.rs`

- [ ] **Step 1: Write failing startup and drain tests**

```rust
#[tokio::test]
async fn start_opens_the_store_and_shutdown_checkpoints_once() {
    let config = test_config();
    let runtime = Runtime::start(config.clone()).await.unwrap();
    assert!(config.data_dir.join("psyche.sqlite3").exists());
    runtime.shutdown().await.unwrap();
    let reopened = psyche_store::Store::open(&config.data_dir.join("psyche.sqlite3")).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 1);
}

#[tokio::test]
async fn future_database_version_blocks_running_state() {
    let config = future_version_fixture_config();
    let error = Runtime::start(config).await.unwrap_err();
    assert!(matches!(error, RuntimeError::Store(StoreError::UnsupportedDatabaseVersion { .. })));
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-runtime start_opens_the_store
```

Expected: failure because `Runtime` does not own a store.

- [ ] **Step 3: Add the store without weakening shutdown**

Add `store: std::sync::Mutex<psyche_store::Store>` to `Runtime`.
`Runtime::start` opens `config.data_dir.join("psyche.sqlite3")` before publishing
`LifecycleState::Running`. Add `RuntimeError::Store`.

At the drain seam, exactly the elected driver:

1. locks the store;
2. runs `checkpoint`;
3. releases the lock;
4. transitions to `Stopped`.

No lifecycle guard and store guard may be held simultaneously or across an
`.await`.

The standard mutex is limited to G2's synchronous open/checkpoint seams.
W3-W9 must route blocking SQLite work through `tokio::task::spawn_blocking` or
a dedicated store actor; they may not lock this mutex around async domain work.

- [ ] **Step 4: Run lifecycle and concurrency tests**

```bash
cargo test -p psyche-runtime
cargo clippy -p psyche-runtime --all-targets -- -D warnings
```

Expected: existing 24,000-attempt shutdown election still passes; checkpoint
runs once; unsupported DB never reaches `Running`.

- [ ] **Step 5: Commit**

```bash
git add crates/psyche-runtime
git commit -m "feat(runtime): bind lifecycle to durable store"
```

---

### Task 12: Add CI wiring and G2 documentation

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `docs/ARCHITECTURE.md`
- Create: `docs/SCHEMAS.md`
- Create: `docs/TESTING.md`
- Create: `docs/G2-EVIDENCE.md`

- [ ] **Step 1: Add a failing CI relationship check**

Create `scripts/check-g2-evidence.py` that fails unless:

- the workflow invokes workspace tests;
- `psyche-store/tests/migrations.rs` and `crash.rs` exist;
- the reusable suite functions exist;
- `docs/G2-EVIDENCE.md` names the current commit and every G2 criterion.

Run:

```bash
python3 scripts/check-g2-evidence.py
```

Expected: fail because `docs/G2-EVIDENCE.md` is absent.

- [ ] **Step 2: Wire exact CI commands**

The workflow must run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
PROPTEST_CASES=2048 PROPTEST_RNG_SEED=00000000000000000000000000000000 cargo test -p psyche-test-support --test state_machine
cargo test -p psyche-store --test migrations --test crash
cargo deny check licenses advisories bans sources
gitleaks detect --no-banner --redact --log-opts="--all"
python3 scripts/check-g2-evidence.py
```

No G2 test may be skipped or xfailed on the real target matrix. Platform-only
signal tests retain their existing justified skips.

- [ ] **Step 3: Document ownership and schemas**

`ARCHITECTURE.md` records:

```text
psyche-core <- psyche-config
psyche-core <- psyche-store
psyche-core <- psyche-coven
psyche-core <- psyche-surfaces
psyche-core + psyche-coven + psyche-surfaces + psyche-store <- psyche-test-support
psyche-config + psyche-store <- psyche-runtime <- psyche-cli
```

`SCHEMAS.md` lists every schema registry name, typed G2 records, strict
unknown-major behavior, canonical digest rules, and deferred policy owner.
`TESTING.md` explains fake scripts, deterministic seeds, and crash points.

- [ ] **Step 4: Write the evidence template with actual fields**

`G2-EVIDENCE.md` must contain:

```markdown
# G2 Contract Foundation Evidence

**Status:** candidate
**Psyche commit:** not recorded before remote review
**Plan:** `docs/superpowers/plans/2026-08-05-psyche-w2-g2-foundation.md`

| Criterion | Command | Result | Artifact |
|---|---|---|---|
| Unknown-version denial | `cargo test -p psyche-core --test decode` | not run remotely | none |
| Migrations | `cargo test -p psyche-store --test migrations` | not run remotely | none |
| State-machine/property | `cargo test -p psyche-test-support --test state_machine` | not run remotely | none |
| Crash/restart | `cargo test -p psyche-store --test crash` | not run remotely | none |
| Fake boundaries | `cargo test -p psyche-test-support --test fakes` | not run remotely | none |
```

The relationship check accepts this explicit candidate state. It accepts
`passed` only when the commit is a 40-character SHA and every artifact is an
HTTPS Actions run URL.

- [ ] **Step 5: Run the relationship check**

```bash
python3 scripts/check-g2-evidence.py
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/ci.yml scripts/check-g2-evidence.py docs
git commit -m "docs: wire Psyche G2 evidence"
```

---

### Task 13: Run the full gate and stop for review

**Files:**
- Modify: `docs/G2-EVIDENCE.md` only after remote CI produces URLs

- [ ] **Step 1: Run the complete local gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
PROPTEST_CASES=2048 PROPTEST_RNG_SEED=00000000000000000000000000000000 \
  cargo test -p psyche-test-support --test state_machine
cargo test -p psyche-store --test migrations --test crash
cargo deny check licenses advisories bans sources
gitleaks detect --no-banner --redact --log-opts="--all"
npm --prefix packages/psyche-npm test
npm pack ./packages/psyche-npm --dry-run
python3 scripts/check-g2-evidence.py
git diff --check
```

Expected: every command exits 0; no test is ignored; npm pack still contains
only the approved wrapper files.

- [ ] **Step 2: Request focused reviews**

Request one storage/crash review and one contract/ownership review. Both must
compare the implementation with `specs/psyche/PLAN.md`,
`RUNTIME_DESIGN.md`, `TECH.md`, and `COVEN_PREREQUISITES.md`.

- [ ] **Step 3: Push and open the PR**

```bash
issue_number=$(gh issue list --repo OpenCoven/psyche --state open \
  --search '"feat: complete Psyche W2 contract foundation" in:title' \
  --json number --limit 1 --jq '.[0].number')
git push -u origin feat/psyche-g2-foundation
gh pr create --repo OpenCoven/psyche \
  --base main \
  --head feat/psyche-g2-foundation \
  --title "feat: complete Psyche W2 contract foundation" \
  --body "Completes the approved W2/G2 foundation plan: typed canonical records, forward-only SQLite migrations, immutable storage, quarantine/retention, behavior-level fake ports, and migration/property/crash evidence. No W3-W9 behavior and no G4+ capability is enabled. Closes #${issue_number}."
```

Expected: a PR URL.

- [ ] **Step 4: Fill candidate evidence from the exact head**

After remote CI completes, update `docs/G2-EVIDENCE.md` with the immutable head
SHA and CI run URL, commit, push, and rerun CI. Evidence from an earlier head
does not count.

- [ ] **Step 5: Stop at G2 approval**

Do not merge and do not set `psyche.graphs.v1`. Report:

- PR URL and reviewed head SHA;
- local and remote gate results;
- all review-thread states;
- the filled G2 evidence matrix;
- any remaining uncertainty.

G2 passes only after a human approves the evidence and the reviewed PR is
squash-merged. W3/W4 planning begins after that terminal decision.

- [ ] **Step 6: Reconcile the claim after merge or stopped work**

Do not release while the PR remains open. After squash merge, or immediately
if implementation stops without an active PR, run from the implementation
worktree:

```bash
issue_number=$(gh issue list --repo OpenCoven/psyche --state all \
  --search '"feat: complete Psyche W2 contract foundation" in:title' \
  --json number --limit 1 --jq '.[0].number')
coven claim release "issue-${issue_number}"
```

Expected: the issue remains the durable coordination record and the shared
claim no longer blocks the next child plan.

## Final verification standard

The implementation is ready for G2 review only when:

1. every persisted known record has a strict v1 typed decoder;
2. every unknown major or enum value becomes a bounded quarantine record;
3. same-ID/different-digest insertion fails without mutation;
4. transitions are append-only and monotonically versioned;
5. migrations are forward-only, atomic, and unknown-future versions fail closed;
6. crash fixtures expose only committed state after reopen;
7. fake ports advertise only scripted behavior;
8. the reusable behavior suites have no fake-only relaxed assertions;
9. full local and remote gates pass at the exact reviewed SHA;
10. no W3-W9 policy or G4+ capability is enabled.
