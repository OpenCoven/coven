# Coven Automations v1 Program Status Record — 2026-08-30

**Type:** status/decision record (verified facts; no task plan)
**Subject:** OpenCoven/coven issue #854 — Program: Coven Automations v1
**Evidence snapshot:** upstream `main` at `1364cec9dbaf1e2aca2e4544dec0e1ce807d859c` (2026-08-30), inspected locally; GitHub state read via REST on 2026-08-30 ~15:03–15:20 UTC
**Deconfliction:** no open PR references #854 upstream, and no `agent/*854*` branch exists on the CompleteDotTech/coven fork (checked 2026-08-30 ~15:08 UTC)

---

## Verdict

**Coven Automations v1 is not satisfied on `main`. The local durable-scheduler foundation has landed and is independently confirmed in code; the v1 protocol, authority, conformance, SDK, and tracker-control work has not started.** The issue's own "foundation-ready, not yet v1-certified" assessment (final implementation assessment, 2026-08-30) matches the code; every P0 child issue (#855–#859) was opened on 2026-08-30 and is open with no landed work yet.

- Definition of done status: **not met** (no P0 gate is implemented and evidenced end-to-end; no release candidate exists).
- #854 must remain open: this record closes nothing. The one delivered artifact is this record itself, which unblocks #816's evidence-closure item and #859's mapping task.

## What exists on `main` today (evidence)

