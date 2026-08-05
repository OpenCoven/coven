# Psyche W2 G2 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Psyche W2 and produce terminal G2 evidence for canonical versioned records, durable SQLite migrations, behavior-level fake boundaries, and migration/state-machine/property/crash tests.

**Architecture:** Extend the merged four-crate bootstrap with the focused crates named by the canonical architecture. `psyche-core` owns policy-free identifiers, digests, schema names, and minimum record shapes; `psyche-store` owns SQLite, migrations, immutable records, append-only transitions, quarantine, retention, and its package-local crash helper; `psyche-coven` and `psyche-surfaces` own behavior-level traits for their respective boundaries; `psyche-test-support` owns deterministic scripted fakes and reusable conformance fixtures. `psyche-runtime` remains the only composition root. W3-W9 retain identity resolution, graph admission, orchestration policy, real Coven integration, Telegram behavior, verification, and add-on trust.

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
- `crates/psyche-core/src/contracts/surface.rs` - canonical surface event/effect envelopes and bounded adapter payloads.
- `crates/psyche-core/src/contracts/error.rs` - redacted `psyche.error.v1`.
- `crates/psyche-core/tests/contracts.rs` - explicit integration target for canonical contract fixtures and evidence commands.

### New `psyche-store` files

- `crates/psyche-store/src/transitions.rs` - store-owned, validated
  `Transition` wire type and append-only persistence.

### New crates

- `crates/psyche-store/` - migrations, connection, records, transitions, quarantine, retention.
- `crates/psyche-coven/` - behavior-level Coven trait and request/result types.
- `crates/psyche-surfaces/` - behavior-level surface trait and acceptance/delivery types.
- `crates/psyche-test-support/` - scripted fakes and reusable conformance fixtures.

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
impl CanonicalDocument {
    pub fn validate(&self) -> Result<(), ContractError>;
    pub fn persistable_record_id(&self) -> Option<&RecordId>;
}

// psyche-store
pub struct Store;
impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError>;
    pub fn schema_version(&self) -> Result<u32, StoreError>;
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<IngestOutcome, StoreError>;
    pub fn insert(&mut self, document: &CanonicalDocument) -> Result<(), StoreError>;
    pub fn append_transition(&mut self, transition: &Transition) -> Result<(), StoreError>;
    pub fn load(&self, kind: SchemaKind, id: &RecordId)
        -> Result<Option<CanonicalDocument>, StoreError>;
    pub fn load_canonical_bytes(&self, kind: SchemaKind, id: &RecordId)
        -> Result<Option<Vec<u8>>, StoreError>;
    pub fn quarantine(&mut self, rejected: RejectedDocument)
        -> Result<QuarantineId, StoreError>;
    pub fn resolve_quarantine(
        &mut self,
        id: &QuarantineId,
        resolution: &QuarantineResolution,
    ) -> Result<ResolveQuarantineOutcome, StoreError>;
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
    async fn reconcile(&self, request: ReconciliationRequest)
        -> Result<ReconciliationDisposition, PortError>;
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
`SchemaKind::Attempt`, `CanonicalDocument::Attempt`, or second record-kind
variant for bindings; `SchemaKind::ExecutionBinding` owns a persisted record
whose identity maps to the sole `RecordKind::Attempt`.

These traits name behavior, not Coven endpoints or Telegram methods. The fake
and future real adapters must run the same behavior-level assertions.

## Security and privacy failure cases

Every item below has a named automated test:

1. Unknown schema kinds, major versions, and typed enum values are quarantined before dispatch or state mutation.
2. Unknown fields are rejected unless that exact schema opts into additive fields.
3. Canonicalization rejects non-finite numbers and duplicate JSON keys.
4. Record digests exclude no mutable field: changing any serialized field changes the digest.
5. Typed direct inserts run the same schema and field-kind validation as decoded inserts.
6. Immutable records reject same-ID/different-digest reinsertion.
7. Transition rows validate their complete digest-bearing shape and append atomically; no API updates or deletes historical rows.
8. Raw rejected payloads are not written to logs or error messages.
9. Quarantine retains bounded bytes and a digest, not an unbounded diagnostic echo.
10. SQLite enables foreign keys, WAL, secure delete, and a busy timeout on every connection.
11. Unknown database schema versions fail before migrations or application reads.
12. A crash before commit exposes no partial record or transition after reopen.
13. Automated retention never deletes transition history, audit events, or unresolved quarantine.
14. Fake services never advertise behavior that their script cannot execute.
15. No fake result is described as current real-Coven conformance.
16. Delivery keeps the canonical `del_` prefix; delegation cannot claim it.
17. Quarantine resolution is validated, durable, idempotent, and conflict-safe.
18. Checkpoint failure still publishes `Stopped`, wakes every waiter, and returns the preserved error.
19. Every canonical v1 error code decodes strictly; unknown codes quarantine rather than aliasing a known code.
20. Ambiguous Coven adoption is durably returned or fenced through the reconciliation operation; faults never authorize redispatch.
21. Adoption request digests are recomputed from every canonical typed request field; callers cannot reuse a digest for changed work.
22. Cancellation completion requires typed O5 acknowledgement evidence; raw `killed`, `orphaned`, or other ledger status never suffices.

## Gate mapping

- **G2 Contract foundation:** this plan supplies canonical records, strict
  version denial, migrations, fake ports, state-machine/property tests, and
  crash/restart evidence.
- **G4 Single-node conformance:** not claimed. W5 must run the unchanged Coven
  behavior suite against a pinned real daemon.
- **G8 Adapter reliability:** not claimed. W6 supplies a real surface adapter
  and later runs the unchanged surface suite.

## Immutable Coven source snapshot

The implementation repository is `OpenCoven/psyche`, so relative paths in this
checkout are not evidence there. Normative specification review is pinned to
the immutable `OpenCoven/coven` commit
`42dcbc43-34cb48ec-af63efb5-50345e3e-ea2fb7ad` (display-grouped; remove the
hyphen for the 40-hex Git object ID):

| Document | Immutable URL | SHA-256 |
|---|---|---|
| PLAN | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/PLAN.md` | `sha256:01382f8a-0d2bca95-ddd53563-4dd6a9f0-9ac4a80d-588ccbeb-d72f163e-af56bc1e` |
| RUNTIME_DESIGN | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/RUNTIME_DESIGN.md` | `sha256:ab8c9222-14b8f117-9ebf71fb-8dfb55bd-6d0ff2d6-dfced455-1bf90503-767bb6b8` |
| TECH | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/TECH.md` | `sha256:1d00fb2b-725f384c-a027db60-d0afbd0a-62a7ec6c-7dcbb563-7bf14d30-d40e2e1c` |
| COVEN_PREREQUISITES | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/COVEN_PREREQUISITES.md` | `sha256:33994a28-921e70f8-24b0260c-e08231b2-117c5043-0c54e996-ed47582d-060e72f9` |
| COVEN_W1_AUDIT | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/COVEN_W1_AUDIT.md` | `sha256:eab9028b-f7ef9c8a-96d4c6be-d69e4ef0-b3497b47-0ca26589-cb3ffcd8-0677322d` |

The reviewed implementation-plan commit is recorded later because this file
must first merge or otherwise obtain an immutable GitHub commit. G2 evidence
must record that 40-character Coven plan commit, its immutable blob URL, and
the fetched plan file's SHA-256. It may not cite a branch URL, PR URL, local
relative path, or mutable `main`.

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

- [ ] **Step 5: Pin the approved Coven plan and specification sources**

Set `COVEN_PLAN_SHA` to the reviewed immutable `OpenCoven/coven` commit that
contains this plan, then verify it and the fixed specification snapshot through
the GitHub API before implementation:

```bash
: "${COVEN_PLAN_SHA:?set the reviewed OpenCoven/coven plan commit}"
test "${#COVEN_PLAN_SHA}" = 40
coven_plan_path="docs/superpowers/plans/"
coven_plan_path="${coven_plan_path}2026-08-05-psyche-w2-g2-foundation.md"
gh api "repos/OpenCoven/coven/contents/$coven_plan_path?ref=$COVEN_PLAN_SHA" \
  --jq .sha >/dev/null
coven_spec_sha="42dcbc43"
coven_spec_sha="${coven_spec_sha}34cb48ec"
coven_spec_sha="${coven_spec_sha}af63efb5"
coven_spec_sha="${coven_spec_sha}50345e3e"
coven_spec_sha="${coven_spec_sha}ea2fb7ad"
gh api "repos/OpenCoven/coven/commits/$coven_spec_sha" \
  --jq .sha | grep -Fx "$coven_spec_sha"
```

Record `COVEN_PLAN_SHA` in the implementation issue/readiness packet. All
reviews and evidence use immutable URLs built from these SHAs, never paths
relative to the Psyche checkout.

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

Verify root `[workspace.package] publish = false`. For `psyche-store`,
`psyche-coven`, and `psyche-surfaces`, inherit package metadata including
`publish.workspace = true`, workspace lints, and only the dependencies required
by the public boundary. `psyche-test-support` is the explicit exception: its
`[package]` table inherits version/edition/rust-version/license/repository but
contains only `publish = false` for publication policy and must not also set
`publish.workspace = true`.

```toml
# crates/psyche-coven/Cargo.toml (same publication pattern for store/surfaces)
[package]
name = "psyche-coven"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish.workspace = true

# crates/psyche-test-support/Cargo.toml
[package]
name = "psyche-test-support"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish = false
```

Add a manifest test or metadata assertion that
`cargo metadata --no-deps --format-version 1` reports
`psyche-test-support.publish == []` and that no manifest contains both
`publish.workspace` and `publish` in one `[package]` table.

- [ ] **Step 4: Add minimal compiling libraries**

```rust
// crates/psyche-store/src/lib.rs
//! Durable SQLite substrate for Psyche contracts.

// crates/psyche-coven/src/lib.rs
//! Behavior-level Coven execution boundary.

// crates/psyche-surfaces/src/lib.rs
//! Behavior-level surface acceptance and delivery boundary.

// crates/psyche-test-support/src/lib.rs
//! Deterministic fakes and reusable Psyche conformance fixtures.
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
fn delivery_keeps_the_canonical_del_prefix() {
    assert!(RecordId::parse(
        RecordKind::Delivery,
        "del_01J00000000000000000000000"
    ).is_ok());
    assert!(RecordId::parse(
        RecordKind::Delivery,
        "dly_01J00000000000000000000000"
    ).is_err());
}

#[test]
fn delegation_uses_the_distinct_dlg_prefix() {
    assert!(RecordId::parse(
        RecordKind::Delegation,
        "dlg_01J00000000000000000000000"
    ).is_ok());
    assert!(RecordId::parse(
        RecordKind::Delegation,
        "del_01J00000000000000000000000"
    ).is_err());
}

#[test]
fn execution_binding_uses_attempt_as_its_only_record_kind() {
    assert_eq!(
        SchemaKind::ExecutionBinding.record_kind(),
        Some(RecordKind::Attempt)
    );
    assert_eq!(RecordKind::Attempt.prefix(), "att_");
    assert_eq!(RecordKind::ALL.len(), 15);
    assert_eq!(
        RecordKind::ALL
            .iter()
            .filter(|kind| kind.prefix() == "att_")
            .copied()
            .collect::<Vec<_>>(),
        vec![RecordKind::Attempt]
    );
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
    Approval, Evidence, Verdict, Recovery, Addon, SurfaceEvent, SurfaceEffect,
    Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RecordId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
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

impl SchemaKind {
    pub const fn record_kind(self) -> Option<RecordKind> {
        Some(match self {
            Self::IdentitySnapshot => RecordKind::IdentitySnapshot,
            Self::Intent => RecordKind::Intent,
            Self::SurfaceEvent => RecordKind::SurfaceEvent,
            Self::Graph => RecordKind::Graph,
            Self::GraphNode => RecordKind::GraphNode,
            Self::Delegation => RecordKind::Delegation,
            Self::Budget => RecordKind::Budget,
            Self::Approval => RecordKind::Approval,
            Self::ExecutionBinding => RecordKind::Attempt,
            Self::Evidence => RecordKind::Evidence,
            Self::Verdict => RecordKind::Verdict,
            Self::Recovery => RecordKind::Recovery,
            Self::Addon => RecordKind::Addon,
            Self::SurfaceEffect => RecordKind::SurfaceEffect,
            Self::Delivery => RecordKind::Delivery,
            Self::Error => return None,
        })
    }
}

impl RecordKind {
    pub const ALL: [Self; 15] = [
        Self::IdentitySnapshot, Self::Intent, Self::Graph, Self::GraphNode,
        Self::Attempt, Self::Delegation, Self::Budget, Self::Approval,
        Self::Evidence, Self::Verdict, Self::Recovery, Self::Addon,
        Self::SurfaceEvent, Self::SurfaceEffect, Self::Delivery,
    ];

    pub const fn prefix(self) -> &'static str {
        match self {
            Self::IdentitySnapshot => "ids_",
            Self::Intent => "int_",
            Self::Graph => "grf_",
            Self::GraphNode => "nod_",
            Self::Attempt => "att_",
            Self::Delegation => "dlg_",
            Self::Budget => "bud_",
            Self::Approval => "apr_",
            Self::Evidence => "evd_",
            Self::Verdict => "vrd_",
            Self::Recovery => "rcv_",
            Self::Addon => "adn_",
            Self::SurfaceEvent => "sev_",
            Self::SurfaceEffect => "sfx_",
            Self::Delivery => "del_",
        }
    }
}
```

`RecordId::parse` must enforce this exact prefix registry and parse the suffix
as a ULID:

| Kind | Prefix |
|---|---|
| Identity snapshot | `ids_` |
| Intent | `int_` |
| Graph | `grf_` |
| Graph node | `nod_` |
| Attempt/execution binding | `att_` |
| Delegation | `dlg_` |
| Budget | `bud_` |
| Approval | `apr_` |
| Evidence | `evd_` |
| Verdict | `vrd_` |
| Recovery | `rcv_` |
| Add-on | `adn_` |
| Surface event | `sev_` |
| Surface effect | `sfx_` |
| Delivery | `del_` |

The separate `RequestId` newtype enforces `req_`; it is not a
`RecordKind`.
The store-owned `QuarantineId` is a separate validated newtype, not a
`RecordId`. `QuarantineId::new()` emits `qua_` plus a canonical uppercase ULID;
`parse`/`TryFrom<String>` require exactly that prefix and a 26-character ULID,
and serde always validates through `TryFrom`. Add accept/reject and serde
round-trip tests, including rejection of `sev_`, bare ULIDs, lowercase or
non-canonical ULIDs, and trailing data.
`del_` is the authoritative delivery prefix from the
`psyche.delivery.v1` example in `specs/psyche/TECH.md`. That specification has
no concrete delegation-ID example, so W2 derives the distinct mnemonic `dlg_`
for `RecordKind::Delegation`; `dly_` is not accepted for any kind. Add tests
proving delivery accepts `del_` and rejects `dly_`, while delegation accepts
`dlg_` and rejects `del_`.
`canonical_bytes` must call `serde_json_canonicalizer::to_vec`; `digest` must
return the `sha256:` prefix followed by exactly 64 lowercase hexadecimal
characters using `sha2::Sha256`.

`TryFrom<String>` validates every deserialization path for IDs, digests, and
schema versions. Each record's `validate()` additionally checks its exact
schema kind/major and that every ID field uses the field-specific `RecordKind`.
`SchemaKind::record_kind()` is the single mapping used by record and transition
validation; it returns `None` for `Error` and maps `ExecutionBinding` to
`RecordKind::Attempt`. `RecordKind` intentionally has no `ExecutionBinding`
variant: `Attempt` is the sole identity kind and `att_` the sole prefix for an
execution-binding record. `RecordKind::ALL`, `prefix()`, `RecordId::parse`, and
every exhaustive `match RecordKind` use exactly the same 15 variants; adding a
second execution-binding arm or alias is a contract failure.
`CanonicalDocument::validate()` dispatches to the contained type, and both
`decode_document` and `Store::insert` call it. A caller cannot bypass schema or
record-kind validation by constructing a typed value directly.

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
- Create: `crates/psyche-core/tests/contracts.rs`
- Create: `crates/psyche-core/tests/fixtures/surface-event.json`
- Create: `crates/psyche-core/tests/fixtures/surface-effect.json`
- Create: `crates/psyche-core/tests/fixtures/delivery-ready.json`
- Create: `crates/psyche-core/tests/fixtures/error-codes-v1.json`
- Create: `crates/psyche-core/tests/fixtures/intent-local.json`
- Create: `crates/psyche-core/tests/fixtures/node-root.json`

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
    assert!(decode_document(include_bytes!("fixtures/intent-local.json")).is_ok());
    assert!(decode_document(include_bytes!("fixtures/node-root.json")).is_ok());
}
```

