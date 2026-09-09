# Coven Automations receipt delivery map

Status: scoped implementation map for #936

Parent program: #854

This document separates the shipped runtime path from the frozen
`coven.automations.v1` contract. The contract defines an
`AutomationReceipt`; the current runtime does not yet produce, persist,
publish, or serve one.

## Evidence boundary

The map was traced against these immutable revisions. Commit identifiers use
12-character prefixes verified as unambiguous in their repository:

| Repository | Revision | Evidence role |
| --- | --- | --- |
| `OpenCoven/coven` | `a0e4b2a48d06` | Runtime settlement, SQLite schemas, control actions, base receipt type, and authority-profile validator |
| `OpenCoven/coven` | `2fdc3b270c72` | Merge of the separately advertised `coven.automations.authority.v1` companion profile |
| `OpenCoven/coven-cave` | `2106e49a1418` | Shipped Automations client, compatibility routes, run-history UI, and tests |
| `OpenCoven/sdk` | `160864ad61ef` | Shipped package surface and immutable base-contract canary |

Issue text and architecture documents are requirements evidence, not proof
that a runtime path exists. Negative findings below are repository-wide
searches at the pinned revision, not conclusions drawn from one file.

## Current end-to-end result

The shipped path ends at a legacy-shaped run projection:

```text
terminal session evidence
  -> occurrence + active attempt + run settlement in one SQLite transaction
  -> coven.automations.runs
  -> Cave compatibility route
  -> status, timestamp, summary/log presentation
```

The required v1 path now has a durable commitment seam but no production
producer:

```text
terminal evidence
  -> [missing producer] immutable base receipt + authority sidecar correlation
  -> immutable receipt commitment + run reference + receipt event
  -> privacy-authorized daemon read
  -> SDK verification result
  -> Cave verified/degraded/unverifiable/invalid presentation
```

The first path is useful operational history. It is not an
`AutomationReceipt`, and Cave must not present it as verified receipt evidence.

## Existing, missing, and unknown behavior

