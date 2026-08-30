# Coven Automation Authority — TECH

**Status:** Draft v1 · 2026-08-30
**Companion to:** [PRODUCT.md](./PRODUCT.md)

This document maps the PRODUCT contract onto the current code and lists the
deltas needed to enforce it end to end.

## Current code mapped to PRODUCT

| PRODUCT element | Code location | State |
|---|---|---|
| Routine definitions, default PAUSED | `crates/coven-cli/src/automations/definition.rs` | Implemented (`AUTOMATION_SCHEMA_VERSION = 1`, `RoutineStatus::Paused` default) |
| Definition revision + digest pinning | (none — `automation_definitions` rows mutate in place) | **Missing** |
| Occurrence fence + lease | `crates/coven-cli/src/automations/occurrences.rs` | Implemented — `UNIQUE(automation_id, scheduled_for)` fence, compare-and-set claim, lease owner/expiry, `attempt` counter |
| Canonical occurrence key + fence generation | `attempt` column only | **Partial** — needs explicit fence-generation semantics on top of `attempt` |
| Shared dispatch path | `crates/coven-cli/src/automations/runner.rs` (`run_routine_now`, `dispatch_claimed_occurrences`) | Implemented — every run builds a `SessionLaunch` and dispatches through `SessionRuntime` |
| Per-run authorization between claim and launch | (none — the scheduler is trusted implicitly) | **Missing** |
| Principal identity + authentication | (none — local trust; no principal resolution at dispatch) | **Missing** |
| Familiar root / identity revision / declaration digest | `crates/coven-cli/src/familiar_identity.rs` | **Partial** — resolves an id to a display context from `familiars.toml`; no root, revision, digest, or revocation state |
| Adopted request / idempotency key | `crates/coven-cli/src/request_adoption.rs`, `adoption_gate.rs` | Implemented for the Psyche request path (`psyche.request_adoption.v1`, file-lock mutual exclusion); not consumed by automation dispatch |
| Psyche execution binding | `crates/coven-cli/src/execution_binding.rs` | Implemented — opaque `psyche.execution_binding.v1` validation for Psyche-orchestrated sessions; unchanged by this spec |
| Threads protected-surface decision | `crates/coven-cli/src/threads_gate.rs` | Implemented for the daemon edit path — authority weave, fail-closed verdicts, degrade-to-proposal staging, append-only `ward_audit`; not wired into automation dispatch |
| Runtime capability descriptors | `crates/coven-cli/src/capabilities.rs` | **Partial** — harness capability discovery manifests; no per-action requirement matching or dispatch-time pinning |
| Approval lifecycle | (none for automations) | **Missing** |
| Run ledger | `crates/coven-cli/src/automations/runs.rs` | Implemented — `automation_runs` records status/exit/session/log/output-commit; no binding correlation or receipt |
| Automation receipt | (none) | **Missing** |
| Nonce / replay protection | (none) | **Missing** |
| Redaction, at-rest encryption, retention | `crates/coven-cli/src/privacy.rs`, `encrypted_artifacts.rs`, trust-layer retention pruning | Implemented — reusable for receipt payload classes |
| Scheduler cadence | `crates/coven-cli/src/automations/daemon_tick.rs` | Implemented — 60s tick: plan, recover, claim, dispatch |

## Integration seams

The implementation introduces four narrow traits (consumed by
`runner.rs`; production adapters wrap canonical versioned artifacts; tests use
deterministic fakes and golden vectors). A missing adapter fails closed —
it must not fall back to unbound launch:

```rust
trait FamiliarBindingResolver {
    fn resolve_for_automation(&self, request: BindingRequest) -> Result<FamiliarBinding, BindingError>;
}

trait AutomationAuthorizer {
    fn authorize(&self, request: AuthorizationRequest) -> Result<AuthorizationDecision, AuthorizationError>;
}

trait RuntimeCapabilityResolver {
    fn select(&self, request: RuntimeRequirement) -> Result<RuntimeBinding, RuntimeError>;
}

trait ReceiptSigner {
    fn commit(&self, receipt: ReceiptBody) -> Result<CommittedReceipt, ReceiptError>;
}
```