These paths resolve from `crates/psyche-core/tests/contracts.rs`. Create both
files at those exact locations; do not reference a workspace-level
`tests/fixtures` directory. `intent-local.json` is a complete valid
`psyche.intent.v1` fixture with `surface_event_id: null`.
`node-root.json` is a complete valid `psyche.graph_node.v1` fixture with
`delegation_id: null`, non-null graph/familiar/budget IDs, an empty dependency
list, at least one required-evidence entry, `state: "ready"`, and `version: 1`.
Use full valid ULIDs and 64-hex digests. Add a fixture-existence/parse check to
`scripts/check-g2-evidence.py`, and assert the only null bindings are the two
documented fields.

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

Define the W0 `GraphState`, `NodeState`, and `AdoptionState` spellings exactly
from the lifecycle vocabulary in `TECH.md`; do not add transition methods.
TECH does not define a `CancellationState` enum, so do not attribute one to it.
G2 owns this complete local persistence vocabulary:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationState {
    NotRequested,
    TerminationRequested,
    AcknowledgedTerminated,
    AcknowledgedAlreadyTerminal,
    TerminationUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationAcknowledgementKind {
    Terminated,
    AlreadyAuthoritativelyTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "CancellationAcknowledgementEvidenceWire")]
pub struct CancellationAcknowledgementEvidence {
    pub acknowledgement_id: String,
    pub termination_request_id: RequestId,
    pub session_id: String,
    pub execution_request_id: RequestId,
    pub execution_request_digest: Sha256Digest,
    pub kind: CancellationAcknowledgementKind,
    pub authority_evidence_digest: Sha256Digest,
    #[serde(with = "time::serde::rfc3339")]
    pub acknowledged_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "CancellationUnresolvedEvidenceWire")]
pub struct CancellationUnresolvedEvidence {
    pub disposition_id: String,
    pub termination_request_id: RequestId,
    pub session_id: String,
    pub execution_request_id: RequestId,
    pub execution_request_digest: Sha256Digest,
    pub reason_code: String,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: time::OffsetDateTime,
}
```

`NotRequested` and `TerminationRequested` carry no terminal evidence.
`AcknowledgedTerminated` requires a validated O5
`CancellationAcknowledgementEvidence { kind: Terminated, .. }`;
`AcknowledgedAlreadyTerminal` requires
`AlreadyAuthoritativelyTerminal`; and `TerminationUnknown` requires the durable
`CancellationUnresolvedEvidence`. Raw Coven ledger strings, including
`killed` and `orphaned`, cannot select either acknowledged state.

This enum is G2-owned Psyche foundation vocabulary for
`psyche.execution_binding.v1`; it is not a claim that Coven's external O5
contract already exists or uses these wire spellings. W5 must map the
eventually approved O5 response into these states without weakening the
evidence rules. Unknown spellings remain typed-enum quarantine failures. Add
round-trip tests for all five values and negative tests for unknown values,
acknowledged states without matching evidence, mismatched acknowledgement
kinds, and raw-ledger promotion. Add `ExecutionBinding` with the exact fields
required by the W0 TECH description. Name the focused integration test
`cancellation_state_vocabulary_requires_matching_o5_evidence`.

`CancellationAcknowledgementEvidence`,
`CancellationAcknowledgementKind`, and `CancellationUnresolvedEvidence` are
owned by `psyche-core/src/contracts/execution.rs` and re-exported by
`psyche-core`; they do not import `psyche-coven`. `ExecutionBinding` contains:

```rust
pub cancellation_state: CancellationState,
pub cancellation_acknowledgement: Option<CancellationAcknowledgementEvidence>,
pub cancellation_unresolved: Option<CancellationUnresolvedEvidence>,
```

Validation enforces an exact one-of matrix: both evidence fields are absent for
`NotRequested`/`TerminationRequested`; acknowledgement alone is present with
the matching kind for either acknowledged state; unresolved alone is present
for `TerminationUnknown`. Every evidence reference must match the binding's
session ID, execution request ID/digest, and termination request ID, use
1..=255-byte UTF-8 acknowledgement/disposition/session IDs and a 1..=128-byte
lowercase ASCII reason code, and carry a valid SHA-256 authority digest and
RFC 3339 timestamp. The acknowledgement/unresolved timestamp cannot precede
the binding/request creation time or exceed its validity window. The
termination request ID must differ from the execution request ID. Constructors
validate these rules; serde uses `TryFrom` validation rather than deriving an
unchecked public wire path. This makes direct
`CanonicalDocument::ExecutionBinding` inserts enforceable entirely in
core/store without a reverse dependency.

The store tests mutate each evidence field independently: absent evidence,
wrong kind, acknowledgement plus unresolved evidence, wrong session,
termination/execution request ID or digest, reused request ID, invalid
authority digest, empty/oversized ID or reason, and timestamps before creation
or after lifetime. Every case must return
`ContractError::CancellationEvidenceMismatch` before a row or transition is
written.

The two private `*Wire` structs mirror the public fields and use
`#[serde(deny_unknown_fields)]`; their `TryFrom` implementations call the same
constructors/validators. They are module-private implementation details, so no
unchecked evidence struct crosses the `psyche-core` boundary.

Define the surface-neutral contracts in `contracts/surface.rs` with these exact
owned wire fields:

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEvent {
    pub schema_version: SchemaVersion,
    pub surface_event_id: RecordId,
    pub adapter_id: String,
    pub account_id: String,
    pub actor: serde_json::Value,
    pub locator: serde_json::Value,
    pub adapter_event_digest: Sha256Digest,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: time::OffsetDateTime,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEffect {
    pub schema_version: SchemaVersion,
    pub surface_effect_id: RecordId,
    pub intent_id: RecordId,
    pub graph_id: RecordId,
    pub node_id: RecordId,
    pub attempt_id: RecordId,
    pub familiar_snapshot_id: RecordId,
    pub project_id: String,
    pub action_class: String,
    pub account_id: String,
    pub locator: serde_json::Value,
    pub effect: serde_json::Value,
    pub effect_digest: Sha256Digest,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}
```

These are the W0 surface-neutral envelopes described by `TECH.md`: event actor,
locator, and content remain adapter-owned; effects preserve origin graph,
attempt, identity/project, target locator, class, and immutable adapter effect.
They do not duplicate Telegram's tagged unions or grant authority. Validation
requires exact v1 schema, `sev_`/`sfx_` and field-specific record-ID kinds,
non-empty `adapter_id`, `account_id`, `project_id`, and `action_class`, object
(not scalar/array/null) actor/locator/content/effect values, and an
`effect_digest` equal to the canonical `effect`. Each adapter-owned JSON value
must canonicalize independently and be at most `MAX_DOCUMENT_BYTES` (1 MiB);
the outer decoder's same 1 MiB limit remains authoritative. This is a storage
bound, not W6 Telegram policy. Add fixtures matching the TECH descriptions and
tests for every wrong ID kind, scalar payload, oversized payload, unknown
field, and effect-digest mismatch.
Name the positive integration test
`surface_event_and_effect_fixtures_round_trip`.

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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub schema_version: SchemaVersion,
    pub delivery_id: RecordId,
    pub intent_id: RecordId,
    pub action_class: String,
    pub account_id: String,
    pub chat_id: String,
    pub topic: DeliveryTopic,
    pub relationship: DeliveryRelationship,
    pub effect: serde_json::Value,
    pub effect_digest: Sha256Digest,
    pub surface_decision: DeliverySurfaceDecision,
    pub logical_response_id: String,
    pub logical_part: u32,
    pub state: DeliveryState,
    pub attempt_count: u32,
    pub telegram_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryTopic {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRelationship {
    ReplySameDm,
    ReplySameGroup,
    ReplySameTopic,
    CrossChat,
    Broadcast,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverySurfaceDecision {
    pub decision_id: String,
    pub request_digest: Sha256Digest,
    pub policy_revision: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: time::OffsetDateTime,
    pub state: DeliveryDecisionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryDecisionState { Reserved, Consumed }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Ready, Sending, Sent, Retryable, DeliveryUnknown, Failed, Abandoned,
    DeadLetter, ResolvingUnknown, Compensated,
}
```

This is the exact `psyche.delivery.v1` shape frozen at `TECH.md`'s Delivery
intent section; do not retain the incompatible `surface_effect_id`,
`surface_decision_digest`, or embedded `attempts` placeholder. Physical
attempts remain separate rows as required by TECH. Validation requires the
canonical `del_` and intent IDs, non-empty bounded strings (action class,
account, topic components, policy revision, and logical response ID at most
256 UTF-8 bytes; decimal chat/message IDs at most 32 bytes), `logical_part` and
`attempt_count` within SQLite `INTEGER` range, a non-empty object `effect`
whose canonical bytes fit `MAX_DOCUMENT_BYTES`, matching `effect_digest`, and
an RFC 3339 decision expiry. Whether that expiry is still current is checked
at W6 dispatch time, never during immutable decode/reload. Relationship and
state spellings are exactly the TECH lists above. `telegram_message_id` is absent
until the transport supplies a decimal ID and is required for `sent`.
G2 validates shape and immutable correlations but does not execute W6 delivery
transitions or Telegram policy.

Add `tests/fixtures/delivery-ready.json` by transcribing the canonical TECH
example with valid full ULIDs/digests. Strict tests round-trip that fixture,
reject each removed placeholder field, unknown relationship/state/decision
state, wrong delivery or intent prefix, scalar/oversized/mutated effect,
digest mismatch, malformed decision expiry, invalid decimal IDs, and
`sent` without `telegram_message_id`. Add a direct `Store::insert(
&CanonicalDocument::Delivery(delivery))` round-trip and a direct-insert
negative case for each correlation/digest class. Name the positive core and
store tests `delivery_v1_fixture_round_trips_canonically` and
`delivery_direct_insert_round_trips_canonically`.

Place all Task 3 integration tests in the explicit Cargo target
`crates/psyche-core/tests/contracts.rs`. It owns the named filters
`all_canonical_error_codes_decode`, `delivery_v1_*`, `surface_*`,
`delivery_keeps_the_canonical_del_prefix`, and
`delegation_uses_the_distinct_dlg_prefix`, and
`execution_binding_uses_attempt_as_its_only_record_kind`; evidence commands
must not rely on a nonexistent implicit test target.

The remaining foundation structs freeze names and wire types only.
W4/W6/W7/W9 add policy in forward-compatible child migrations; G2 does not
evaluate it.

- [ ] **Step 5: Add typed error envelopes**

Implement `ErrorEnvelope`, `ErrorBody`, and the complete canonical
`psyche.error.v1` code enum from `TECH.md`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ConfigInvalid,
    SecretUnavailable,
    TelegramUnauthorized,
    TelegramBotIdentityMismatch,
    TelegramConflict,
    TelegramRateLimited,
    TelegramUnavailable,
    WebhookAuthFailed,
    StorageUnavailable,
    EventSchemaUnsupported,
    PrincipalMappingInvalid,
    GraphInvalid,
    DelegationWidened,
    BudgetUnenforceable,
    EvidenceIncomplete,
    VerdictInvalid,
    RouteNotFound,
    RouteAmbiguous,
    SenderUnauthorized,
    IdentityInvalid,
    IdentityChanged,
    CovenUnavailable,
    CovenVersionUnsupported,
    CovenCapabilityMissing,
    CovenPolicyDenied,
    CovenExecutionBindingInvalid,
    CovenBindingMismatch,
    CovenArtifactRejected,
    CovenIntentConflict,
    CovenAdoptionUnknown,
    CovenCancellationUnknown,
    CovenSessionFailed,
    DeliveryUnknown,
    PreviewFinalizeBlocked,
    MediaRejected,
    CallbackInvalid,
}
```

`ErrorCode::ALL` lists every variant in the same order as the canonical table,
and `as_str()` is the single spelling source used by validation and fixtures.
Do not keep a smaller “G2-owned” allowlist: the schema registry recognizes the
whole v1 envelope, so every canonical v1 code, including
`coven_capability_missing`, must decode. Retryability is presentation metadata
from TECH, not a reason to omit a code or accept arbitrary strings.

`details` accepts only string-to-string public classifications; it must not
accept arbitrary JSON. `ErrorEnvelope::validate()` requires exactly
`psyche.error.v1`, a recognized code, a bounded public message, a bounded
correlation ID, and bounded string keys/values in `details`.
An unknown code is a typed enum failure and returns
`ContractError::UnknownEnumValue { schema: SchemaKind::Error, field: "code" }`,
so byte ingestion follows the same bounded quarantine route as other unknown
typed enum values.

Add `tests/fixtures/error-codes-v1.json`, containing one valid redacted envelope
for every `ErrorCode::ALL` spelling. A table-driven test decodes every fixture
entry as `CanonicalDocument::Error`, checks the exact enum variant and canonical
round-trip, and asserts the fixture set equals `ErrorCode::ALL` with no missing
or duplicate code; name it `all_canonical_error_codes_decode`. Dedicated tests cover `coven_capability_missing`,
`coven_adoption_unknown`, and `preview_finalize_blocked`, and reject unknown
fields, an unknown code, case changes, aliases, empty code, and non-string
detail values. The unknown-code byte-ingest test must assert a bounded
`RejectionReason::UnknownEnumValue` quarantine row and zero canonical records.
The error envelope is a decoded boundary message, not an immutable domain
record: it has no `RecordId`, does not implement `VersionedRecord`, and is not
persisted in `canonical_records`.

- [ ] **Step 6: Run contract and serialization tests**

```bash
cargo test -p psyche-core --test contracts
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
- Create: `crates/psyche-core/tests/fixtures/error-storage-unavailable.json`

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

