---
source_adjacent_reason: "Retains the ADR 1 old-to-current mapping for historical coven.familiar.identityRebind.v1 guarantees next to the Coven O2/O3/O4 contracts and tests that supply its Coven-side evidence."
---

# Historical `coven.familiar.identityRebind.v1`: old-to-current mapping

**Decision:** [psyche#12 ADR 1](https://github.com/OpenCoven/psyche/issues/12#issuecomment-7117320214) (23 November 2026) replaces `coven.familiar.identityRebind.v1` through an explicit old-to-current mapping. This document is that mapping for Coven [#1230](https://github.com/OpenCoven/coven/issues/1230). The decision is made; most rows are still open.

**Status:** 22 rows. 7 are `mapped-with-evidence`; 15 are `open`. No gate row has passed. This mapping does not certify G1b, G4 or G6, does not satisfy #1230 acceptance, and does not unblock #1237. Production activation stays a separate decision.

**Baseline:** Coven `origin/main` `9415c3098f9635963db593970b8943370e90bf81`. Psyche `origin/main` `7eee7426d501357a3a13a3d051ebd05961a41089` was read for its ownership statement only. Historical specification pin `9b07c500ad10a8d509d451d64b9739a1bdc4dc0f` is unavailable ([ADR 2](https://github.com/OpenCoven/psyche/issues/12#issuecomment-7117320214)), and no replacement SHA is inferred.

## 1. Rules this mapping follows

ADR 1 sets four rules, and this document applies them literally:

1. Each historical guarantee names its producer, current contract and version, executable canary, and denial behavior. This covers a surviving prior session, unresolved adoption and restart.
2. A row migrates only with executable evidence. A row without a named, passing test on the baseline stays `open`, however complete its design is.
3. Until a route's mapping is proven, an identity change keeps that route blocked (§3).
4. No familiar or person authority is added to Coven to reproduce the historical name. Familiar Contract owns identity and person binding (packet row 1). Coven Threads owns protected-surface authorization (row 2). Psyche owns identity snapshots, intent, request meaning, migration orchestration and blocked recovery (rows 4, 5 and 15). Coven owns sessions, execution admission, adoption and non-adoption, fencing and physical terminal state (rows 8 and 9). Row 4 excludes rebind, per ADR 1. Row and producer approvals are in the same decision comment.

**Status vocabulary.** `mapped-with-evidence` means the Coven-owned slice named in that row has at least one named test on the baseline that exercises it, and every listed test passed in the run recorded in §5. It is unit- and in-process-level evidence from Coven's own test binary, not the real-daemon fault-injection packet #1230 still requires. `open` means the producer has no executable canary on the baseline, the producer is outside Coven, or both. Where one historical guarantee spans owners, it is split into separate rows. The Coven slice can then migrate while the other owner's slice stays open.

## 2. Retained sources

ADR 2 names the preserved #1230 acceptance text and the 20 October requirement matrix as the retained statement of each obligation. Each row in §4 cites one of these sources:

| Key | Source | Location |
|---|---|---|
| S1.a | #1230 issue body, "Acceptance criteria" | "The real daemon advertises … the coven.familiar.identityRebind.v1 control action …" |
| S1.b | same | "Capability-present-but-denied creates no session, Telegram request, or local fallback." |
| S1.c | same | "Coven termination/stall after discovery and at authorization, identity rebind, and turn/input adoption leaves routes or lanes blocked." |
| S1.d | same | "A pre-existing coven_adoption_unknown is fenced and resolved/quarantined before rebind or the route remains blocked." |
| S2 | [20 October requirement-to-evidence matrix](https://github.com/OpenCoven/coven/issues/1230#issuecomment-6303076065) | Rows "`coven.familiar.identityRebind.v1`", "Session familiar binding", "Termination/stall …" and "Pre-existing `coven_adoption_unknown` …" |
| S3.0 | [20 October historical-to-current inventory](https://github.com/OpenCoven/coven/issues/1230#issuecomment-6303103643) | Inventory row "`coven.familiar.identityRebind.v1`" |
| S3.1–S3.5 | same, "Required rebind composition and evidence" | Numbered steps 1–5 |
| S3.F | same | Fault-injection paragraph after step 5 (termination and stall points; listed adversarial cases) |

Both 20 October comments are treated as the matrix. The second one carries the rebind composition that the first one defers to #12.

## 3. Denial rule: an identity change keeps the route blocked

**Rule.** If the identity bound to a route changes, the route stays blocked until every row below that applies to that route is `mapped-with-evidence` and the route's own transition evidence is retained. A partial mapping does not unblock the route. Neither does an elapsed timeout or a local quarantine record.

**Who can enforce it.** Routes, lanes and graph admission belong to Psyche, and identity facts belong to Familiar Contract. Coven has no identity-change signal by design: it compares opaque O2 tuples byte for byte and holds no familiar or person authority. So Coven cannot detect that an identity changed, and it cannot block a route. Coven's contribution is narrower. A session already bound to the old tuple refuses operations carrying a different tuple (row H12), and its binding cannot be rewritten (row H11). Coven admits a launch under a new tuple as an ordinary new session. Stopping that launch while the route is unproven is Psyche's job.

**Enforcement status: not enforced, explicit gap.** Neither Coven nor Psyche `main` has an executable test showing that an identity change blocks the affected route. This is row H2, which stays `open`. Coven does not claim enforcement, and none of the passing Coven canaries below implies it.

## 4. Mapping table

Producer row numbers refer to the ownership packet rows approved in ADR 1. "O4" means the approved `psyche.adoption_resolution.v1` design ([#1627](https://github.com/OpenCoven/coven/pull/1627), [O4 design](./O4_CONTRACT_DESIGN.md)) and plan ([#1630](https://github.com/OpenCoven/coven/pull/1630)). Its implementation, [#1629](https://github.com/OpenCoven/coven/issues/1629), has not landed, and `docs/API-CONTRACT.md` still lists O4–O8 as excluded.

| ID | Historical guarantee (source) | Producer (owner) | Current contract / version | Executable canary | Denial behavior | Status |
|---|---|---|---|---|---|---|
| H1 | The daemon exposes a `coven.familiar.identityRebind.v1` control action (S1.a; S2; S3.0) | None. ADR 1 replaces the action instead of reproducing it | This table as a whole. The guarantee closes only when every other row is `mapped-with-evidence` | Missing. No crate or control-catalog entry carries the name, and none will be added | A consumer that requires the historical action fails closed, with no local fallback or substitute name | `open` |
| H2 | An identity change is detected and blocks affected admission before migration starts (S3.1; S1.c) | Psyche (row 4), using identity facts from Familiar Contract (row 1) | `psyche.identity_rebind.v1` orchestration approval, design only ([TECH.md §Familiar identity rebind](./TECH.md#familiar-identity-rebind)). No implementation on Psyche `main` | Missing. This is the §3 gap | The route stays blocked | `open` |
| H3 | The rebind record binds old and new snapshot IDs and digests, principal and project, affected routes, conversations and graphs, the unresolved execution set, a reason, a stable request ID and the record digest (S3.1) | Psyche (row 4) | `psyche.identity_rebind.v1`, design only | Missing | An incomplete or mismatched record is rejected, and the route stays blocked | `open` |
| H4 | Identity provenance is validated independently (S3.2) | Familiar Contract (row 1) | No released first-profile representation | Missing | Unverified provenance yields no new binding, and the route stays blocked | `open` |
| H5 | The protected-change decision for the rebind is validated (S3.2) | Coven Threads (row 2) | No released first-profile representation | Missing | No decision, or a mismatched one, means no rebind, and the route stays blocked | `open` |
| H6 | Every execution bound under the old snapshot is enumerated, including after coordinator loss (S3.2; S2 "Termination/stall") | Coven (row 9) | O4 immutable-binding discovery (`psyche.adoption_resolution.v1`), not implemented | Missing until #1629 lands | Incomplete discovery is `unknown`, and the route stays blocked | `open` |
| H7 | Active approvals, callbacks, verifications and deliveries are enumerated and block the rebind. None is silently discarded or treated as terminal (S3.2; S3.F "active callbacks/approvals/deliveries") | Psyche (rows 7 and 15; row 13 is held under ADR 4), with Threads for protected-surface items | No released representation | Missing | The rebind is rejected while any item is active, and the route stays blocked | `open` |
| H8 | Coven slice of S3.2: an unresolved adoption is retained. It is never silently discarded or re-executed (S3.2; S2 "O3 retains ambiguous adoption") | Coven (row 9) | O3 `psyche.request_adoption.v1` storage and adopted routes | `api::tests::adopted_input_postcommit_failures_are_retained_redacted_and_not_retried`, `store::tests::adopted_session_sacrifice_returns_typed_retention_error`, `store::tests::request_adoptions_raw_update_and_delete_are_rejected` | Post-commit failure is retained, redacted and not retried. Sacrifice of a session with adoption evidence returns a typed retention error. Raw UPDATE and DELETE on adoption rows are rejected | `mapped-with-evidence` |
| H9 | Every possible old adoption, including a pre-existing `coven_adoption_unknown`, gets an authoritative adopted, proven-not-adopted or unknown resolution with exact correlation. A timeout, a missing response or a local quarantine record cannot prove exclusion (S1.d; S2 "Pre-existing …"; S3.3) | Coven (row 9) | O4 lookup and return-or-fence (`psyche.adoption_resolution.v1`), not implemented | Missing until #1629 lands | Empty or failed lookup is `unknown`, never absence. Unknown keeps the route blocked | `open` |
| H10 | A surviving prior session or process is shown terminal or fenced before the old attempt is resolved (S3.3; ADR 1 "surviving prior session") | Coven for its sessions (row 9; row 12 is deferred). The process-attempt fence issuer is held under ADR 3 (row 11; psyche#19, Coven #804) | O5 terminal acknowledgement, not designed or approved. ADR 3 fence, not implemented | Missing | Termination-unknown, abandoned or recovery-required attempts stay non-dispatchable, and the route stays blocked | `open` |
| H11 | Coven slice of S3.4: an old session's execution binding is immutable, so it can never be rebound to the new snapshot (S3.4; S2 "Session familiar binding") | Coven (row 9) | O2 `psyche.execution_binding.v1` storage, plus O3 event-correlation triggers | `store::tests::execution_binding_survives_status_and_archive_updates_unchanged`, `store::tests::request_adoption_event_correlation_rejects_invalid_insert_and_rebind` | Binding bytes are unchanged across status, archive and summon. A raw correlation rebind is rejected by the store | `mapped-with-evidence` |
| H12 | Coven slice of S3.4: a surviving prior session refuses input and kill under any changed tuple field, including `familiarId`, `familiarSnapshotDigest`, `principalRef` and `projectDigest` (S3.4; S3.F "changed snapshot/principal/project"; ADR 1 "surviving prior session") | Coven (row 9) | O2 `psyche.execution_binding.v1` exact comparison on bound operations | `api::tests::bound_operations_reject_every_mismatched_root_field`, `api::tests::bound_operations_reject_every_mismatched_parent_field` | `409 execution_binding_mismatch` with the first mismatched path, and submitted values redacted. Limit: this is admission refusal only. It does not show the old session has stopped (H10), and it does not stop a caller that still presents the old tuple (H2) | `mapped-with-evidence` |
| H13 | Coven slice of S3.F "revoked or expired authority": an expired O2 binding admits no new input, but the session can still be terminated (S3.F; TECH.md "inspected or terminated but never resumed") | Coven (row 9) | O2 `psyche.execution_binding.v1` expiry on bound operations | `api::tests::bound_input_rejects_expired_exact_proof`, `api::tests::bound_input_expired_rejection_calls_no_runtime_or_event`, `api::tests::bound_kill_accepts_expired_exact_proof` | `409 execution_binding_expired`, with no runtime call and no event. Kill with the exact expired proof is accepted | `mapped-with-evidence` |
| H14 | Revoked authority (identity or protected-surface decision) is refused (S3.F) | Familiar Contract (row 1) and Coven Threads (row 2) | No released first-profile representation | Missing | Revoked authority yields no new binding, and the route stays blocked | `open` |
| H15 | Old route bindings are archived and affected pairings and callback nonces invalidated atomically. The new binding is published only after the H9 and H10 exclusion evidence (S3.4) | Psyche (row 4) | `psyche.identity_rebind.v1`, design only | Missing | Without exclusion evidence, no new binding is published and the route stays blocked | `open` |
| H16 | Coven slice of S3.5: after restart, replay by stable request identity is read-only and never re-executes (S3.5; ADR 1 "restart") | Coven (row 9) | O3 `psyche.request_adoption.v1` replay | `api::tests::adopted_input_sequential_terminal_and_restart_replay_is_read_only`, `api::tests::adopted_launch_expired_committed_created_survives_reap_restart_and_replay`, `store::tests::request_adoptions_close_reopen_preserves_exact_record_and_session` | Replay returns the retained record with zero runtime calls. Limit: restart here is store close/reopen and re-initialization inside one test process, not a real-daemon kill | `mapped-with-evidence` |
| H17 | Rebind replay or restart issues no second transition and never resumes an old session under the new identity. An inconclusive result stays blocked (S3.5; TECH.md "Lost rebind responses") | Psyche (rows 4 and 15) | `psyche.identity_rebind.v1`, design only | Missing | Inconclusive leaves admission blocked. File state alone never shows the new identity active | `open` |
| H18 | Coven slice of S3.F: a conflicting same-ID, different-digest replay is refused without execution (S3.F) | Coven (row 9) | O3 `psyche.request_adoption.v1` global key and attempt scope | `store::tests::request_adoptions_each_same_key_identity_difference_conflicts`, `api::tests::adopted_launch_conflict_matrix_and_new_attempt`, `api::tests::adopted_input_global_key_conflicts_are_static_and_redacted` | Static, redacted `requestAdoption.key` conflict with no runtime side effect | `mapped-with-evidence` |
| H19 | A stale fence generation is refused (S3.F) | Coven (row 9), with the ADR 3 issuer (row 11) | O4 and ADR 3 fence, not implemented | Missing | Stale generation is denied, and the attempt stays unknown | `open` |
| H20 | Coven slice of S1.b: a capability-present but denied request creates no session, no adoption, no runtime effect and no legacy fallback (S1.b) | Coven (rows 8 and 9) | O2 and O3 adopted routes | `api::tests::adopted_input_precommit_denials_write_no_adoption`, `api::tests::bound_legacy_launch_requires_adoption_and_never_executes` | Pre-commit denial writes no adoption row and no runtime call. A bound legacy launch returns `request_adoption_required`, with no launch and no store created | `mapped-with-evidence` |
| H21 | Psyche slice of S1.b: a denied request sends no Telegram request and takes no local fallback (S1.b) | Psyche Telegram component (row 3) and adapter | Adapter side-effect counters, not released for the real daemon | Missing | No network effect and no fallback | `open` |
| H22 | Termination and stall injected before and after discovery, authorization, launch or input adoption, lookup or fencing, and rebind commit leave routes or lanes blocked. Observations are taken after a new-process reopen (S1.c; S2 "Termination/stall"; S3.F) | Coven (O7 crash and restart across O2–O6, not designed) and Psyche recovery (row 15) | Unchanged fake/real suite against a pinned daemon digest, not run | Missing | The route or lane stays blocked. Session and adoption cardinality, exact binding and fallback count are retained | `open` |

**Rows by status:** 7 `mapped-with-evidence` (H8, H11, H12, H13, H16, H18 and H20), all Coven-owned slices of O2 and O3; 15 `open` (H1–H7, H9, H10, H14, H15, H17, H19, H21 and H22).

## 5. Canary run record

All 18 named canaries were run once on the baseline, in one invocation:

```sh
# worktree at 9415c3098f9635963db593970b8943370e90bf81, rustc 1.99.1
cargo test -p coven-cli --locked --bin coven -- --exact \
  api::tests::adopted_input_postcommit_failures_are_retained_redacted_and_not_retried \
  store::tests::adopted_session_sacrifice_returns_typed_retention_error \
  store::tests::request_adoptions_raw_update_and_delete_are_rejected \
  store::tests::execution_binding_survives_status_and_archive_updates_unchanged \
  store::tests::request_adoption_event_correlation_rejects_invalid_insert_and_rebind \
  api::tests::bound_operations_reject_every_mismatched_root_field \
  api::tests::bound_operations_reject_every_mismatched_parent_field \
  api::tests::bound_input_rejects_expired_exact_proof \
  api::tests::bound_input_expired_rejection_calls_no_runtime_or_event \
  api::tests::bound_kill_accepts_expired_exact_proof \
  api::tests::adopted_input_sequential_terminal_and_restart_replay_is_read_only \
  api::tests::adopted_launch_expired_committed_created_survives_reap_restart_and_replay \
  store::tests::request_adoptions_close_reopen_preserves_exact_record_and_session \
  store::tests::request_adoptions_each_same_key_identity_difference_conflicts \
  api::tests::adopted_launch_conflict_matrix_and_new_attempt \
  api::tests::adopted_input_global_key_conflicts_are_static_and_redacted \
  api::tests::adopted_input_precommit_denials_write_no_adoption \
  api::tests::bound_legacy_launch_requires_adoption_and_never_executes
# test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 5141 filtered out
```

A row's status is only as current as this run. A later change to any listed test, or to the code it exercises, requires a rerun before the row is cited again.

## 6. What would migrate an open row

- H6, H9 and H19 migrate with the #1629 O4 implementation and its tests, once O4 is removed from the API contract's exclusions.
- H10 needs an approved O5 design and the ADR 3 fence (psyche#19, Coven #804).
- H22 needs O7 and the real-daemon suite (#1230 remaining item 4).
- H2–H5, H7, H14, H15, H17 and H21 need executable canaries from their named non-Coven producers. Those canaries are recorded on psyche#12 or in the producer's repository. Coven adds no code for them.
- H1 closes only when every other row has migrated.

Each migration updates its row here, with the test name and a fresh run record, in a reviewed PR. Nothing in this document is updated by inference.

## 7. Known limits

- The historical specification at `9b07c500` is unavailable. Any guarantee that only that specification stated cannot be enumerated here, so this table cannot prove it is complete. ADR 2 makes S1–S3 the retained statement. A guarantee found later is added as a new `open` row.
- `psyche.identity_rebind.v1` in [TECH.md](./TECH.md#familiar-identity-rebind) is a current Psyche orchestration artifact, not a historical guarantee source. It is cited above as the design contract for Psyche-owned rows. It has no implementation on Psyche `main`.
- Coven's mapped rows are admission, storage and replay guarantees. None of them shows that a running process has stopped, that a route is blocked, or that a real daemon behaves the same under kill or stall.