Seam placement:

- A new `crates/coven-cli/src/automations/authority/` module owns the trait
  definitions, the `AutomationExecutionBinding` type (wire contract
  `coven.automation_execution_binding.v1`), typed refusal errors, and the
  deterministic fakes. The binding type follows the closed-wire style of
  `execution_binding.rs`: exact member-set check, `deny_unknown_fields`,
  opaque-value syntax validation, static field paths in errors, no offending
  values echoed.
- `runner.rs` changes shape, not direction: claim → resolve → authorize →
  bind → dispatch → settle → receipt. Dispatch of a claimed occurrence
  without a committed binding is a bug, not a fallback.
- `threads_gate.rs` verdicts feed `AuthorizationDecision` as the
  protected-surface decision; degrade outcomes stage proposals exactly like
  the gate does today, so automation writes never bypass the authority weave.
- `request_adoption.rs` keys become the `adoptedRequestKey` input; the
  adoption gate's lock discipline is the model for approval consumption.
- The Psyche path is unaffected: when a run arrives with a
  `psyche.execution_binding.v1`, Coven validates it exactly as today. The two
  contracts share semantics (root/revision/digest pinning, decisions, receipts)
  but stay separate artifacts — Coven does not reinterpret Psyche fields, and
  Psyche does not author Coven authority state.

## Schema changes

All additions live in the single Coven store and follow its
`*_SCHEMA_SQL` constant pattern:

```sql
-- Definitions become revisable: in-place edits mint a new revision row.
CREATE TABLE IF NOT EXISTS automation_definition_revisions (
    automation_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    definition_digest TEXT NOT NULL,   -- sha256 of canonical definition_json
    definition_json TEXT NOT NULL,
    risk_class TEXT NOT NULL,          -- R0..R4, set by reviewed policy
    created_at TEXT NOT NULL,
    PRIMARY KEY (automation_id, revision)
);

-- One immutable binding per run, committed with its authorization decision.
CREATE TABLE IF NOT EXISTS automation_execution_bindings (
    run_id TEXT PRIMARY KEY NOT NULL,
    binding_json TEXT NOT NULL,
    binding_digest TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    familiar_root_id TEXT NOT NULL,
    familiar_identity_revision TEXT NOT NULL,
    definition_revision INTEGER NOT NULL,
    decided_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES automation_runs(id)
);

-- Approval ledger; clients may request, never grant.
CREATE TABLE IF NOT EXISTS automation_approvals (
    id TEXT PRIMARY KEY NOT NULL,
    automation_id TEXT NOT NULL,
    definition_revision INTEGER NOT NULL,
    occurrence_id TEXT,
    action_digest TEXT NOT NULL,
    capabilities_digest TEXT NOT NULL,
    familiar_identity_revision TEXT NOT NULL,
    intended_runtime TEXT NOT NULL,
    risk_class TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    state TEXT NOT NULL,               -- required/requested/approved/rejected/expired/revoked/consumed
    nonce TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_by_run_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Single-use authorization nonces (replay protection).
CREATE TABLE IF NOT EXISTS automation_nonces (
    nonce TEXT PRIMARY KEY NOT NULL,
    purpose TEXT NOT NULL,
    run_id TEXT,
    consumed_at TEXT
);

-- Append-only receipt spine; sensitive payload classes route through
-- encrypted_artifacts per the trust-layer rules.
CREATE TABLE IF NOT EXISTS automation_receipts (
    run_id TEXT PRIMARY KEY NOT NULL,
    receipt_version INTEGER NOT NULL,
    receipt_digest TEXT NOT NULL,
    receipt_json TEXT NOT NULL,        -- public/operational fields only
    privacy_class TEXT NOT NULL,
    redaction_status TEXT NOT NULL,
    committed_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES automation_runs(id)
);

ALTER TABLE automation_runs ADD COLUMN binding_id TEXT;
ALTER TABLE automation_runs ADD COLUMN receipt_id TEXT;
ALTER TABLE automation_runs ADD COLUMN principal_id TEXT;
```