#[test]
fn recognized_error_envelope_decodes_exhaustively() {
    let bytes = include_bytes!("fixtures/error-storage-unavailable.json");
    assert!(matches!(
        decode_document(bytes).unwrap(),
        CanonicalDocument::Error(_)
    ));
}

#[test]
fn error_envelope_rejects_non_string_details() {
    let bytes = br#"{
      "schema_version":"psyche.error.v1",
      "error":{
        "code":"storage_unavailable",
        "message":"Storage is unavailable.",
        "retryable":true,
        "correlation_id":"corr_01J00000000000000000000000",
        "details":{"attempt":1}
      }
    }"#;
    assert!(matches!(
        decode_document(bytes),
        Err(ContractError::InvalidShape { .. })
    ));
}

#[test]
fn error_envelope_rejects_unknown_fields() {
    let bytes = fixture_error_envelope_with_extra_field("internal_debug");
    assert!(matches!(
        decode_document(&bytes),
        Err(ContractError::InvalidShape { .. })
    ));
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
    Error(ErrorEnvelope),
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

Dispatch is exhaustive over every `SchemaKind`, including
`SchemaKind::Error -> CanonicalDocument::Error`. Add decode tests for a valid
`psyche.error.v1`, an error envelope with an unknown field, and an error
envelope whose `details` contains a non-string value. Recognized registry
entries may never fall through to an unknown or unsupported branch.

Before the version probe, parse with a recursive map visitor that rejects a key
already present in the current object. This covers duplicate keys inside
`constraints` and adapter payloads, not only duplicate top-level fields.

For every typed enum field, run the schema registry's field-specific enum
validator before typed deserialization. An unrecognized spelling returns
`ContractError::UnknownEnumValue { schema, field }`; the error is redacted and
does not echo the rejected value. Do not collapse this case into
`InvalidShape`, because `Store::ingest` must distinguish and quarantine it.

Add a test using a recognized schema and an unknown `GraphState` spelling:

```rust
#[test]
fn unknown_typed_enum_is_a_quarantinable_decode_failure() {
    let bytes = fixture_graph_bytes_with_state("future_state");
    assert!(matches!(
        decode_document(&bytes),
        Err(ContractError::UnknownEnumValue { field: "state", .. })
    ));
}
```

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
  resolved_at TEXT,
  resolution_code TEXT,
  resolution_digest TEXT,
  CHECK (
    (resolved_at IS NULL AND resolution_code IS NULL AND resolution_digest IS NULL)
    OR
    (resolved_at IS NOT NULL AND resolution_code IS NOT NULL AND resolution_digest IS NOT NULL)
  )
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

Factor each migration's SQL application into a package-private
`apply_migration_sql(transaction, version)` primitive. `Store::open` owns the
production transaction, version update, and commit directly; it accepts no
observer, callback, fault selector, or test parameter. Task 10 reuses only the
package-private SQL primitive from a feature-gated test driver.

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

**Owned transition contract (implemented in Step 3):**

In `transitions.rs`, define the exact public type accepted by
`Store::append_transition`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub kind: SchemaKind,
    pub record_id: RecordId,
    pub record_version: u64,
    pub from_state: Option<String>,
    pub to_state: String,
    pub transition_digest: Sha256Digest,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}
```

`Transition::new` accepts every field except `transition_digest`, canonicalizes
this digest input, and computes the digest:

```rust
#[derive(serde::Serialize)]
struct TransitionDigestInput<'a> {
    kind: SchemaKind,
    record_id: &'a RecordId,
    record_version: u64,
    from_state: &'a Option<String>,
    to_state: &'a str,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
}
```

`Transition::validate()` requires:

- a persistable `SchemaKind` and the corresponding field-specific
  `RecordKind` (`ExecutionBinding` maps to its `att_` attempt ID);
- `record_version >= 1`;
- `from_state == None` only for version 1, and a non-empty `from_state` for
  later versions;
- state names matching `[a-z][a-z0-9_]{0,63}`, with
  `from_state != to_state`;
- a `transition_digest` equal to the SHA-256 digest of the canonical
  `TransitionDigestInput`; and
- a UTC `created_at` serialized as RFC 3339.

The digest input intentionally omits `transition_digest` to avoid a
self-reference. These are storage-shape invariants only; W4 still owns which
named state transitions are legal.
`psyche-store/src/lib.rs` exposes the owned type with
`pub use transitions::Transition;`, making the fixed `Store` signature
self-contained.

- [ ] **Step 1: Write failing immutability tests**

```rust
#[test]
fn same_id_same_digest_is_idempotent_but_changed_payload_conflicts() {
    let (mut store, _dir) = test_store();
    let intent = fixture_intent("Review A");
    store.insert(&CanonicalDocument::Intent(intent.clone())).unwrap();
    store.insert(&CanonicalDocument::Intent(intent)).unwrap();
    let changed = fixture_intent_with_same_id("Review B");
    assert!(matches!(
        store.insert(&CanonicalDocument::Intent(changed)),
        Err(StoreError::RecordConflict { .. })
    ));
}

#[test]
fn direct_insert_rejects_wrong_field_id_kind_without_writing() {
    let (mut store, _dir) = test_store();
    let mut intent = fixture_intent("Review A");
    intent.intent_id =
        RecordId::parse(RecordKind::Graph, "grf_01J00000000000000000000000").unwrap();
    assert!(matches!(
        store.insert(&CanonicalDocument::Intent(intent)),
        Err(StoreError::Contract(ContractError::WrongRecordKind { .. }))
    ));
    assert_eq!(store.total_record_count().unwrap(), 0);
}

#[test]
fn direct_insert_rejects_wrong_schema_without_writing() {
    let (mut store, _dir) = test_store();
    let mut intent = fixture_intent("Review A");
    intent.schema_version = SchemaVersion::parse("psyche.graph.v1").unwrap();
    assert!(matches!(
        store.insert(&CanonicalDocument::Intent(intent)),
        Err(StoreError::Contract(ContractError::SchemaMismatch { .. }))
    ));
    assert_eq!(store.total_record_count().unwrap(), 0);
}

#[test]
fn direct_insert_rejects_non_persistable_error_envelope() {
    let (mut store, _dir) = test_store();
    assert!(matches!(
        store.insert(&CanonicalDocument::Error(fixture_error_envelope())),
        Err(StoreError::NonPersistableKind {
            kind: SchemaKind::Error
        })
    ));
    assert_eq!(store.total_record_count().unwrap(), 0);
}

#[test]
fn direct_insert_rejects_acknowledged_cancellation_without_evidence() {
    let (mut store, _dir) = test_store();
    let mut binding = fixture_execution_binding();
    binding.cancellation_state = CancellationState::AcknowledgedTerminated;
    binding.cancellation_acknowledgement = None;
    assert!(matches!(
        store.insert(&CanonicalDocument::ExecutionBinding(binding)),
        Err(StoreError::Contract(ContractError::CancellationEvidenceMismatch { .. }))
    ));
    assert_eq!(store.total_record_count().unwrap(), 0);
}

#[test]
fn direct_insert_rejects_mismatched_cancellation_evidence() {
    let (mut store, _dir) = test_store();
    let mut binding = fixture_acknowledged_execution_binding();
    binding
        .cancellation_acknowledgement
        .as_mut()
        .unwrap()
        .execution_request_digest = fixture_other_digest();
    assert!(matches!(
        store.insert(&CanonicalDocument::ExecutionBinding(binding)),
        Err(StoreError::Contract(ContractError::CancellationEvidenceMismatch { .. }))
    ));
    assert_eq!(store.total_record_count().unwrap(), 0);
}

#[test]
fn transition_versions_are_monotonic_and_append_only() {
    let (mut store, _dir) = test_store();
    store
        .append_transition(&transition(1, None, "admitted"))
        .unwrap();
    assert!(matches!(
        store.append_transition(&transition(1, None, "running")),
        Err(StoreError::TransitionConflict { .. })
    ));
}

#[test]
fn transition_validation_rejects_wrong_id_kind_and_digest_without_writing() {
    let (mut store, _dir) = test_store();
    let mut wrong_kind = transition(1, None, "admitted");
    wrong_kind.record_id =
        RecordId::parse(RecordKind::Intent, "int_01J00000000000000000000000").unwrap();
    assert!(matches!(
        store.append_transition(&wrong_kind),
        Err(StoreError::Contract(ContractError::WrongRecordKind { .. }))
    ));

    let mut wrong_digest = transition(1, None, "admitted");
    wrong_digest.transition_digest = fixture_digest('f');
    assert!(matches!(
        store.append_transition(&wrong_digest),
        Err(StoreError::Contract(ContractError::DigestMismatch { .. }))
    ));
    assert_eq!(store.count_transitions().unwrap(), 0);
}

#[test]
fn transition_append_requires_exact_version_and_prior_state() {
    let (mut store, _dir) = test_store();
    store
        .append_transition(&transition(1, None, "admitted"))
        .unwrap();
    assert!(matches!(
        store.append_transition(&transition(3, Some("admitted"), "running")),
        Err(StoreError::TransitionConflict { .. })
    ));
    assert!(matches!(
        store.append_transition(&transition(2, Some("draft"), "running")),
        Err(StoreError::TransitionConflict { .. })
    ));
    assert_eq!(store.count_transitions().unwrap(), 1);
}
```

`direct_insert_rejects_mismatched_cancellation_evidence` is table-driven over
the complete Task 3 evidence mutation list, not only the digest example shown
above. It also covers acknowledged states carrying unresolved evidence and
`TerminationUnknown` carrying acknowledgement evidence.

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --test records
```

Expected: compile failure because record APIs and the store-owned `Transition`
type are absent.

- [ ] **Step 3: Implement validated insertion and transitions**

Implement the `Transition` contract above, then make `insert` first call
`CanonicalDocument::validate()` before canonicalization,
hashing, opening a transaction, or executing SQL. It then requires
`persistable_record_id()`; `CanonicalDocument::Error` returns
`StoreError::NonPersistableKind { kind: SchemaKind::Error }` and writes
nothing. `ingest` returns the same explicit error for a recognized error
envelope rather than quarantining it. For persistable records, `insert`
canonicalizes and digests the typed document inside the method. Use `INSERT ...
ON CONFLICT DO NOTHING`, then load the existing digest:

Add `StoreError::Contract(ContractError)` for validation failures and
`StoreError::NonPersistableKind { kind: SchemaKind }` for recognized boundary
envelopes that have no storage identity.

- equal digest: return `Ok(())`;
- different digest: return `StoreError::RecordConflict`;
- never update canonical bytes.

`load` and `load_canonical_bytes` likewise reject `SchemaKind::Error` with
`StoreError::NonPersistableKind` before querying because the envelope has no
record ID.

`ingest` is the byte boundary: it calls `decode_document`, invokes `insert` for
known records, and invokes `quarantine` for unknown kinds, unknown majors, or
`ContractError::UnknownEnumValue`. The quarantine conversion uses
`RejectedDocument::from_decode_error(bytes, error)` so the stable reason is
preserved without logging payload bytes. Other malformed known-schema shapes
remain explicit ingest errors. `ingest` returns `IngestOutcome::Inserted`,
`IngestOutcome::AlreadyPresent`, or `IngestOutcome::Quarantined`; it never
returns unknown bytes as dispatchable.

`append_transition` calls `Transition::validate()` before opening a transaction.
It then uses an immediate transaction and loads the latest row for the exact
`(kind, record_id)` pair. With no prior row it requires version 1 and
allows either a validated initial `from_state` or `None`; otherwise it requires
`record_version == previous + 1` and
`from_state == Some(previous.to_state)`. Any existing version, skipped version,
prior-state mismatch, kind/ID mismatch, or digest mismatch fails without
inserting. It inserts the validated serialized fields once and commits. No
store API updates or deletes transition rows. The store does not decide whether
`admitted -> running` is policy-valid; W4 owns that policy.

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

`psyche-store/src/quarantine.rs` owns `QuarantineId`, its `qua_` constructor,
strict parser/serde validation, and the resolution types below.

**Owned quarantine resolution contract:**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct QuarantineId(String);