The `coven#816` automations series landed 2026-08-28 (PR #846 merged 2026-08-28T14:22:30Z; PR #847 merged 2026-08-28T19:55:52Z; parts 6–8 commits dated 2026-08-28 arrived via consolidated merges `52c3d81` 2026-08-29 and `1364cec` 2026-08-30). All of the following was verified by direct inspection of `main` at `1364cec`:

| Foundation element | Evidence on main |
| --- | --- |
| Versioned Coven-owned routine definitions | `crates/coven-cli/src/automations/definition.rs` (217 lines; introduced in `882fc83`, PR #846) |
| SQLite definition / occurrence / run records | `automations/store.rs:14` (`automation_definitions`), `automations/occurrences.rs:20` (`automation_occurrences`), `automations/runs.rs:13` (`automation_runs`) |
| RRULE-backed daily/weekly planning | `automations/rrule.rs` (180 lines, 8 unit tests), `automations/schedule.rs` (178 lines, 6 unit tests) |
| Unique occurrence fencing | `automations/occurrences.rs:31` — `UNIQUE(automation_id, scheduled_for)`; planning is idempotent |
| Claim leases and expiry/recovery | `automations/health.rs:23-24,79-106` (`lease_owner`, `lease_expires_at`, `stale_reason`) |
| Latest-only misfire, overlap refusal | defaults `misfire: "latest"`, `overlap: "forbid"` (`definition.rs:61-62,159-160`; enforced in `daemon_tick.rs:69-70`) |
| Daemon-side recurring tick + scheduled dispatch | `automations/daemon_tick.rs:33-51` (thread `coven-automations-scheduler`, fixed 60s cadence), wired at `crates/coven-cli/src/daemon.rs:4285`; shared launch path in `automations/runner.rs` (415 lines) |
| Familiar ID propagation, bounded logs, atomic delivery | `definition.rs:68` (`familiar_id: Option<String>`), `runner.rs` |
| Health + run-history projections | `automations/health.rs` (203 lines), `automations/runs.rs` (307 lines) |
| Non-destructive paused legacy import | `automations/import_legacy.rs` (249 lines; reads `~/.codex/automations/<id>/automation.toml`, imports PAUSED, never modifies sources; PR #847 merged 2026-08-28T19:55:52Z) |
| `coven.automations.*` control actions | `crates/coven-cli/src/control_plane.rs:103-118` — capability domain `coven.automations` with 10 actions (`list`, `get`, `create`, `update`, `delete`, `tick`, `runs`, `run`, `import`, `health`); API-level tests in `crates/coven-cli/src/api.rs` (~lines 10639–10845) |

Module size: `crates/coven-cli/src/automations/` is 11 files / 2,719 lines with 43 unit tests (per-file `#[test]` counts summed); exercised further by `crates/coven-cli/src/api.rs` integration tests. Cave-side ownership migration is reported in OpenCoven/coven-cave#4990 (per #816's body; not independently verified in this repo).

### What is absent on `main` (verified)

- No automations spec under `specs/` (12 spec directories, none for automations) and no `coven.automations.v1` schema, state-machine, typed-error, idempotency, or changefeed contract anywhere on main → #855.
- No automations documentation under `docs/` (grep for "automations" returns nothing) → coven-docs#76.
- No automation surface in the npm SDK `npm/coven/src` (no matches) → sdk#80.
- No Beads store in this repo (`.beads` absent) and no live-Dolt mutation yet → #859 (its 2026-08-30 comment states mutation of Cave's embedded-Dolt Beads graph is "not yet completed").
- No conformance, chaos, load/SLO, or release-receipt gate → #858.
- No principal/capability/approval/receipt binding: `familiar_id` is an optional unversioned string validated only for length (`definition.rs:68,131-134`); `automation_runs` carries no authority evidence → #857 (+ familiar-contract#17, coven-threads#29, cross-repo).
- Scheduler cadence is a fixed wall-clock `thread::sleep(60s)` loop (`daemon_tick.rs:35-50`) with no virtual-time, DST-transition, clock-jump, or leader-fencing contract → #856.
- A routine remains a schedule + familiar-bound prompt (`definition.rs:4`), not a trigger/condition/authorized-action model.

## Program issue family (REST state, 2026-08-30)

| Issue | State | Created | Evidence note |
| --- | --- | --- | --- |
| #854 program | open | 2026-08-30T13:36:04Z | 1 comment: BunsDev operationalization checkpoint (14:06:20Z) |
| #816 foundation | open | 2026-08-24T15:32:52Z | body records foundation "materially landed"; closure blocked on an evidence checklist (updated 2026-08-30T13:54:46Z) |
| #855 protocol schemas | open | 2026-08-30T13:37:23Z | no activity |
| #856 time/fencing/crash hardening | open | 2026-08-30T13:38:31Z | no activity |
| #857 authority binding + receipts | open | 2026-08-30T13:39:54Z | 1 design comment (receipt replay resistance, 14:23:31Z) |
| #858 conformance/chaos/SLO | open | 2026-08-30T13:40:55Z | no activity |
| #859 Beads/GitHub mirrors | open | 2026-08-30T13:41:50Z | 1 comment routing the graph to Cave's embedded Dolt DB (`cave-hlv` epic; coven-cave#5219 roadmap PR; coven-cave#5220 seed/verification task) |

Cross-repo outcomes cited by the #854 checkpoint comment (not independently verified in this sweep): P0 — OpenCoven/familiar-contract#17, OpenCoven/coven-threads#29; P1 — OpenCoven/sdk#80, OpenCoven/coven-cave#5217, OpenCoven/psyche#18, OpenCoven/coven-docs#76, OpenCoven/.github#2. Zero PRs are open upstream at snapshot time; no PR implements any of #855–#859 yet.

## Verdict against the issue's gates

- **Gate A (durable local scheduler):** partially met — deterministic planning, unique occurrence fencing, bounded leases, latest-only misfire, overlap refusal, 60s daemon tick, and 43 module unit tests exist; DST/virtual-time/clock-jump/restart-convergence certification does not (#856 open).
- **Gate B (identity and authority):** not met — optional string `familiar_id` only; no principal authorization, capability grants, approval path, or exercised-authority receipts on any run record (#857, familiar-contract#17, coven-threads#29).
- **Gate C (public contract):** not met — the wire contract lives in Rust structs (`definition.rs`) with no independent versioned schemas or golden vectors; the SDK has no automation surface (#855, sdk#80).
- **Gate D (operations):** partial — health snapshot and run-history projections exist and are CLI/API-observable; chaos/restart certification, load/SLO evidence, alerts/retention/redaction exercise, and a machine-readable release receipt do not exist (#858 open).
- **Tracker (Beads/GitHub graph):** not started — the canonical graph lives in Cave's Dolt database; the seeding/verification task (coven-cave#5220) has not been executed (#859 open).

## Critical path

The #854 checkpoint comment (2026-08-30T14:06:20Z) fixes the engineering sequence, which matches the issue's Beads dependency rules and this record's code findings:

1. coven-cave#5220 / #859 — seed and verify the Automations v1 delivery epic in Cave's embedded-Dolt Beads graph (first executable action).
2. #816 — attach landed-series evidence (PRs #846/#847 + parts 6–8 commits, clean-clone test run, migration and daemon/restart verification), then close #816 as the landed foundation.
3. #855 — versioned `coven.automations.v1` schemas, state machines, idempotency, typed errors, changefeed.
4. In parallel: #856 (deterministic time, DST, retries, cancellation, fencing, crash recovery) + familiar-contract#17 + coven-threads#29.
5. #857 — dispatch-time principal/familiar/authority/runtime/approval binding and receipts.
6. #858 — conformance, chaos, security/privacy, load/SLO, operator diagnostics.
7. P1 consumption — sdk#80, coven-cave#5217 (Cave oversight), psyche#18, coven-docs#76, .github#2.
8. Exact-release go/no-go packet (release receipt, certification).

## Decision

- Do **not** close #854, #816, #855–#859. #854 is correctly decomposed; its verdict ("foundation-ready, not yet v1-certified") is independently confirmed by the code inspection above.
- Unattended external side effects stay out of scope until #857 and #858 pass at exact immutable artifacts (per the #854 safety gate).
- Next executable actions: coven-cave#5220 (live Beads seeding) and #816 evidence closure; both precede any #855 contract work.

## Sources

- Code: `crates/coven-cli/src/automations/` at `1364cec` (files, tests, and line refs as cited above); `crates/coven-cli/src/control_plane.rs:103-116`; `crates/coven-cli/src/daemon.rs:4285`.
- History: commits `882fc83` (part 1, PR #846, merged 2026-08-28T14:22:30Z), `39b8618` (part 5, PR #847, merged 2026-08-28T19:55:52Z), `bd3b47d`/`a4a71af`/`1de50a8` (parts 6–8, 2026-08-28), `52c3d81` (2026-08-29), `1364cec` (2026-08-30).
- Issues (all read 2026-08-30 via REST): OpenCoven/coven#854, #816, #855, #856, #857, #858, #859; cross-repo train per the #854 checkpoint comment (familiar-contract#17, coven-threads#29, sdk#80, coven-cave#5217/#5219/#5220, psyche#18, coven-docs#76, .github#2).