| Surface | Status | Evidence and consequence |
| --- | --- | --- |
| Frozen base receipt contract | Existing | `spec/coven-automations/v1/automation-receipt.schema.json:1-141` defines immutable correlation, outcome, side-effect, integrity, producer, and privacy fields. `AutomationReceipt` verifies its JCS SHA-256 digest during deserialization (`crates/coven-cli/src/automations/contract/types.rs:2075-2200`). This proves object validation, not production. |
| Terminal session reconciliation | Existing, partial v1 projection | `settle_finished_runs` treats only `completed` plus exit code `0` before the deadline as success, settles occurrence and active attempt, finishes the run, and commits atomically. All other terminal session evidence becomes failed (`crates/coven-cli/src/automations/runner.rs:1973-2164`). |
| Retry and timeout settlement | Existing, partial v1 projection | Rejected pre-ownership launches persist failed attempts and either schedule a new adopted attempt or finish the run (`crates/coven-cli/src/automations/runner.rs:505-662`). A waiting retry that exceeds the run deadline records an attempt as `timed_out` but finishes the run as `failed` (`runner.rs:1793-1868`). |
| Occurrence distinctions | Existing, narrower than the v1 schema | Production occurrence rows use `planned`, `claimed`, `running`, `succeeded`, `failed`, and `skipped`; stale planned slots become `skipped` during claim (`crates/coven-cli/src/automations/occurrences.rs:127-179`), while settlement accepts only `succeeded` or `failed` (`occurrences.rs:371-413`). `skipped` is occurrence evidence, not a receipt outcome. |
| Receipt construction | Missing | The contract type validates integrity, but no production settlement branch constructs an `AutomationReceipt`. Coven still lacks authoritative terminal inputs for per-action side-effect class, exercised capabilities, result/delivery digests, producer authentication, and authority receipt evidence, so the legacy run projection must not fabricate them. |
| Durable receipt persistence | Immutable commitment seam exists; production producer missing | `automation_receipts` stores one validated receipt per run and terminal attempt. `commit_receipt` exact-correlates the typed receipt and `receipt.recorded` event to durable run/attempt state, inserts the immutable receipt, sets `automation_runs.receipt_id`, and appends the event under one savepoint (`crates/coven-cli/src/automations/receipts.rs`). Database triggers refuse receipt mutation/deletion and receipt-reference reassignment. No normal settlement path calls the seam yet. |
| Receipt idempotency and restart recovery | Commitment replay is safe; settlement replay remains missing | Replaying the identical receipt/event returns the committed result without a second row or event. A changed body, receipt ID, run/attempt correlation, or event fails closed, and event-append failure rolls back the receipt and run reference. Restart settlement still needs a producer that deterministically reconstructs the same terminal evidence. |
| Run/attempt correlation inputs | Existing, with immutable authority slots | Runs pin definition revision/digest, occurrence, and nullable authority profile; attempts pin run, occurrence, attempt number, adoption key, occurrence fence, dispatch generation, session, and nullable authority-extension JSON (`crates/coven-cli/src/automations/runs.rs`). Database triggers prevent a pinned run profile or attempt extension from being rewritten and prevent deletion of an authority-bound attempt. |
| Runtime authority companion contract | Dispatch pin seam exists; production adapter missing | The profile defines the execution binding and receipt-correlated sidecar and requires terminal evidence to match the base receipt (`spec/coven-automations/authority/v1/README.md:1-41`). The runner's explicit Runtime Authority mode now resolves, validates, exactly correlates, and stores one pre-dispatch extension in the same immediate transaction that moves the attempt to `dispatching`, before runtime launch (`crates/coven-cli/src/automations/runner.rs`). Existing scheduler and manual-run entry points remain base-v1 because no trusted Familiar/Threads/approval/runtime adapter is wired and the capability is not advertised. |
| Authority and approval outcome distinction | Contract and dispatch pin seam exist; live policy adapter is missing | The companion admits only `permit` and satisfied `requires_approval` bindings and makes `degrade_to_proposal` or `reject` non-dispatch outcomes (`spec/coven-automations/authority/v1/README.md:39-64`). Runtime Authority validation failures roll back the launch transaction and expose only stable refusal codes, but current production Automations actions still have no approval request/decision or effective-authority read action (`crates/coven-cli/src/control_plane.rs:108-131`). |
| Receipt event/changefeed | Atomic commitment exists; producer is missing | The event schema and append-only store support `receipt.recorded`. The receipt commitment seam writes that event atomically with the receipt and run reference, but production appends remain definition lifecycle/import events because terminal settlement does not yet construct a receipt. |
| Daemon receipt read | Missing | `coven.automations.runs` is the only run-history action. Although `list_runs` reads `receipt_id`, `automation_runs_payload` omits it and there is no `receipt.get` action (`crates/coven-cli/src/control_plane.rs:1053-1111`). |
| Privacy and redaction | Stored classification; authorized reads still missing | The receipt contract and commitment seam preserve `public`, `operational`, `sensitive`, or `restricted` classification plus retention (`crates/coven-cli/src/automations/contract/types.rs:867-895,2279-2292`). There is no receipt read API, and event reads deserialize and return stored `event_json` without a principal-aware field filter (`crates/coven-cli/src/automations/contract/events.rs:577-700`). The legacy runs action also exposes `logJson`; receipt authorization/redaction must be explicit rather than inherited from that route. |
| Effective-authority explanation | Missing read contract | The authority profile contains requested, granted, denied, degraded, approval, risk, runtime, and policy evidence, but it is not bound at dispatch and no action projects an effective `may` / `must ask` / `cannot` explanation. Daemon health advertises generic execution/request-adoption contracts, not Automations authority profiles (`crates/coven-cli/src/api_health.rs:117-145`). |
| Per-action evidence and daily aggregation | Unknown and therefore unsupported | The base receipt can list exercised capability keys and a maximum side-effect class, but the current runtime reports only session/run terminal evidence. There is no authoritative per-action ledger proving counts such as files changed, commands run, remote calls, or protected surfaces untouched. A daily view may later count verified receipts by outcome/side-effect class; it must not invent action counts from logs or model summaries. |
| SDK consumer | Contract canary only | At SDK revision `160864ad61ef`, `conformance/automations-v1-artifact-lock.json:1-41` pins the immutable base artifact, and the canary checks its object manifest. No package source implements `automations.getReceipt`, `verifyReceipt`, or subscriptions. OpenCoven/sdk#80 remains the owner. |
| Cave consumer | Legacy run/log projection only | At Cave revision `2106e49a1418`, `src/lib/server/coven-automations-client.ts:1-188` exposes list/get/create/update/delete/run/import and run-list operations. `src/lib/coven-automations-types.ts:1-62` has no receipt, authority, verification, or attempt fields. The compatibility route maps status, timestamps, exit code, and log summary only (`src/app/api/codex-automations/[id]/runs/route.ts:1-34`), and the UI opens logs rather than receipts (`src/components/automations/cron-detail-panel.tsx:410-462`). OpenCoven/coven-cave#5217 remains the presentation owner. |

## State and evidence semantics

Authorization, execution, occurrence disposition, and verification are
separate axes:

| Situation | Authoritative representation | Cave wording |
| --- | --- | --- |
| Authorized attempt completed successfully | Run/attempt `succeeded`; receipt outcome `succeeded` | "Succeeded. Receipt verified." only when integrity, authentication, and required authority correlation verify |
| Authorized attempt failed | Run/attempt `failed`; receipt outcome `failed` with failure evidence | "Allowed to run; execution failed." |
| Attempt timed out or cancellation was confirmed | Receipt outcome `timed_out` or `cancelled` | Name the terminal outcome; do not collapse it into generic failure |
| A known attempt lost conclusive outcome evidence | Run/attempt and receipt outcome `ambiguous`; occurrence `recovery_required` | "Outcome unknown; recovery required." Never infer failure, cancellation, or no effects |
| Scheduled occurrence was superseded/skipped before execution | Occurrence `skipped`/`superseded`; no fabricated attempt receipt | "Skipped before execution" with the scheduler reason |
| Authority rejected dispatch | Authority decision `reject`; no execution receipt | "Not run: authority denied the request." Successful enforcement is not execution success |
| Approval is required but not decided | Approval/occurrence state; no execution receipt | "Waiting for approval." |
| Approval expired or was revoked | Approval evidence plus the non-dispatch occurrence disposition | "Not run: approval expired/revoked." |
| Receipt is absent or required trust evidence is unavailable | Missing receipt/reference or verification dependency | "Unverifiable" or "Receipt unavailable"; never "protected surfaces untouched" |
| Receipt body or correlation is inconsistent | Failed digest, signature, run/attempt, or authority-sidecar check | "Invalid evidence" with a safe machine-derived reason |

Do not add a new four-state lifecycle enum to encode this matrix.
`verified`, `degraded`, `unverifiable`, and `invalid` are verification results,
not replacements for occurrence/run/approval outcomes.

## Dependency-ordered delivery slices

### 1. Reopen and complete #857: runtime authority and receipt commitment

#857 already owns dispatch-time identity/authority/runtime binding, terminal
receipts, privacy, and verification. Its merged PR #925 delivered the companion
profile and validator, but not the issue's runtime acceptance criteria. Reopen
it rather than creating a duplicate authority owner.

Land the work as bounded PRs:

1. Resolve and validate one immutable `AutomationExecutionBinding` after claim
   and immediately before dispatch. Persist the exact binding with the run and
   pass only its bounded runtime projection to the session launch. Missing
   adapters and unavailable trusted state fail closed.
2. The immutable receipt commitment seam is now separated from evidence
   production. It stores one validated receipt per run and terminal attempt,
   sets `automation_runs.receipt_id`, appends `receipt.recorded`, refuses
   conflicting replay, and rolls all writes back together. The remaining
   producer must construct the base receipt from pinned definition, occurrence
   fence, run, attempt, session outcome, runtime, delivery, side-effect, and
   privacy evidence, then correlate the authority sidecar without inventing
   fields the runtime did not report.
3. Make settlement replay-safe. An identical restart replay returns the
   committed receipt; a second receipt body or correlation for the same
   terminal attempt fails closed. Cover normal completion, launch refusal
   after retry exhaustion, lease-expiry exhaustion, waiting-retry timeout,
   acknowledged cancellation, and ambiguous recovery.
4. Add principal-authorized receipt/history reads and redacted event
   projections. Return explicit omitted/redacted metadata; do not expose raw
   restricted fields through the existing log-shaped action.

This work depends on the state/error/adoption contract in #855 and the
remaining cancellation/ambiguous-recovery semantics in #856. It must not
change the frozen base schema or duplicate Familiar/Threads authority rules.

### 2. Add the effective-authority read projection in Coven

After the live execution binding exists, add a read-only projection that
explains effective authority from the exact policy inputs already authenticated
for dispatch:

- `may`: currently permitted without another approval, within the displayed
  resource, capability, time, usage, and side-effect bounds;
- `must ask`: requires an unconsumed valid approval or a proposal decision;
- `cannot`: explicitly denied, unsupported, stale, revoked, outside scope, or
  unavailable.

The projection must cite machine-verifiable reason codes and evidence
references. It must not derive authority from familiar tier alone, treat Ward
gating as an unconditional prohibition, use semantic-version labels as safety
proof, or use model-generated prose as authority evidence. Cave #5217 owns the
human wording and layout; Coven owns the projection.

### 3. Complete #858 certification and publish exact producer artifacts

#858 remains the owner for conformance, failure-injection, upgrade, and
cross-repository release evidence. It depends on #855, #856, and #857. Certify
the live producer/read path and publish its exact artifacts before treating the
SDK or Cave P1 owners as complete. Narrow cross-repository canaries may consume
those artifacts for compatibility proof; they do not substitute for the
consumer implementations.

Certification must include all validation rows below, not only happy-path
receipt parsing, and must fail when a canary silently falls back to legacy
run/log evidence.

### 4. Complete SDK #80 against the certified daemon contract

OpenCoven/sdk#80 remains the correct owner for typed receipt reads,
verification, and subscriptions. Its first receipt slice follows #858's exact
producer/read artifact and must:

- expose run and receipt references without replacing unknown fields;
- verify base-receipt integrity separately from producer authentication;
- verify the authority sidecar only when the companion profile is advertised;
- return structured `verified`, `degraded`, `unverifiable`, or `invalid`
  results with safe reasons;
- preserve abort, pagination, checkpoint, and privacy/authorization failures.

The existing artifact canary is a prerequisite, not completion evidence for
these APIs.

### 5. Complete Cave #5217 as the human oversight projection

OpenCoven/coven-cave#5217 already owns receipt inspection and
verification-state presentation. After the SDK read/verify slice:

- replace the compatibility-only run shape with the typed run/attempt/receipt
  projection;
- show observed outcome separately from authorization and verification;
- show evidence digests, familiar/runtime binding, authority reason, and
  approval state only when present in the caller-authorized projection;
- render typed redacted/omitted states, privacy classification, and as-of time
  without inferring hidden values;
- render stale/offline/partially authorized data without enabling
  authority-bearing actions;
- export only the caller-authorized redacted evidence packet.

A per-familiar daily view may count verified receipts by outcome and
side-effect class. Counts of individual actions or claims about protected
resources remain disabled until a canonical per-action evidence source exists.

### 6. Complete #937: fixture-only repo-caretaker demonstration

#937 owns the reference package after the core, SDK, Cave, and #858
certification slices are green. The package is demonstration evidence, not an
authority shortcut:

- use a checked-in disposable repository fixture and an isolated worktree;
- permit bounded local reads and a narrowly scoped local-write proposal only;
- classify workflow, release, credential, policy, and other protected paths as
  requiring proposal/approval;
- prohibit secrets, package publication, deployment, merge, branch deletion,
  broad network mutation, and unattended external effects;
- record the definition revision, familiar binding, effective-authority
  explanation, attempted operation, resulting diff/artifacts, rollback, and
  receipt verification in Cave;
- make cleanup deterministic and prove the fixture returns to its initial
  digest;
- require a separate explicit activation approval before any non-fixture
  schedule is created or enabled.

The demonstration may show a denied protected mutation as successful
enforcement and an allowed local proposal that later fails as a failed
execution. It must not claim that a patch is safe merely because a dependency
uses a patch/minor version increment.

## Targeted validation plan

| Area | Required proof |
| --- | --- |
| Executed terminal settlement | Every terminal run/attempt branch, including deterministic post-binding launch refusal, commits the run, attempt, receipt, receipt reference, authority sidecar, and event atomically; injected failure at each write boundary rolls back all of them |
| Occurrence/authority non-dispatch | Skipped, superseded, rejected, degrade-to-proposal, approval-pending, expired, and revoked paths record only their applicable occurrence/authority evidence and never fabricate a run, attempt, receipt, or receipt sidecar |
| Restart and idempotency | Crash before commit produces no receipt; crash after commit replays the identical receipt; a mismatched second body or authority sidecar is rejected |
| Retry history | Each attempt remains immutable; only the terminal attempt owns the run receipt; retry exhaustion preserves prior failure classes and quarantine evidence |
| Failed actions | An authorized failing action produces a failed receipt without changing authorization to denied |
| Denied and pending work | Reject, degrade-to-proposal, approval-pending, expired, and revoked paths launch nothing and do not fabricate execution receipts |
| Cancellation and ambiguity | Cancel remains requested until acknowledged; a known attempt with unprovable outcome settles run/attempt/receipt as `ambiguous` while the occurrence becomes `recovery_required`; ambiguous mutating work is never automatically retried |
| Authority correlation | Definition, occurrence, fence, run, attempt, adoption, principal, familiar, approval, capabilities, runtime, decision, and base receipt digest exact-match trusted state |
| Tampered or missing evidence | Body digest, authentication, sidecar digest, stale policy, missing trust root, missing receipt, and cross-run replay each produce the correct verification result |
| Privacy | Each classification and caller profile receives only authorized fields; redaction is explicit; logs/errors/events never leak credentials, raw approval material, unrestricted prompts, memories, or paths |
| Changefeed | Terminal transition and receipt events are gapless, duplicate-safe, restart-safe, and visible from both run stream and global feed without state regression |
| SDK and Cave | Exact-artifact tests exercise real daemon reads, independent verification, stale/offline behavior, accessibility, redacted export, and no compatibility fallback |
| Repo caretaker | Fixture digest, bounded worktree, protected-path refusal/proposal, rollback, no external mutation, and explicit activation gate are all machine-checked |

## Completion boundary

#936 is complete when this map is accepted, #857 is restored as the core
runtime owner, the existing SDK/Cave owners carry exact links to this evidence,
and #937 carries the dependency-gated repo-caretaker demonstration.

The map does not claim that native receipt delivery, effective-authority
presentation, daily aggregation, or the repo caretaker has shipped.