impl QuarantineId {
    pub fn new() -> Self;
    pub fn parse(value: &str) -> Result<Self, StoreError>;
    pub fn as_str(&self) -> &str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineResolutionCode {
    SchemaNowSupported,
    ConfirmedInvalid,
    DuplicatePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineResolution {
    pub code: QuarantineResolutionCode,
    #[serde(with = "time::serde::rfc3339")]
    pub resolved_at: time::OffsetDateTime,
}

pub enum ResolveQuarantineOutcome {
    Resolved { resolution_digest: Sha256Digest },
    AlreadyResolved { resolution_digest: Sha256Digest },
}
```

`psyche-store/src/lib.rs` re-exports these types with `QuarantineId`; no other
module defines or aliases the ID.

- [ ] **Step 1: Write failing quarantine and retention tests**

```rust
#[test]
fn unknown_major_is_quarantined_without_dispatchable_record() {
    let (mut store, _dir) = test_store();
    let outcome = store.ingest(br#"{"schema_version":"psyche.intent.v2"}"#).unwrap();
    assert!(matches!(outcome, IngestOutcome::Quarantined { .. }));
    assert_eq!(store.count_records(SchemaKind::Intent).unwrap(), 0);
}

#[test]
fn unknown_enum_is_quarantined_without_dispatchable_record() {
    let (mut store, _dir) = test_store();
    let outcome = store.ingest(&fixture_graph_bytes_with_state("future_state")).unwrap();
    let IngestOutcome::Quarantined { quarantine_id } = outcome else {
        panic!("unknown enum was not quarantined");
    };
    let rejected = store.quarantine_record(&quarantine_id).unwrap().unwrap();
    assert_eq!(rejected.reason, RejectionReason::UnknownEnumValue);
    assert_eq!(store.count_records(SchemaKind::Graph).unwrap(), 0);
}

#[test]
fn quarantine_resolution_is_durable_and_idempotent() {
    let (mut store, dir) = test_store();
    let id = quarantined_fixture(&mut store);
    let resolution = resolution(
        QuarantineResolutionCode::ConfirmedInvalid,
        "2026-08-05T00:01:00Z",
    );
    let first = store.resolve_quarantine(&id, &resolution).unwrap();
    let ResolveQuarantineOutcome::Resolved { resolution_digest } = first else {
        panic!("first resolution did not resolve");
    };
    assert!(matches!(
        store.resolve_quarantine(&id, &resolution).unwrap(),
        ResolveQuarantineOutcome::AlreadyResolved {
            resolution_digest: repeated
        } if repeated == resolution_digest
    ));
    drop(store);

    let reopened = Store::open(&dir.path().join("psyche.sqlite3")).unwrap();
    let persisted = reopened.quarantine_record(&id).unwrap().unwrap();
    assert_eq!(persisted.resolution_code, Some(QuarantineResolutionCode::ConfirmedInvalid));
    assert_eq!(persisted.resolution_digest, Some(resolution_digest));
}

#[test]
fn quarantine_resolution_rejects_unknown_stale_or_conflicting_requests() {
    let (mut store, _dir) = test_store();
    let unknown = fixture_quarantine_id();
    assert!(matches!(
        store.resolve_quarantine(&unknown, &valid_resolution()),
        Err(StoreError::QuarantineNotFound { .. })
    ));

    let id = quarantined_fixture(&mut store);
    assert!(matches!(
        store.resolve_quarantine(&id, &resolution(
            QuarantineResolutionCode::ConfirmedInvalid,
            "2026-08-04T23:59:00Z",
        )),
        Err(StoreError::InvalidQuarantineResolution { .. })
    ));

    store.resolve_quarantine(&id, &valid_resolution()).unwrap();
    assert!(matches!(
        store.resolve_quarantine(&id, &resolution(
            QuarantineResolutionCode::DuplicatePayload,
            "2026-08-05T00:02:00Z",
        )),
        Err(StoreError::QuarantineResolutionConflict { .. })
    ));
}

#[test]
fn pruning_preserves_unresolved_quarantine_and_all_transition_history() {
    let (mut store, _dir) = old_fixture_store();
    let transitions_before = store.transitions(&fixture_graph_id()).unwrap();
    let audit_before = store.audit_events().unwrap();
    let report = store.prune(time::OffsetDateTime::now_utc()).unwrap();
    assert!(report.resolved_quarantine_deleted > 0);
    assert_eq!(report.unresolved_quarantine_deleted, 0);
    assert_eq!(report.transitions_deleted, 0);
    assert_eq!(report.audit_events_deleted, 0);
    assert_eq!(store.transitions(&fixture_graph_id()).unwrap(), transitions_before);
    assert_eq!(store.audit_events().unwrap(), audit_before);
}
```

Also add `concurrent_quarantine_resolution_has_one_durable_winner`: open two
`Store` connections to the same fixture, release two threads simultaneously
with different valid resolutions, and assert exactly one `Resolved`, one
`QuarantineResolutionConflict`, one persisted resolution digest, and one
`quarantine_resolved` audit event after reopen.

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --test retention
```

Expected: compile failure because quarantine, resolution, and prune APIs are
absent.

- [ ] **Step 3: Implement bounded quarantine**

`quarantine` inserts the schema string, full digest, at most 64 KiB of payload,
stable reason code, and discovery time. Repeated identical digest/reason pairs
return the existing `QuarantineId`. `Debug` and errors never render bytes.
New rows obtain their ID only through `QuarantineId::new()`; callers cannot
inject an unvalidated text ID into SQL. Add constructor/parser/serde tests and
assert every persisted `quarantine_id` starts with `qua_` and reparses.
Name the manifest-listed test
`quarantine_id_constructor_parser_and_serde_round_trip`.

- [ ] **Step 4: Implement durable quarantine resolution**

`resolve_quarantine` validates the typed `QuarantineId` before SQL, then uses an
immediate transaction to load the row. Unknown IDs return
`StoreError::QuarantineNotFound`. `resolved_at` must be UTC and no earlier than
the row's `discovered_at`.

Compute `resolution_digest` over canonical JSON containing the quarantine ID,
payload digest, rejection reason, resolution code, and resolution timestamp.
For an unresolved row, atomically set `resolved_at`, `resolution_code`, and
`resolution_digest` with `WHERE resolved_at IS NULL`, and append one redacted
`quarantine_resolved` audit event in the same transaction. A replay with the
exact code, timestamp, and computed digest returns `AlreadyResolved` without a
second update or audit event. Any different resolution returns
`StoreError::QuarantineResolutionConflict` and preserves the first resolution.
Concurrent resolvers use the same compare-and-set plus reload rule, so exactly
one resolution wins. Resolution never deletes or redispatches payload bytes.
All resolution errors are stable and redacted; they may include the typed
quarantine ID and resolution digest, but never bounded payload bytes.
`old_fixture_store()` must create resolved rows through
`Store::resolve_quarantine`, not by setting `resolved_at` directly, so retention
tests exercise the all-or-none resolution columns and digest.

- [ ] **Step 5: Implement conservative retention**

`prune` may delete only:

- quarantine rows whose `resolved_at` is older than the cutoff and whose
  `resolution_code` and `resolution_digest` are both present.

Transition history is excluded from automated retention. `prune` executes no
`DELETE` against `transitions`, and `PruneReport::transitions_deleted` is always
zero. Audit events are also excluded because W2 stores public details as opaque
JSON and defines no indexed reference projection capable of proving that a
correlation is resolved. `prune` executes no `DELETE` against `audit_events`,
and `PruneReport::audit_events_deleted` is always zero. A later workstream may
add an explicit indexed reference table in a forward-only migration before
automating audit deletion; parsing opaque JSON is never a retention authority.
Any future transition or audit deletion requires that separately reviewed
projection/export contract and explicit evidence; it is not part of W2/G2.
Return counts in `PruneReport`; do not vacuum automatically.

- [ ] **Step 6: Run retention tests**

```bash
cargo test -p psyche-store --test retention
```

Expected: resolution is durable and idempotent; stale/unknown/conflicting
resolution fails without mutation; unresolved quarantine, every transition
row, and every audit event survive; only eligible fully resolved quarantine
rows are removed.

- [ ] **Step 7: Commit**

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
- Create: `crates/psyche-coven/tests/request_digest.rs`
- Create: `crates/psyche-coven/tests/fixtures/execution-request-launch.json`
- Create: `crates/psyche-coven/tests/fixtures/execution-request-input.json`
- Create: `crates/psyche-coven/tests/bindings.rs`
- Create: `crates/psyche-coven/tests/fixtures/result-bundle.json`

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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionRequestInput {
    Launch {
        schema_version: String,
        request_id: RequestId,
        graph_id: RecordId,
        node_id: RecordId,
        attempt_id: RecordId,
        principal_id: String,
        familiar_snapshot_id: RecordId,
        project_id: String,
        project_root: String,
        cwd: String,
        harness: String,
        context_manifest_digest: Sha256Digest,
        delegation_digest: Option<Sha256Digest>,
        budget_digest: Sha256Digest,
        required_artifact_bindings: Vec<ExecutionArtifactBinding>,
        payload_digest: Sha256Digest,
        #[serde(with = "time::serde::rfc3339")]
        created_at: time::OffsetDateTime,
        #[serde(with = "time::serde::rfc3339")]
        valid_until: time::OffsetDateTime,
    },
    Input {
        schema_version: String,
        request_id: RequestId,
        graph_id: RecordId,
        node_id: RecordId,
        attempt_id: RecordId,
        principal_id: String,
        familiar_snapshot_id: RecordId,
        project_id: String,
        session_id: String,
        input_digest: Sha256Digest,
        context_manifest_digest: Sha256Digest,
        required_artifact_bindings: Vec<ExecutionArtifactBinding>,
        payload_digest: Sha256Digest,
        #[serde(with = "time::serde::rfc3339")]
        created_at: time::OffsetDateTime,
        #[serde(with = "time::serde::rfc3339")]
        valid_until: time::OffsetDateTime,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionRequest {
    input: ExecutionRequestInput,
    request_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionArtifactBinding {
    pub artifact_id: String,
    pub digest: Sha256Digest,
    pub media_type: String,
    pub size: u64,
}

impl ExecutionRequestInput {
    pub fn validate(&self) -> Result<(), PortError>;
}

impl AdoptionRequest {
    pub fn new(input: ExecutionRequestInput) -> Result<Self, PortError>;
    pub fn input(&self) -> &ExecutionRequestInput;
    pub fn request_digest(&self) -> &Sha256Digest;
    pub fn correlation(&self) -> ExecutionCorrelation;
    pub fn recompute_digest(&self) -> Result<Sha256Digest, PortError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionCorrelation {
    pub request_id: RequestId,
    pub request_digest: Sha256Digest,
    pub familiar_snapshot_id: RecordId,
    pub project_id: String,
    pub graph_id: RecordId,
    pub node_id: RecordId,
    pub attempt_id: RecordId,
    pub created_at: time::OffsetDateTime,
    pub valid_until: time::OffsetDateTime,
}

pub enum AdoptionDisposition {
    Adopted { session_id: String },
    ProvenNotAdopted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationRequest {
    pub correlation: ExecutionCorrelation,
    pub ambiguity_digest: Sha256Digest,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationDisposition {
    Returned {
        disposition_id: String,
        session_id: String,
        correlation: ExecutionCorrelation,
        ambiguity_digest: Sha256Digest,
        recorded_at: time::OffsetDateTime,
    },
    Fenced {
        disposition_id: String,
        fence_token: String,
        correlation: ExecutionCorrelation,
        ambiguity_digest: Sha256Digest,
        recorded_at: time::OffsetDateTime,
    },
    Unresolved,
}

pub struct SessionSnapshot {
    pub session_id: String,
    pub correlation: ExecutionCorrelation,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "ContentAddressedReferenceWire")]
pub struct ContentAddressedReference {
    pub digest: Sha256Digest,
    pub media_type: String,
    pub size_bytes: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "ResultBundleWire")]
pub struct ResultBundle {
    pub session_id: String,
    pub correlation: ExecutionCorrelation,
    pub result: ContentAddressedReference,
    pub artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "ArtifactReferenceWire")]
pub struct ArtifactReference {
    pub artifact_id: String,
    pub session_id: String,
    pub correlation: ExecutionCorrelation,
    pub content: ContentAddressedReference,
}

pub struct TerminationRequest {
    pub request_id: RequestId,
    pub session_id: String,
    pub correlation: ExecutionCorrelation,
    pub reason_code: String,
    pub requested_at: time::OffsetDateTime,
}

pub enum TerminationDisposition {
    Acknowledged { evidence: CancellationAcknowledgementEvidence },
    Unresolved { evidence: CancellationUnresolvedEvidence },
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
surface support. `RequestId` is a validated `req_` ID newtype.
`ExecutionRequestInput` is the canonical digest input. It includes every typed
field represented by TECH's `psyche.execution_request.v1`: stable request,
graph/node/attempt, principal, familiar snapshot, project, launch/input
operation, project root/cwd/harness or adopted session, context and input/
payload digests, delegation/budget digests where applicable, required artifact
bindings, creation time, and the correlation lifetime required by C-S3/C-S10.
The serde-tagged operation serializes `operation` as exactly `launch` or
`input`; there is no caller-provided free-form operation string.

`AdoptionRequest::new` validates the input, canonicalizes the complete
`ExecutionRequestInput` with RFC 8785, and computes `request_digest`; callers
constructing a request through the API cannot pass or overwrite that field.
The serialized wire envelope necessarily carries the claimed digest, but it is
never trusted. `ExecutionCorrelation` is derived by `correlation()` from the
same request and digest rather than independently constructed. On `adopt`, the
`psyche-coven` adapter recomputes the digest from `input` before any network
call. The Coven-side owner recomputes it again from the received typed request
before uniqueness lookup or persistence, compares it in constant time, and
atomically persists the canonical request bytes and digest with adoption. A
digest mismatch returns
`PortError::RequestDigestMismatch` without lookup, adoption, or mutation.
Stable-ID replay compares the recomputed digest; same ID/same canonical bytes
returns one adoption, while same ID with any changed field returns
`PortError::IntentConflict`.

`ExecutionRequestInput::validate()` checks exact schema, every field-specific
ID kind, absolute/canonical project paths, supported harness spelling, bounded
strings and artifact lists, unique artifact IDs, artifact digest/type/size,
operation-specific required/forbidden fields, and that `valid_until` is later
than `created_at`; adoption separately rejects an already-expired request.
The exact schema string is `psyche.execution_request.v1`. This behavior request
is not added to the W0 `CanonicalDocument` registry and therefore uses a
validated string rather than the domain-record `SchemaVersion` newtype.
Every wire timestamp uses `#[serde(with = "time::serde::rfc3339")]`, so canonical
JSON contains RFC 3339 strings exactly as required by TECH, never the default
`OffsetDateTime` tuple/object representation. If a later compatible field is
nullable, it must use `time::serde::rfc3339::option`; nullable timestamps may
not silently switch encodings.
`SessionSnapshot`, `ResultBundle`, and every
`ArtifactReference` must echo the exact adoption correlation. Result and
artifact expiry may shorten but never extend `correlation.valid_until`; an
artifact must also echo its enclosing result's session ID and cannot outlive
the result.

`ContentAddressedReference` is the single canonical wire shape for both the
primary result and each artifact's bytes. Validation requires a valid SHA-256
digest, a lowercase ASCII media type with exactly one `/` and at most 255
bytes (both non-empty components use only
`[a-z0-9!#$&^_.+-]`; parameters/whitespace are forbidden),
`1 <= size_bytes <= i64::MAX`, and RFC 3339 expiry.
`ContentAddressedReference::for_bytes(media_type, bytes, expires_at)` computes
the digest/size, while `validate_payload(bytes)` compares both the exact byte
length and SHA-256 digest before consumption. The metadata-only `validate()`
cannot claim payload verification. Positive and negative tests exercise both
constructors, one-byte content mutation, a claimed-size mutation, and an
expired reference. These are reference/
SQLite bounds, not a promise that W5 will transfer arbitrarily large content;
the negotiated resource contract may impose a lower maximum. `ResultBundle`
requires a non-empty session ID, exact complete correlation, at most 1,024
artifact references, unique non-empty artifact IDs of at most 255 UTF-8 bytes,
and no artifact expiry after `result.expires_at` or
`correlation.valid_until`. Empty results use a real content digest/media type
with `size_bytes` equal to the canonical empty representation's byte length;
zero is never an “unknown size” sentinel.

The three private `*Wire` forms mirror their public fields and deny unknown
fields. Their `TryFrom` implementations recursively validate the content
reference, full correlation, session/artifact association, uniqueness, bounds,
and lifetime before returning any public typed value.

Add a strict canonical `tests/fixtures/result-bundle.json` containing one
primary `application/json` result and one `text/plain` artifact. In
`tests/bindings.rs`, name the positive test
`result_bundle_fixture_round_trips_complete_content_references`. Add negative
tests for missing/unknown result fields, wrong result digest/media type/size,
zero or oversized size, malformed media type, result expiry beyond correlation
lifetime, duplicate artifact ID, artifact session/correlation mismatch, and
artifact digest/media type/size/expiry disagreement. Collect the constructor,
payload, bounds, and expiry cases in the exact test
`content_reference_rejects_digest_size_media_type_and_lifetime_mismatch`.

The fixture's exact decoded values are:

```json
{"session_id":"session-1","correlation":{"execution_request_id":"req_01J00000000000000000000000","request_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","familiar_snapshot":"ids_01J00000000000000000000000","project_id":"project:sha256:abc","graph_id":"grf_01J00000000000000000000000","node_id":"nod_01J00000000000000000000000","attempt_id":"att_01J00000000000000000000000","valid_until":"2026-08-05T14:05:00Z"},"result":{"digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","media_type":"application/json","size_bytes":2,"expires_at":"2026-08-05T14:04:00Z"},"artifacts":[{"artifact_id":"artifact-1","session_id":"session-1","correlation":{"execution_request_id":"req_01J00000000000000000000000","request_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","familiar_snapshot":"ids_01J00000000000000000000000","project_id":"project:sha256:abc","graph_id":"grf_01J00000000000000000000000","node_id":"nod_01J00000000000000000000000","attempt_id":"att_01J00000000000000000000000","valid_until":"2026-08-05T14:05:00Z"},"content":{"digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","media_type":"text/plain","size_bytes":5,"expires_at":"2026-08-05T14:03:00Z"}}]}
```

The physical fixture is this single RFC 8785-canonical line with no trailing
newline; the test parses it, validates it, and requires reserialization to
produce identical bytes.

The fake's durable result ledger retains the complete result/artifact
references through the greater of their expiry, the graph recovery window, and
the adapter deduplication window. Expiry blocks content retrieval but does not
erase adoption/fence evidence. W2's generic Psyche store has no result-reference
table and `prune` therefore does not delete these port-owned fixtures; W5 must
add an indexed projection and explicit retention tests before production
deletion.

The C-S3 and C-S10 suites reject a one-field mismatch independently
for request digest, familiar snapshot, project, graph, node, attempt, session,
and lifetime. This makes complete association and mismatch rejection provable
without naming transport endpoints; W5 maps the W1-classified current Coven
profile into these types rather than changing the suite.

Add table-driven request-digest tests for both `Launch` and `Input`. Start from
one valid request, mutate each field independently (schema, request/graph/node/
attempt/familiar IDs, principal, project, every launch/input field, every
digest, each artifact ID/digest/type/size, created/valid times, and collection
order/content), retain the old digest, and require
`RequestDigestMismatch` plus zero persisted/adoption calls. Reconstructing
through `AdoptionRequest::new` must produce a different digest. Add same-ID/
same-request replay and same-ID/different-request conflict tests across restart.
These tests bind the idempotency key to the full typed operation instead of
trusting a digest supplied by a caller.

In `crates/psyche-coven/tests/request_digest.rs`, decode the two fixture files
with paths relative to that test source:

```rust
const LAUNCH_GOLDEN: &[u8] =
    include_bytes!("fixtures/execution-request-launch.json");
const INPUT_GOLDEN: &[u8] =
    include_bytes!("fixtures/execution-request-input.json");
```

Both fixture files are RFC 8785 canonical JSON with no trailing newline.
`canonical_bytes(&decoded)` must equal the original fixture bytes exactly,
including these timestamp members:

```json
"created_at":"2026-08-05T14:00:00Z","valid_until":"2026-08-05T14:05:00Z"
```

The input fixture uses `2026-08-05T14:01:00Z` and
`2026-08-05T14:06:00Z`. Hard-code and assert these complete golden digests:

`execution-request-launch.json` is exactly:

```json
{"attempt_id":"att_01J00000000000000000000000","budget_digest":"sha256:2222222222222222222222222222222222222222222222222222222222222222","context_manifest_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","created_at":"2026-08-05T14:00:00Z","cwd":"/workspace/project","delegation_digest":null,"familiar_snapshot_id":"ids_01J00000000000000000000000","graph_id":"grf_01J00000000000000000000000","harness":"codex","node_id":"nod_01J00000000000000000000000","operation":"launch","payload_digest":"sha256:4444444444444444444444444444444444444444444444444444444444444444","principal_id":"principal:val","project_id":"project:sha256:abc","project_root":"/workspace/project","request_id":"req_01J00000000000000000000000","required_artifact_bindings":[{"artifact_id":"artifact-1","digest":"sha256:3333333333333333333333333333333333333333333333333333333333333333","media_type":"text/plain","size":12}],"schema_version":"psyche.execution_request.v1","valid_until":"2026-08-05T14:05:00Z"}
```

`execution-request-input.json` is exactly:

```json
{"attempt_id":"att_01J00000000000000000000000","context_manifest_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","created_at":"2026-08-05T14:01:00Z","familiar_snapshot_id":"ids_01J00000000000000000000000","graph_id":"grf_01J00000000000000000000000","input_digest":"sha256:5555555555555555555555555555555555555555555555555555555555555555","node_id":"nod_01J00000000000000000000000","operation":"input","payload_digest":"sha256:6666666666666666666666666666666666666666666666666666666666666666","principal_id":"principal:val","project_id":"project:sha256:abc","request_id":"req_01J00000000000000000000000","required_artifact_bindings":[],"schema_version":"psyche.execution_request.v1","session_id":"session-1","valid_until":"2026-08-05T14:06:00Z"}
```

```text
launch sha256:75d651c5eb7f6e3ccd65631fce08afdcb8ac2a800bc0d8db55eaf9cf43519d04
input  sha256:c8c3d0cad99f65d0fdac7b2bb577cf1278412a7ea6255d443e45394109311c61
```

Add `execution_request_launch_matches_golden_bytes_and_digest` and
`execution_request_input_matches_golden_bytes_and_digest`; each also parses the
canonical bytes as `serde_json::Value` and asserts both timestamp values are
JSON strings. A negative serialization test using a deliberately unannotated
test-only struct proves its bytes differ, preventing accidental adapter
removal.

The core-owned `CancellationAcknowledgementEvidence` is the separate O5
authority evidence required by C-S9. `psyche-coven` imports it from
`psyche-core`; it does not define a parallel acknowledgement type.
The adapter maps the O5 response into the canonical evidence only after exact
session/request/correlation/digest validation, then persists the resulting
`ExecutionBinding`. An unresolved O5 response maps to the core-owned
`CancellationUnresolvedEvidence`. This dependency remains
`psyche-coven -> psyche-core`; core/store never depend on the adapter.

Acknowledgement evidence is not derived from `SessionSnapshot::terminal_state`,
`CovenEvent::terminal_state`, process exit, disconnect, or a raw persisted
session status. `Acknowledged` validates a non-empty opaque acknowledgement ID,
the exact termination request/session/execution correlation, a non-zero
authority-evidence digest, and an acknowledgement timestamp no earlier than
the request. `AlreadyAuthoritativelyTerminal` still requires this O5 evidence;
reading a terminal ledger string is insufficient. `Unresolved` is durable,
correlation-bound, restart-stable, and maps only to Psyche
`termination_unknown`.

The C-S9 suite must feed every raw O1 ledger status (`created`, `running`,
`idle`, `completed`, `failed`, `killed`, and `orphaned`) through snapshots and
events and prove none can construct or satisfy
`TerminationDisposition::Acknowledged`. In particular, accepted kill plus a
durable `killed` row and restart recovery to `orphaned` remain unresolved
without O5 evidence. Positive cases require the typed acknowledgement returned
by `terminate`, exact binding/digest validation, persistence across restart,
and idempotent replay. Mismatched request/session/correlation/digest/time,
silence, disconnect-before-acknowledgement, and a raw-status-only response all
produce or preserve `Unresolved`; no case promotes a ledger string to
`cancelled`.

`CovenPort::reconcile` is the executable C-S6 ambiguity-fencing boundary.
It carries the complete immutable `ExecutionCorrelation`, a digest of the
durable evidence that made adoption ambiguous, and a bounded stable reason.
The daemon-side behavior is idempotent on
`(request_id, request_digest, ambiguity_digest)`: identical replay returns the
same durable disposition ID and timestamp; any changed correlation field or
ambiguity digest returns `PortError::IntentConflict`.

`Returned` means the possibly adopted execution was authoritatively found and
echoes its session and complete correlation. `Fenced` means every resource
that could satisfy that exact correlation was fenced before the disposition
was committed. `Unresolved` is explicit and never grants redispatch. Returned
or fenced dispositions must survive restart and validate their exact
correlation, ambiguity digest, non-empty opaque disposition ID, timestamp, and
non-empty session/fence token. Psyche records the disposition before changing
the node from its ambiguity-blocked state. A returned session is resumed and
is never redispatched; only a validated durable `Fenced` disposition can make
a later W5 redispatch eligible. This plan supplies no local unblock and does
not claim that the current real Coven adapter implements the operation.

- [ ] **Step 4: Implement scripted fakes**

Builders accept a `VecDeque<ScriptStep>`. Each call consumes exactly one
matching step or returns `FakeError::UnexpectedCall`. Provide fault steps:
`Return`, `Error`, `DisconnectBeforeCommit`, `DisconnectAfterCommit`, and
`Stall`. For `reconcile`, the two disconnect steps occur immediately before or
after the fake's durable disposition write. After-commit replay returns the
same disposition; before-commit and stall leave `Unresolved`. Record calls
using redacted typed enums, not raw request debug output. The fake exposes
test-only durable-disposition and call counters only through the
adapter-neutral `CovenConformanceFixture::observations` contract, so the C-S6
suite proves restart persistence and absence of automatic redispatch without
fake-specific assertions or production introspection.

- [ ] **Step 5: Run fake tests**

```bash
cargo test -p psyche-test-support --test fakes
cargo test -p psyche-coven --test request_digest
cargo test -p psyche-coven --test bindings
```

Expected: contract mismatch stops before mutation; scripts are consumed in
order; unscripted advertised capabilities fail construction; changed request
fields with a retained digest never consume an adoption step; raw ledger
statuses never produce termination acknowledgement evidence; and all fixture
fault/reset/observation methods behave identically through the trait object.
Both RFC 3339 request fixtures retain their pinned bytes/digests; the complete
result fixture round-trips and every correlation, digest, media-type, size, and
lifetime mutation fails closed. O5 adapter output maps into the core-owned
evidence type; no adapter-owned acknowledgement type exists.

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
- Create: `crates/psyche-test-support/tests/conformance.rs`

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

proptest::proptest! {
    #[test]
    fn unknown_enum_operations_never_create_dispatchable_records(
        unknown_state in "future_[a-z]{1,24}"
    ) {
        let (mut store, _dir) = test_store();
        let outcome = store.ingest(&fixture_graph_bytes_with_state(&unknown_state)).unwrap();
        proptest::prop_assert!(matches!(outcome, IngestOutcome::Quarantined { .. }));
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
reinsert, invalid direct insert (schema or field-ID kind), append-next-transition,
append-duplicate-version, invalid transition digest/kind, quarantine,
resolve-quarantine, prune, checkpoint, and reopen. The reference model tracks
only durable keys, digests, transition versions, and quarantine resolution.
Every invalid direct insert and invalid transition leaves the model and store
unchanged.
Generated `resolve-quarantine` operations call the public
`Store::resolve_quarantine` API and model first resolution, identical replay,
unknown ID, stale timestamp, and conflicting replay outcomes explicitly.
Prune never removes transition versions or audit events from either model or
store.
It must not encode graph/node transition legality, admission, delegation,
budget, verification, delivery, or recovery policy.

Add a separate behavior model for C-S6; do not mix Coven authority into the
SQLite foundation model. Generate `CovenRecoveryOperation::{MarkAmbiguous,
Reconcile, DisconnectBeforeDisposition, DisconnectAfterDisposition, Restart,
AttemptRedispatch}` over one immutable `ExecutionCorrelation`. The model tracks
`Ambiguous`, `Returned`, and `Fenced` durable states plus adoption-call count.
The scripted fixture and model must agree after every operation: before-commit
faults/stalls remain ambiguous, after-commit replay returns the same disposition
after restart, returned correlation resumes the same session, changed
correlations conflict, and `AttemptRedispatch` is rejected in `Ambiguous` and
`Returned`. A fenced state may report `RedispatchEligible` to the model but the
G2 fake does not perform a second adoption. This property is the executable
operation-level support for C-S6, not a claim of real Coven conformance.

The behavior model also generates `ConstructRequest`, `ReplayRequest`,
`MutateRequestFieldRetainDigest`, and `Restart` across both launch and input
operations. It computes the expected RFC 8785 digest from the full
`ExecutionRequestInput`; stale/reused digests always fail before the adoption
counter or durable map changes, while identical canonical replay returns the
same adoption after restart.
Name the focused properties `c_s6_model_never_redispatches_without_fence` and
`request_digest_binds_every_typed_field`.

- [ ] **Step 4: Add reusable async suite entry points**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConformanceOutcome {
    Verified,
    ExpectedUnsupported { code: String },
}

pub async fn assert_c_s1_contract_negotiation(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s2_session_lifecycle(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s3_snapshot_attempt_binding(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s4_stable_adoption(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s5_non_adoption_proof(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s6_ambiguity_fence(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s7_ordered_cursor(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s8_terminal_authority(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s9_cancellation_acknowledgement(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s10_result_artifact_binding(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s11_restart_persistence(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_c_s12_structured_denial(fixture: &mut dyn CovenConformanceFixture) -> ConformanceOutcome;
pub async fn assert_surface_unknown_delivery(port: &dyn SurfacePort);
```

`tests/conformance.rs` supplies Tokio test wrappers with the exact manifest
names `c_s1_contract_negotiation`, `c_s2_session_lifecycle`,
`c_s3_snapshot_attempt_binding`, `c_s4_stable_adoption`,
`c_s5_non_adoption_proof`, `c_s6_ambiguity_fence`,
`c_s7_ordered_cursor`, `c_s8_terminal_authority`,
`c_s9_cancellation_acknowledgement`,
`c_s10_result_artifact_binding`, `c_s11_restart_persistence`, and
`c_s12_structured_denial`. Every wrapper constructs the scripted fixture,
calls its reusable `assert_*` function, and requires
`ConformanceOutcome::Verified`. No C-S function is library-only or unexecuted.
W5 adds real-adapter wrappers that call these same reusable functions; it does
not copy their assertions.

The wrapper file contains these explicit tests (a local helper may remove the
repeated assertion, but not hide or rename the test functions):

```rust
#[tokio::test] async fn c_s1_contract_negotiation() {
    assert_eq!(assert_c_s1_contract_negotiation(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s2_session_lifecycle() {
    assert_eq!(assert_c_s2_session_lifecycle(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s3_snapshot_attempt_binding() {
    assert_eq!(assert_c_s3_snapshot_attempt_binding(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s4_stable_adoption() {
    assert_eq!(assert_c_s4_stable_adoption(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s5_non_adoption_proof() {
    assert_eq!(assert_c_s5_non_adoption_proof(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s6_ambiguity_fence() {
    assert_eq!(assert_c_s6_ambiguity_fence(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s7_ordered_cursor() {
    assert_eq!(assert_c_s7_ordered_cursor(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s8_terminal_authority() {
    assert_eq!(assert_c_s8_terminal_authority(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s9_cancellation_acknowledgement() {
    assert_eq!(assert_c_s9_cancellation_acknowledgement(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s10_result_artifact_binding() {
    assert_eq!(assert_c_s10_result_artifact_binding(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s11_restart_persistence() {
    assert_eq!(assert_c_s11_restart_persistence(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
#[tokio::test] async fn c_s12_structured_denial() {
    assert_eq!(assert_c_s12_structured_denial(&mut scripted_fixture()).await, ConformanceOutcome::Verified);
}
```

Every function also handles a fixture-declared `ExpectedUnsupported` mode by
making the relevant public call and requiring the exact stable
`ContractUnsupported` or `CapabilityMissing` denial plus zero mutation/calls
beyond negotiation. It returns `ExpectedUnsupported { code }`; it never skips
the call or returns `Verified`. That diagnostic outcome documents the current
pre-G4 real-adapter gap but cannot satisfy a G2/G4 `passed` evidence row. The
G2 matrix below explicitly says these are scripted-boundary verifications, not
current real-Coven conformance.

Define this adapter-neutral test-only fixture contract:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CovenFaultPoint {
    AdoptionBeforeCommit,
    AdoptionAfterCommit,
    InputBeforeCommit,
    InputAfterCommit,
    LookupBeforeRead,
    LookupAfterRead,
    CursorBeforePage,
    CursorAfterPage,
    CancellationBeforeAcknowledgement,
    CancellationAfterAcknowledgement,
    TerminalBeforePersistence,
    ResultBeforePersistence,
    ArtifactBeforePersistence,
    ReconcileBeforeDisposition,
    ReconcileAfterDisposition,
    ReconcileStall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableDispositionKind {
    Returned { session_id: String },
    Fenced { fence_token: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableDispositionObservation {
    pub disposition_id: String,
    pub correlation: ExecutionCorrelation,
    pub ambiguity_digest: Sha256Digest,
    pub kind: DurableDispositionKind,
    pub recorded_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CovenConformanceObservations {
    pub adoption_calls: u64,
    pub reconciliation_calls: u64,
    pub durable_reconciliation: Option<DurableDispositionObservation>,
}

pub enum CovenConformanceCase {
    C_S1, C_S2, C_S3, C_S4, C_S5, C_S6,
    C_S7, C_S8, C_S9, C_S10, C_S11, C_S12,
}

pub enum FixtureAvailability {
    Supported,
    ExpectedUnsupported { code: String },
}

#[async_trait::async_trait]
pub trait CovenConformanceFixture {
    fn port(&self) -> &dyn CovenPort;
    fn availability(&self, case: CovenConformanceCase) -> FixtureAvailability;
    async fn restart(&mut self);
    async fn select_fault(&mut self, point: CovenFaultPoint);
    async fn clear_fault(&mut self);
    async fn reset(&mut self);
    async fn observations(&self) -> CovenConformanceObservations;
}
```

The C-S functions take `&mut dyn CovenConformanceFixture`, not a fake concrete
type. `availability`, `select_fault`, `clear_fault`, `reset`, and `observations` are mandatory
for every fixture implementation. The scripted fake maps them to script steps;
the W5 real-adapter fixture maps them to its isolated daemon/test-transport
fault controller and durable test query. They are never methods on production
`CovenPort` and compile only in conformance-test support. Observations expose
counts and typed durable disposition metadata only, never raw request payloads
or a fake implementation type.

The scripted G2 fixture reports `Supported` for all twelve cases. A future real
adapter reports `ExpectedUnsupported` only for a capability absent from its
negotiated contract. The generic function must still execute the public call,
verify the exact structured denial and zero mutation, and return the diagnostic
`ConformanceOutcome::ExpectedUnsupported`; a passed conformance row requires
`Verified`.

Each function consumes only public behavior types and runs against the
scripted fixture now. Together they cover every fault point in
`COVEN_PREREQUISITES.md`: before/after request adoption, input adoption,
lookup, cursor consumption, cancellation acknowledgement, terminal/result/
artifact persistence, and ambiguity fencing. W5 must call these same functions
against its real adapter; it may not copy, skip, or relax assertions. The
surface function establishes the shared fake boundary but does not claim G8.

The twelve functions have these mandatory executable assertions:

| Suite | Scripted-boundary behavior |
|---|---|
| C-S1 | negotiate the exact contract/capability set; reject unknown majors, missing capabilities, and a falsely advertised method before mutation |
| C-S2 | create, attach, observe, and close one session; reject invalid cwd, harness, and lifecycle order |
| C-S3 | round-trip a snapshot and run the complete independent correlation-mismatch matrix below |
| C-S4 | prove one stable adoption across lost replies/restart and reject every-field digest reuse |
| C-S5 | distinguish durable `Adopted`, `NotAdopted`, and `Unknown`; only the durable non-adoption proof permits the model's redispatch-eligible result |
| C-S6 | execute the return-or-fence sequence and all adapter-neutral fault/observation assertions below |
| C-S7 | page an ordered cursor without gaps/duplicates, persist it across restart, and reject regression or foreign-session cursors |
| C-S8 | accept only an authoritative durable terminal record; reject process exit, disconnect, and unpersisted terminal observations |
| C-S9 | require core-owned acknowledgement evidence and reject raw ledger states as detailed below |
| C-S10 | round-trip the complete result fixture and independently reject every correlation/content-reference mismatch below |
| C-S11 | restart after each adoption/cursor/cancellation/terminal/result/fence persistence fault and prove the same durable disposition with no duplicate |
| C-S12 | require stable structured denials for invalid contract, capability, correlation, cursor, lifecycle, and content-reference inputs; reject free-form-only errors |

`assert_c_s3_snapshot_attempt_binding` submits a valid
`ExecutionCorrelation`, verifies the exact echo in `SessionSnapshot`, and
independently mutates request digest, familiar snapshot, project, graph, node,
attempt, and lifetime to prove fail-closed rejection.
`assert_c_s4_stable_adoption` constructs requests only through
`AdoptionRequest::new`, verifies the adapter and authority recompute the
canonical full-request digest, and runs the every-field mutation/reused-digest
matrix above.
`assert_c_s10_result_artifact_binding` applies the same mismatch table to
`ResultBundle` and each `ArtifactReference`, and additionally rejects a wrong
session ID, `result.digest`/media type/size disagreement, zero or oversized
content size, malformed media type, result expiry beyond the
correlation lifetime, artifact expiry beyond either result or correlation
lifetime, duplicate artifact IDs, `artifact.content` disagreement, and
artifacts omitted from the complete result association. It round-trips the
strict `result-bundle.json` fixture before running each independent mutation.

`assert_c_s6_ambiguity_fence` must:

1. call `reset`, select `AdoptionAfterCommit`, adopt once with a complete
   immutable correlation, clear that fault, then select `LookupAfterRead` and
   verify the lost lookup response preserves explicit local `Unknown`;
2. clear the fault and call `reconcile` with that exact correlation and
   ambiguity digest;
3. exercise both legal terminal scripts: `Returned` with the original session,
   and `Fenced` with a non-empty token;
4. restart and replay the same request, proving the exact disposition ID,
   timestamp, correlation, digest, and session/token are durable and identical;
5. independently mutate request digest, familiar snapshot, project, graph,
   node, attempt, validity window, and ambiguity digest and require conflict;
6. use `select_fault` for `ReconcileBeforeDisposition`,
   `ReconcileAfterDisposition`, and `ReconcileStall`, proving
   unresolved cases remain blocked while an after-commit retry recovers the
   durable disposition; and
7. use `observations` to assert adoption calls remain one, reconciliation call
   counts match the operation sequence, and the durable observation exactly
   matches a returned/fenced terminal disposition while remaining `None` for
   unresolved outcomes; assert no redispatch occurs. A fence only yields the
   typed `RedispatchEligible` model result; the suite does not claim or execute
   W5 production dispatch.

`assert_c_s9_cancellation_acknowledgement` uses the same fixture controls:
inject before/after acknowledgement faults, restart, and inspect only the
typed termination disposition. It must run the complete raw-ledger-status
negative table and accept only a valid, durable
`CancellationAcknowledgementEvidence`; unresolved outcomes remain
`termination_unknown`.

The C-S6 function fails if the fixture implements reconciliation as a local
state edit, returns `ProvenNotAdopted` for possible adoption without a fence,
changes any correlation, loses a terminal disposition on restart, or consumes
a second adoption script. Therefore the plan may cite C-S6 only for this fake
behavior contract; G4 still requires the same suite against the real adapter.

- [ ] **Step 5: Run property tests with deterministic seeds**

```bash
PROPTEST_CASES=2048 PROPTEST_RNG_SEED=00000000000000000000000000000000 \
  cargo test -p psyche-test-support --test state_machine -- --nocapture
cargo test -p psyche-test-support --test conformance -- --nocapture
```

Expected: all 2,048 model cases and every behavior conformance case pass;
failures print a reproducible seed. The C-S6 cases exercise both return and
fence dispositions plus all declared fault points.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-test-support
git commit -m "test(g2): add reusable state-machine suites"
```

---

### Task 10: Prove migration and crash atomicity

**Files:**
- Modify: `crates/psyche-store/Cargo.toml`
- Modify: `crates/psyche-store/src/lib.rs`
- Modify: `crates/psyche-store/src/migrations.rs`
- Create: `crates/psyche-store/src/migration_test_support.rs`
- Create: `crates/psyche-store/src/bin/crash_writer.rs`
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

#[test]
fn killed_inside_migration_transaction_rolls_back_before_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("psyche.sqlite3");
    let status = std::process::Command::new(assert_cmd::cargo::cargo_bin!("crash_writer"))
        .arg(dir.path())
        .arg("exit-during-migration")
        .status()
        .unwrap();
    assert!(!status.success());

    let raw = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        raw.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'canonical_records'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    drop(raw);

    assert_eq!(Store::open(&path).unwrap().schema_version().unwrap(), 1);
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-store --features test-fault-injection --test crash -- --nocapture
```

Expected: failure because `crash_writer` is absent.

- [ ] **Step 3: Implement deterministic crash points**

`crash_writer` accepts exactly:

- `exit-before-commit`;
- `exit-after-record-before-transition`;
- `exit-after-commit-before-checkpoint`;
- `exit-during-migration`.

Add this package-local test configuration:

```toml
[features]
test-fault-injection = []

[[bin]]
name = "crash_writer"
path = "src/bin/crash_writer.rs"
required-features = ["test-fault-injection"]

[[test]]
name = "crash"
path = "tests/crash.rs"
required-features = ["test-fault-injection"]
```

`migration_test_support.rs` is compiled and exported only under
`test-fault-injection`:

```rust
#[cfg(feature = "test-fault-injection")]
#[doc(hidden)]
pub mod migration_test_support;
```

The module owns a test-only `MigrationFaultPoint` and migration driver. That
driver configures the connection, begins `BEGIN EXCLUSIVE`, invokes the same
package-private `apply_migration_sql(transaction, 1)` primitive as
`Store::open`, then aborts at `AfterMigrationSqlBeforeUserVersion`: before
`PRAGMA user_version = 1` and before commit. The `exit-during-migration` mode
creates a version-0 database and invokes this test-only driver directly; it
must not call `Store::open`, which would finish the migration before the fault
can fire. Production transaction code has no callback or selectable seam.

The other modes open a real store, write the committed baseline, begin the
named operation, then call `std::process::abort()` at the declared point. The
helper is a `psyche-store` binary target, so `cargo_bin!("crash_writer")` in
the `psyche-store` integration test resolves within the package Cargo is
testing. Default builds do not compile the feature-gated module or binary, and
the public production `Store` API contains no fault selector or test hook.

- [ ] **Step 4: Add reopen assertions**

For every crash point, prove:

- SQLite integrity check returns `ok`;
- only committed rows are visible;
- migration version is either wholly old or wholly new;
- no same-ID/different-digest record exists;
- quarantine and complete transition-history invariants hold;
- reopening twice is idempotent.

- [ ] **Step 5: Run crash and migration suites repeatedly**

```bash
for run in 1 2 3 4 5; do
  cargo test -p psyche-store --features test-fault-injection \
    --test crash -- --nocapture || exit 1
done
cargo test -p psyche-store --test migrations -- --nocapture
```

Expected: all five runs and migration tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-store
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checkpoint_failure_stops_and_releases_every_shutdown_waiter() {
    let checkpoint_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = std::sync::Arc::new(Runtime::with_checkpoint_backend_for_test(
        failing_checkpoint_backend(checkpoint_count.clone()),
    ));
    let gate = std::sync::Arc::new(tokio::sync::Barrier::new(65));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let runtime = runtime.clone();
        let gate = gate.clone();
        tasks.spawn(async move {
            gate.wait().await;
            runtime.shutdown().await
        });
    }
    gate.wait().await;

    let mut failures = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(result) = tasks.join_next().await {
            let error = result.unwrap().unwrap_err();
            let RuntimeError::Checkpoint(source) = error else {
                panic!("shutdown returned a non-checkpoint result");
            };
            failures.push(source);
        }
    })
    .await
    .expect("shutdown observers were stranded");
    assert_eq!(checkpoint_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(runtime.state(), LifecycleState::Stopped);
    assert!(failures[1..]
        .iter()
        .all(|failure| std::sync::Arc::ptr_eq(&failures[0], failure)));
    let RuntimeError::Checkpoint(repeated) = runtime.shutdown().await.unwrap_err() else {
        panic!("post-Stopped shutdown returned a different result");
    };
    assert!(std::sync::Arc::ptr_eq(&failures[0], &repeated));
}
```

Place the checkpoint-failure test in the `#[cfg(test)]` unit-test module in
`psyche-runtime/src/lib.rs` so it can access the private backend seam;
`tests/lifecycle.rs` retains the public clean-start/shutdown coverage.

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p psyche-runtime start_opens_the_store
```

Expected: failure because `Runtime` does not own a store.

- [ ] **Step 3: Add the store without weakening shutdown**

Add `store: std::sync::Mutex<psyche_store::Store>` to `Runtime`.
`Runtime::start` opens `config.data_dir.join("psyche.sqlite3")` before publishing
`LifecycleState::Running`. Add `RuntimeError::Store`.

Represent the terminal drain result as a cloneable internal
`ShutdownOutcome::Clean | CheckpointFailed(Arc<StoreError>)`. Expose checkpoint
failure as `RuntimeError::Checkpoint(Arc<StoreError>)`; this preserves the
original store error for the elected driver and lets every waiter receive the
same deterministic failure object.

At the drain seam, exactly the elected driver:

1. locks the store;
2. runs `checkpoint` into a local `ShutdownOutcome` without `?` or early return;
3. releases the lock;
4. acquires the lifecycle guard, stores the outcome with
   `LifecycleState::Stopped`, and notifies all observers;
5. releases the lifecycle guard; and
6. returns the stored outcome to its caller.

Every caller that observes `Draining` waits for notification, then clones and
returns the published terminal outcome. Calls that begin after `Stopped` return
that same outcome immediately. Therefore a checkpoint failure never leaves the
runtime in `Draining`, never strands shutdown observers, and is not converted
to success for waiters. The implementation must not use `?` between checkpoint
and publication. From successful election through terminal publication there
is no `.await`; checkpoint errors are converted into the terminal outcome
rather than returning early, so cancellation cannot interrupt the elected
synchronous publication section.

Use a private `CheckpointBackend` seam inside `psyche-runtime`; production wraps
`psyche_store::Store`, while a `#[cfg(test)]` constructor supplies the
deterministic failing backend used above. No checkpoint fault selector is
public or compiled into the production API.

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
runs once; clean shutdown returns success to all callers; checkpoint failure
publishes `Stopped`, wakes all observers, and returns the same preserved error
to driver and waiters; unsupported DB never reaches `Running`.

- [ ] **Step 5: Commit**

```bash
git add crates/psyche-runtime
git commit -m "feat(runtime): bind lifecycle to durable store"
```

---

### Task 12: Add CI wiring and G2 documentation

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `scripts/check-g2-evidence.py`
- Create: `scripts/check-g2-evidence-test.py`
- Create: `scripts/g2-test-manifest.json`
- Create: `docs/ARCHITECTURE.md`
- Create: `docs/SCHEMAS.md`
- Create: `docs/TESTING.md`
- Create: `docs/G2-EVIDENCE.md`

- [ ] **Step 1: Add a failing CI relationship check**

Create `scripts/check-g2-evidence.py` that fails unless:

- the workflow invokes workspace tests;
- `psyche-store/tests/migrations.rs` and `crash.rs` exist;
- every test target and fully qualified test name in
  `scripts/g2-test-manifest.json` exists;
- all twelve reusable C-S1 through C-S12 functions and exact wrappers exist,
  each wrapper is manifest-listed and has exactly one evidence row,
  `CovenPort` exposes `reconcile`, and the
  C-S6 suite references immutable correlation, durable return/fence
  dispositions, restart, fault injection, and no-redispatch assertions;
- `AdoptionRequest::new` owns full-request digest construction, both boundary
  owners recompute it, and the C-S4/state-machine tests include every-field
  stale-digest mutation;
- C-S9 uses core-owned `CancellationAcknowledgementEvidence` and its negative matrix names
  every raw ledger status, including `killed` and `orphaned`;
- direct typed insertion rejects acknowledged cancellation without matching
  core-owned evidence, and `psyche-coven` contains no separately owned
  acknowledgement wire type;
- `result-bundle.json` parses as the strict `ResultBundle` fixture; its primary
  result and every artifact contain digest, media type, size, and expiry, and
  the C-S10 mismatch tests name all four fields plus correlation/lifetime;
- the complete `ErrorCode::ALL`, canonical delivery fixture, owned surface
  types, owned `QuarantineId` tests, and both local nullable-binding fixtures
  are present at their declared paths and parse successfully;
- `RecordKind` has exactly 15 variants, contains `Attempt` but no
  `ExecutionBinding`, every exhaustive prefix/mapping match uses that same set,
  and the manifest-listed identity test proves
  `SchemaKind::ExecutionBinding -> RecordKind::Attempt -> att_`;
- both execution-request golden fixtures contain RFC 3339 string timestamps,
  have no trailing newline, canonicalize byte-for-byte, and hash to the two
  plan-pinned SHA-256 values;
- `docs/G2-EVIDENCE.md` names every G2 criterion and, once passed, the tested
  source commit plus its immutable CI run URL;
- the evidence source table exactly matches the pinned Coven specification
  commit/URLs/digests above; in passed state it also names a 40-character Coven
  plan commit, a blob URL for this exact plan path at that commit, and the
  plan's SHA-256;
- the evidence matrix has exactly one row per required criterion and every
  command cell exactly equals the allowlisted executable command declared by
  this plan;
- in passed state, the tested source is an ancestor, the source-to-HEAD diff
  contains only `docs/G2-EVIDENCE.md`, and `gh run view` reports the attested
  run succeeded with `headSha` equal to the tested source;
- in passed state, every result cell is exactly `passed`, every artifact cell
  is exactly the same verified Actions run URL as `CI attestation`, and no
  field or table cell contains candidate placeholders such as `not run
  remotely`, `none`, `not recorded`, `pending`, `placeholder`, `TBD`, or
  `TODO` (case-insensitive).

The checker parses the Markdown table rather than searching for substrings.
Duplicate/missing criteria, altered commands, empty cells, extra passed-state
rows, or a result/artifact copied from a different run fail. Candidate state
accepts only the exact placeholders shown below and cannot be mistaken for
passed.

Source verification uses `gh api` against `OpenCoven/coven` at the commit
embedded in each URL, decodes the returned content, and recomputes SHA-256.
Every URL must match
`https://github.com/OpenCoven/coven/blob/<percent-encoded-40-hex-sha>/<path>`.
The checker URL-decodes the path component, removes display-grouping hyphens
from SHA/digest fields, then requires exact 40-hex/64-hex values; the API
response's blob SHA must belong to that commit. The fixed specification rows
must normalize to `42dcbc43-34cb48ec-af63efb5-50345e3e-ea2fb7ad`; the plan fields
may use a later reviewed Coven commit but must resolve to
`docs/superpowers/plans/2026-08-05-psyche-w2-g2-foundation.md`. Network/API
failure, branch URLs, relative paths, SHA/digest disagreement, or content not
reachable from the named commit fails evidence. The CI checker already has
`GH_TOKEN`; no credential is recorded.

`g2-test-manifest.json` maps every atomic matrix command to its Cargo target and
at least one exact fully qualified test name. Every filtered command uses
`-- --exact <name>`; prefix/substring filters are forbidden. For each unique
target, the checker runs the corresponding
`cargo test ... -- --list --format terse`, parses the listed test names, and
requires the manifest's exact names to exist. It then requires every atomic
command split from the matrix's allowlisted `&&` sequences to have a manifest
entry. A target that lists zero tests, an absent name, a filter without
`--exact`, or a manifest entry unused by the matrix fails. Test listing is
mandatory in candidate and passed states, so Cargo's successful zero-match
behavior can never satisfy evidence.

Use this complete manifest shape:

```json
{
  "targets": {
    "psyche-core/contracts": {
      "list_command": "cargo test -p psyche-core --test contracts -- --list --format terse",
      "tests": [
        "delivery_keeps_the_canonical_del_prefix",
        "delegation_uses_the_distinct_dlg_prefix",
        "execution_binding_uses_attempt_as_its_only_record_kind",
        "all_canonical_error_codes_decode",
        "delivery_v1_fixture_round_trips_canonically",
        "surface_event_and_effect_fixtures_round_trip",
        "cancellation_state_vocabulary_requires_matching_o5_evidence",
        "graph_and_node_accept_only_the_two_frozen_nullable_bindings"
      ]
    },
    "psyche-core/decode": {
      "list_command": "cargo test -p psyche-core --test decode -- --list --format terse",
      "tests": [
        "recognized_error_envelope_decodes_exhaustively",
        "unknown_typed_enum_is_a_quarantinable_decode_failure"
      ]
    },
    "psyche-coven/request_digest": {
      "list_command": "cargo test -p psyche-coven --test request_digest -- --list --format terse",
      "tests": [
        "execution_request_launch_matches_golden_bytes_and_digest",
        "execution_request_input_matches_golden_bytes_and_digest"
      ]
    },
    "psyche-coven/bindings": {
      "list_command": "cargo test -p psyche-coven --test bindings -- --list --format terse",
      "tests": [
        "result_bundle_fixture_round_trips_complete_content_references",
        "content_reference_rejects_digest_size_media_type_and_lifetime_mismatch"
      ]
    },
    "psyche-store/records": {
      "list_command": "cargo test -p psyche-store --test records -- --list --format terse",
      "tests": [
        "delivery_direct_insert_round_trips_canonically",
        "direct_insert_rejects_wrong_field_id_kind_without_writing",
        "direct_insert_rejects_acknowledged_cancellation_without_evidence",
        "direct_insert_rejects_mismatched_cancellation_evidence",
        "transition_versions_are_monotonic_and_append_only"
      ]
    },
    "psyche-store/retention": {
      "list_command": "cargo test -p psyche-store --test retention -- --list --format terse",
      "tests": [
        "quarantine_id_constructor_parser_and_serde_round_trip",
        "unknown_enum_is_quarantined_without_dispatchable_record",
        "quarantine_resolution_is_durable_and_idempotent"
      ]
    },
    "psyche-store/migrations": {
      "list_command": "cargo test -p psyche-store --test migrations -- --list --format terse",
      "tests": ["fresh_store_applies_v1_once_and_reopens"]
    },
    "psyche-store/crash": {
      "list_command": "cargo test -p psyche-store --features test-fault-injection --test crash -- --list --format terse",
      "tests": ["killed_writer_exposes_only_committed_state_after_reopen"]
    },
    "psyche-runtime/lib": {
      "list_command": "cargo test -p psyche-runtime --lib -- --list --format terse",
      "tests": ["tests::checkpoint_failure_stops_and_releases_every_shutdown_waiter"]
    },
    "psyche-test-support/fakes": {
      "list_command": "cargo test -p psyche-test-support --test fakes -- --list --format terse",
      "tests": ["advertised_adoption_requires_a_scripted_adoption_step"]
    },
    "psyche-test-support/state_machine": {
      "list_command": "cargo test -p psyche-test-support --test state_machine -- --list --format terse",
      "tests": [
        "model_and_store_agree_after_any_foundation_operation_sequence",
        "c_s6_model_never_redispatches_without_fence",
        "request_digest_binds_every_typed_field"
      ]
    },
    "psyche-test-support/conformance": {
      "list_command": "cargo test -p psyche-test-support --test conformance -- --list --format terse",
      "tests": [
        "c_s1_contract_negotiation",
        "c_s2_session_lifecycle",
        "c_s3_snapshot_attempt_binding",
        "c_s4_stable_adoption",
        "c_s5_non_adoption_proof",
        "c_s6_ambiguity_fence",
        "c_s7_ordered_cursor",
        "c_s8_terminal_authority",
        "c_s9_cancellation_acknowledgement",
        "c_s10_result_artifact_binding",
        "c_s11_restart_persistence",
        "c_s12_structured_denial"
      ]
    }
  }
}
```

Run:

```bash
python3 scripts/check-g2-evidence.py
```

Expected: fail because `docs/G2-EVIDENCE.md` is absent; after the evidence file
exists, it must also fail if any manifest name is changed to a nonexistent test
or if an exact test is removed.

`check-g2-evidence-test.py` unit-tests the parser/listing layer with injected
command output. It must cover a valid exact name, zero listed tests, a missing
manifest name, a substring filter without `--exact`, an unused manifest entry,
a duplicate matrix row, a relative/mutable Coven URL, and a source SHA-256
mismatch. It also removes each C-S1 through C-S12 wrapper/evidence row in turn,
tries to mark `ExpectedUnsupported` as passed, and removes each required
digest/media-type/size/expiry field from the C-S10 fixture; every mutation must
be rejected. A fixture mutation that adds a second binding-named
`RecordKind`, changes `SchemaKind::ExecutionBinding` away from
`RecordKind::Attempt`, or gives another variant the `att_` prefix must also
fail.

- [ ] **Step 2: Wire exact CI commands**

The workflow must run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
PROPTEST_CASES=2048 PROPTEST_RNG_SEED=00000000000000000000000000000000 cargo test -p psyche-test-support --test state_machine
cargo test -p psyche-test-support --test conformance
cargo test -p psyche-store --test migrations
cargo test -p psyche-store --features test-fault-injection --test crash
cargo clippy -p psyche-store --all-targets --features test-fault-injection -- -D warnings
cargo deny check licenses advisories bans sources
gitleaks detect --no-banner --redact --log-opts="--all"
python3 scripts/check-g2-evidence-test.py
python3 scripts/check-g2-evidence.py
```

No G2 test may be skipped or xfailed on the real target matrix. Platform-only
signal tests retain their existing justified skips.
The workflow step running `check-g2-evidence.py` sets
`GH_TOKEN: ${{ github.token }}` so passed-state attestation verification can
query the run without embedding credentials. Candidate-state validation stays
offline.

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
unknown-kind/major/enum quarantine behavior, canonical digest rules, the
exhaustive but non-persistable `psyche.error.v1` decode, the validated
store-owned `Transition` shape/digest rules, the authoritative `del_` delivery
and derived `dlg_` delegation prefixes, the exact canonical delivery v1 fields,
the owned/bounded surface envelopes, the `qua_` quarantine ID contract,
quarantine resolution fields and idempotency, the transition-history and
audit-event exclusions from automated retention, the G2-owned provisional
`CancellationState` vocabulary and its core-owned O5 evidence mapping, the
complete content-addressed result/artifact reference shape and retention
ownership, the single `Attempt`/`att_` identity mapping for execution bindings
(with no duplicate binding-named record kind), and each deferred policy owner.
`TESTING.md` explains fake scripts,
deterministic seeds, crash points,
the adapter-neutral fixture fault/observation contract, the full-request digest
mutation/RFC3339 golden matrix, the exact-test manifest/listing guard, the O5
raw-status rejection matrix, immutable Coven source verification, and the C-S6
return/fence/restart/no-redispatch matrix. It lists the exact wrapper and
positive/negative scripted-boundary behavior for every C-S1 through C-S12,
including why `ExpectedUnsupported` is diagnostic rather than passed evidence.

- [ ] **Step 4: Write the evidence template with actual fields**

`G2-EVIDENCE.md` must contain:

```markdown
# G2 Contract Foundation Evidence

**Status:** candidate
**Tested source commit:** not recorded before remote review
**CI attestation:** not recorded before remote review
**Coven plan source commit:** not recorded before plan approval
**Coven plan URL:** not recorded before plan approval
**Coven plan SHA-256:** not recorded before plan approval
**Coven specification source commit:** `42dcbc43-34cb48ec-af63efb5-50345e3e-ea2fb7ad`

| Coven source | Immutable URL | SHA-256 |
|---|---|---|
| PLAN | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/PLAN.md` | `sha256:01382f8a-0d2bca95-ddd53563-4dd6a9f0-9ac4a80d-588ccbeb-d72f163e-af56bc1e` |
| RUNTIME_DESIGN | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/RUNTIME_DESIGN.md` | `sha256:ab8c9222-14b8f117-9ebf71fb-8dfb55bd-6d0ff2d6-dfced455-1bf90503-767bb6b8` |
| TECH | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/TECH.md` | `sha256:1d00fb2b-725f384c-a027db60-d0afbd0a-62a7ec6c-7dcbb563-7bf14d30-d40e2e1c` |
| COVEN_PREREQUISITES | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/COVEN_PREREQUISITES.md` | `sha256:33994a28-921e70f8-24b0260c-e08231b2-117c5043-0c54e996-ed47582d-060e72f9` |
| COVEN_W1_AUDIT | `https://github.com/OpenCoven/coven/blob/42dcbc43%334cb48ec%61f63efb5%350345e3e%65a2fb7ad/specs/psyche/COVEN_W1_AUDIT.md` | `sha256:eab9028b-f7ef9c8a-96d4c6be-d69e4ef0-b3497b47-0ca26589-cb3ffcd8-0677322d` |

| Criterion | Command | Result | Artifact |
|---|---|---|---|
| Canonical ID prefixes and execution-binding identity | `cargo test -p psyche-core --test contracts -- --exact delivery_keeps_the_canonical_del_prefix && cargo test -p psyche-core --test contracts -- --exact delegation_uses_the_distinct_dlg_prefix && cargo test -p psyche-core --test contracts -- --exact execution_binding_uses_attempt_as_its_only_record_kind` | not run remotely | none |
| Complete canonical error enum | `cargo test -p psyche-core --test contracts -- --exact all_canonical_error_codes_decode` | not run remotely | none |
| Canonical delivery v1 shape | `cargo test -p psyche-core --test contracts -- --exact delivery_v1_fixture_round_trips_canonically && cargo test -p psyche-store --test records -- --exact delivery_direct_insert_round_trips_canonically` | not run remotely | none |
| Surface and quarantine owned types | `cargo test -p psyche-core --test contracts -- --exact surface_event_and_effect_fixtures_round_trip && cargo test -p psyche-store --test retention -- --exact quarantine_id_constructor_parser_and_serde_round_trip` | not run remotely | none |
| Package-local nullable-binding fixtures | `cargo test -p psyche-core --test contracts -- --exact graph_and_node_accept_only_the_two_frozen_nullable_bindings` | not run remotely | none |
| Exhaustive registered decode | `cargo test -p psyche-core --test decode -- --exact recognized_error_envelope_decodes_exhaustively` | not run remotely | none |
| Unknown kind/version/enum denial and quarantine | `cargo test -p psyche-core --test decode -- --exact unknown_typed_enum_is_a_quarantinable_decode_failure && cargo test -p psyche-store --test retention -- --exact unknown_enum_is_quarantined_without_dispatchable_record` | not run remotely | none |
| Quarantine resolution | `cargo test -p psyche-store --test retention -- --exact quarantine_resolution_is_durable_and_idempotent` | not run remotely | none |
| Direct typed insert validation | `cargo test -p psyche-store --test records -- --exact direct_insert_rejects_wrong_field_id_kind_without_writing && cargo test -p psyche-store --test records -- --exact direct_insert_rejects_acknowledged_cancellation_without_evidence && cargo test -p psyche-store --test records -- --exact direct_insert_rejects_mismatched_cancellation_evidence` | not run remotely | none |
| Transition contract and append-only rules | `cargo test -p psyche-store --test records -- --exact transition_versions_are_monotonic_and_append_only` | not run remotely | none |
| Checkpoint-failure shutdown | `cargo test -p psyche-runtime --lib -- --exact tests::checkpoint_failure_stops_and_releases_every_shutdown_waiter` | not run remotely | none |
| Migrations | `cargo test -p psyche-store --test migrations -- --exact fresh_store_applies_v1_once_and_reopens` | not run remotely | none |
| State-machine/property | `cargo test -p psyche-test-support --test state_machine -- --exact model_and_store_agree_after_any_foundation_operation_sequence` | not run remotely | none |
| Crash/restart | `cargo test -p psyche-store --features test-fault-injection --test crash -- --exact killed_writer_exposes_only_committed_state_after_reopen` | not run remotely | none |
| Fake boundaries | `cargo test -p psyche-test-support --test fakes -- --exact advertised_adoption_requires_a_scripted_adoption_step` | not run remotely | none |
| Execution request RFC3339 golden bytes | `cargo test -p psyche-coven --test request_digest -- --exact execution_request_launch_matches_golden_bytes_and_digest && cargo test -p psyche-coven --test request_digest -- --exact execution_request_input_matches_golden_bytes_and_digest` | not run remotely | none |
| G2 cancellation-state vocabulary | `cargo test -p psyche-core --test contracts -- --exact cancellation_state_vocabulary_requires_matching_o5_evidence` | not run remotely | none |
| Full execution-request digest binding | `cargo test -p psyche-test-support --test state_machine -- --exact request_digest_binds_every_typed_field` | not run remotely | none |
| C-S1 scripted contract negotiation | `cargo test -p psyche-test-support --test conformance -- --exact c_s1_contract_negotiation` | not run remotely | none |
| C-S2 scripted session lifecycle | `cargo test -p psyche-test-support --test conformance -- --exact c_s2_session_lifecycle` | not run remotely | none |
| C-S3 scripted snapshot/attempt binding | `cargo test -p psyche-test-support --test conformance -- --exact c_s3_snapshot_attempt_binding` | not run remotely | none |
| C-S4 scripted stable adoption | `cargo test -p psyche-test-support --test conformance -- --exact c_s4_stable_adoption` | not run remotely | none |
| C-S5 scripted non-adoption proof | `cargo test -p psyche-test-support --test conformance -- --exact c_s5_non_adoption_proof` | not run remotely | none |
| C-S6 scripted ambiguity reconciliation/fence | `cargo test -p psyche-test-support --test state_machine -- --exact c_s6_model_never_redispatches_without_fence && cargo test -p psyche-test-support --test conformance -- --exact c_s6_ambiguity_fence` | not run remotely | none |
| C-S7 scripted ordered cursor | `cargo test -p psyche-test-support --test conformance -- --exact c_s7_ordered_cursor` | not run remotely | none |
| C-S8 scripted terminal authority | `cargo test -p psyche-test-support --test conformance -- --exact c_s8_terminal_authority` | not run remotely | none |
| C-S9 scripted O5 cancellation acknowledgement | `cargo test -p psyche-test-support --test conformance -- --exact c_s9_cancellation_acknowledgement` | not run remotely | none |
| C-S10 scripted result/artifact binding | `cargo test -p psyche-coven --test bindings -- --exact result_bundle_fixture_round_trips_complete_content_references && cargo test -p psyche-coven --test bindings -- --exact content_reference_rejects_digest_size_media_type_and_lifetime_mismatch && cargo test -p psyche-test-support --test conformance -- --exact c_s10_result_artifact_binding` | not run remotely | none |
| C-S11 scripted restart persistence | `cargo test -p psyche-test-support --test conformance -- --exact c_s11_restart_persistence` | not run remotely | none |
| C-S12 scripted structured denial | `cargo test -p psyche-test-support --test conformance -- --exact c_s12_structured_denial` | not run remotely | none |
```

The relationship check accepts this explicit candidate state. For `passed`, it
requires a 40-character tested source SHA that is an ancestor of the evidence
commit, an HTTPS Actions run URL, and only `docs/G2-EVIDENCE.md` changes between
the tested source and evidence commit. It extracts the run ID from that URL and
uses `gh run view` to require `conclusion == success`, the exact URL, and
`headSha == tested source`. It deliberately does not require the tested source
SHA to equal `HEAD`: the evidence-only commit necessarily follows the source it
describes. Every matrix result must be exactly `passed`; every matrix artifact
must use that same run URL. Candidate placeholders are forbidden in passed
state.

Every `C-S* scripted ...` row attests the generic suite against the fully
scripted G2 boundary only. A real-adapter `ExpectedUnsupported` diagnostic is
recorded outside this passed matrix and cannot replace or satisfy any row.

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
cargo test -p psyche-test-support --test conformance
cargo test -p psyche-store --test migrations
cargo test -p psyche-store --features test-fault-injection --test crash
cargo clippy -p psyche-store --all-targets --features test-fault-injection -- -D warnings
cargo deny check licenses advisories bans sources
gitleaks detect --no-banner --redact --log-opts="--all"
npm --prefix packages/psyche-npm test
npm pack ./packages/psyche-npm --dry-run
python3 scripts/check-g2-evidence-test.py
python3 scripts/check-g2-evidence.py
git diff --check
```

Expected: every command exits 0; no test is ignored; npm pack still contains
only the approved wrapper files.

- [ ] **Step 2: Request focused reviews**

Request one storage/crash review and one contract/ownership review. Both must
compare the implementation with the immutable `OpenCoven/coven` URLs and
SHA-256 values recorded in `docs/G2-EVIDENCE.md`, including PLAN,
RUNTIME_DESIGN, TECH, COVEN_PREREQUISITES, COVEN_W1_AUDIT, and this reviewed
implementation plan. Review prompts must paste those immutable URLs; relative
`specs/psyche/...` or `docs/superpowers/...` paths are invalid in the
`OpenCoven/psyche` checkout. Each review records the Coven specification SHA,
plan SHA, and verified file digests in its artifact.

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

- [ ] **Step 4: Bind evidence to the tested source commit**

Push the implementation source commit before editing evidence, wait for its CI
run, and verify that the run itself names that exact source SHA:

```bash
source_sha=$(git rev-parse HEAD)
git push
run_id=
for attempt in $(seq 1 30); do
  run_id=$(gh run list --repo OpenCoven/psyche --workflow ci.yml \
    --commit "$source_sha" --json databaseId --limit 1 \
    --jq '.[0].databaseId // empty')
  test -n "$run_id" && break
  sleep 10
done
test -n "$run_id"
gh run watch "$run_id" --repo OpenCoven/psyche --exit-status
test "$(gh run view "$run_id" --repo OpenCoven/psyche \
  --json headSha --jq .headSha)" = "$source_sha"
run_url=$(gh run view "$run_id" --repo OpenCoven/psyche --json url --jq .url)

# Pin the already reviewed/merged Coven plan source; never use a branch name.
: "${COVEN_PLAN_SHA:?set COVEN_PLAN_SHA to the reviewed OpenCoven/coven commit}"
coven_plan_sha="$COVEN_PLAN_SHA"
test "${#coven_plan_sha}" = 40
gh api "repos/OpenCoven/coven/commits/$coven_plan_sha" --jq .sha \
  | grep -Fx "$coven_plan_sha"
coven_plan_path="docs/superpowers/plans/2026-08-05-psyche-w2-g2-foundation.md"
coven_plan_url="https://github.com/OpenCoven/coven/blob/$coven_plan_sha/$coven_plan_path"
coven_plan_digest=$(
  gh api "repos/OpenCoven/coven/contents/$coven_plan_path?ref=$coven_plan_sha" \
    --jq .content |
  python3 -c 'import base64,hashlib,sys; print(hashlib.sha256(base64.b64decode(sys.stdin.read())).hexdigest())'
)
test "${#coven_plan_digest}" = 64
```

Update `docs/G2-EVIDENCE.md` to `Status: passed`, set
`Tested source commit` to `$source_sha`, set `CI attestation` and every matrix
artifact to `$run_url`, set the Coven plan source commit/URL/SHA-256 to
`$coven_plan_sha`, `$coven_plan_url`, and `$coven_plan_digest`, and replace
every matrix result with exactly `passed`. Do not alter the fixed Coven
specification snapshot rows.
Before committing, assert no candidate placeholder remains:

```bash
! rg -ni 'not run remotely|not recorded|\b(none|pending|placeholder|TBD|TODO)\b' \
  docs/G2-EVIDENCE.md
```

Then commit only that file. Run:

```bash
git diff --name-only "$source_sha"..HEAD
python3 scripts/check-g2-evidence.py
```

Expected: the diff lists only `docs/G2-EVIDENCE.md`; the relationship check
accepts the tested ancestor rather than demanding self-referential `HEAD`.
Push the evidence commit and let its relationship check pass; the workflow may
rerun other gates, but the recorded attestation remains the immutable source
run. Any later executable or test change invalidates the attestation and
requires a new source SHA and CI run; later evidence-only corrections may
retain the same source while the source-to-HEAD diff remains evidence-only.

- [ ] **Step 5: Stop at G2 approval**

Do not merge and do not set `psyche.graphs.v1`. Report:

- PR URL, reviewed evidence commit SHA, and tested source SHA;
- immutable Coven plan/specification commit SHAs, URLs, and verified SHA-256 values;
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

1. every registered schema has an exhaustive strict v1 decoder, including `CanonicalDocument::Error`;
2. the error decoder accepts every canonical TECH v1 code and quarantines every unknown spelling;
3. delivery uses canonical `del_`, the exact TECH v1 wire shape, immutable effect/decision correlations, and no incompatible placeholder fields;
4. `SurfaceEvent`, `SurfaceEffect`, and `QuarantineId` have one documented owner, exact validated wire fields/IDs, bounded payloads, fixtures, and negative tests;
5. every persisted typed record is validated on both decode and direct insert;
6. every unknown kind, major, or typed enum value becomes a bounded quarantine record;
7. quarantine resolution is durable, idempotent, conflict-safe, and the only route that makes a row retention-eligible;
8. same-ID/different-digest insertion fails without mutation;
9. the store-owned `Transition` validates kind/ID/version/state/digest/time, appends monotonically, and has no update/delete API;
10. automated retention excludes transition history and audit events;
11. migrations and crash recovery are atomic, and unknown-future versions fail closed;
12. clean or failed checkpointing publishes `Stopped`, wakes all shutdown callers, and returns one deterministic terminal outcome through `Runtime::state()`;
13. C-S6 uses the executable correlation-bound reconciliation operation, persists return/fence dispositions across restart and faults, and proves no redispatch while unresolved or returned;
14. adoption IDs/digests bind the full canonical launch/input request, are recomputed by both owners, and reject every changed field before mutation;
15. C-S9 accepts only core-owned, fully correlated durable typed O5 acknowledgement evidence; direct inserts reject absent/mismatched evidence, and every raw ledger status, including `killed` and `orphaned`, remains insufficient;
16. execution-request timestamps serialize as RFC 3339 strings and both golden canonical byte/digest fixtures match exactly;
17. `CancellationState` uses the complete G2-owned local vocabulary and never claims nonexistent TECH or current Coven O5 authority;
18. every fixture include path resolves from its Rust source and the nullable-binding fixtures exist at the declared package-local paths;
19. fake and real-adapter fixtures expose the same adapter-neutral availability/fault/reset/observation controls, and reusable suites have no fake-only relaxed assertions;
20. every C-S1 through C-S12 reusable function has an exact executable wrapper, manifest entry, and evidence row; expected-unsupported diagnostics cannot be counted as passed;
21. passed evidence contains exact executable commands, `passed` results, one verified immutable run URL, immutable Coven plan/spec URLs and SHA-256 values, and no candidate placeholders;
22. the C-S10 result and every artifact carry validated digest, media type, size, expiry, and full correlation, with strict fixture, mismatch, and retention-owner tests;
23. every filtered evidence command uses an exact manifest-listed test that Cargo `--list` proves exists, so zero-match success is rejected;
24. `RecordKind::Attempt` and `att_` are the sole execution-binding identity kind/prefix; there is no duplicate binding-named record-kind variant and every exhaustive mapping/test agrees;
25. full local and remote gates pass for the attested source SHA, every later commit through the reviewed evidence head changes only `docs/G2-EVIDENCE.md`, and no W3-W9 policy or G4+ capability is enabled.