Append-only enforcement (receipts, bindings, nonces) uses the same
trigger-in-schema technique the `ward_audit` table uses, so tamper resistance
does not depend on call-site discipline.

## Dispatch sequence

```text
daemon tick
  -> claim occurrence (existing compare-and-set, lease, attempt++)
  -> FamiliarBindingResolver.resolve_for_automation
       root + identity revision + declaration digest + decision-time status
       ambiguous alias | paused | revoked | retired -> typed refusal, fail closed
  -> AutomationAuthorizer.authorize
       principal authentication, nonce issue, Threads verdict,
       capability decision, approval requirement + consumption, risk class
       -> AuthorizationDecision + AutomationExecutionBinding, one snapshot
  -> binding commit (same transaction where possible; digest pair otherwise)
  -> RuntimeCapabilityResolver.select
       exact descriptor/version/capabilities pinned to the run
  -> SessionLaunch (existing shared path) carrying binding reference
  -> settle occurrence + run ledger (existing)
  -> ReceiptSigner.commit (every terminal disposition, including refusals
       that consumed an occurrence)
```

Refusals between claim and dispatch settle the occurrence with a typed,
durable reason and still commit a receipt recording the refusal — an
authorization refusal is audit evidence, not silence.

## Verification plan

| PRODUCT requirement | Test surface |
|---|---|
| Principal authentication, nonce, rotation, revocation, replay | `automations::authority` unit tests: nonce single-use, replay refusal, revocation between claim and dispatch |
| Familiar alias → root resolution and ambiguity | fake resolver vectors: unique root, ambiguous alias, unknown alias — all deterministic |
| Exact revision/digest pinning; stale-revision refusal | definition edit between approval and dispatch invalidates the approval and refuses dispatch |
| Threads permit / degrade / reject vectors | fake `AutomationAuthorizer` seeded with gate verdicts; degrade stages a proposal, nothing launches |
| Approval creation, race, expiry, revocation, consumption, replay | approval-ledger tests: two consumers race one approval, expired approval refuses, changed definition invalidates |
| Runtime capability match / downgrade | fake `RuntimeCapabilityResolver`: requirement satisfied / descriptor downgrade / unavailable runtime |
| TOCTOU around claim → authorize → commit → dispatch | digest-pair assertions on the binding snapshot; policy change mid-sequence refuses or records, never silently proceeds |
| Receipt canonicalization, tamper, authentication, redaction | golden receipt vectors; digest mismatch detection; sensitive-field redaction before `receipt_json` |
| Privacy authorization for history/changefeed readers | reader-profile tests: public fields only; encrypted payload refs never resolve over client profiles |
| Duplicate worker under an old fence | lease-expiry double-claim test (extends the existing occurrence CAS tests) |
| Cross-repository canaries | golden vectors exchanged with Familiar Contract, Threads, Runtimes, SDK, and Cave at pinned artifact revisions |

Golden vectors live beside the fake adapters and are checked into the repo so
every consumer verifies the same bytes.

## Delivery plan (slices)

This spec is **slice 1 of N**. The landed foundation (coven#816) keeps running
unchanged until each slice lands; the PRODUCT standing rule applies throughout:
recurring familiar work is a local execution convenience, and unattended
external side effects stay disabled or approval-gated.

1. **This spec** — the normative contract and the code map.
2. **Binding + resolver seams** — `authority/` module, binding type, fail-closed
   gate in `runner.rs`, definition revisions table, deterministic fakes.
3. **Authorization + nonces** — principal resolution, Threads verdict feed,
   nonce table, replay tests.
4. **Approvals** — ledger, lifecycle transitions, consumption, Cave/SDK surface.
5. **Receipts + privacy classes** — receipt spine, canonicalization, redaction,
   retention, changefeed field authorization.
6. **Cross-repository canaries** — exchanged golden vectors at pinned revisions.

## Local checks for docs-only changes

This slice is documentation only; the routed CI surface is docs. Run locally:

```sh
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

Rust gates (`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo test --workspace --locked`) apply from slice 2 onward, when
code changes join the spec.
