---
title: "Coven Automations v1 delivery roadmap"
summary: "Canonical Bead <-> GitHub outcome graph, priorities, dependencies, release gates, and drift controls for the Coven Automations v1 program (OpenCoven/coven#854, operationalized by OpenCoven/coven#859)."
read_when:
  - Working on any Coven Automations v1 P0/P1 outcome
  - Reconciling Beads state against GitHub outcomes
  - Running or interpreting the tracker drift check
description: "Delivery roadmap for Coven Automations v1: tracker roles, ownership, the P0/P1/P2 table, the Bead-GitHub mapping, the dependency graph, release gates, and active blockers."
---

# Coven Automations v1 delivery roadmap

_Last synchronized: 2026-09-01T18:40:00Z (see the sync metadata block below)_

> [!WARNING]
> **Generated content.** The mapping table in the marked block below is generated from
> `docs/roadmaps/coven-automations-v1.mapping.json` by
> `node docs/roadmaps/drift-check.mjs --render`. Edit the mapping, never the block.
> Mutable run status is deliberately **not** duplicated here: authoritative state lives
> in the trackers and is linked from this document.

## Program

**Coven Automations v1 — reliable, identity-bound familiar routines**
([OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854)).
Tracker operationalization control:
[OpenCoven/coven#859](https://github.com/OpenCoven/coven/issues/859).
Parent of the initial P0 graph (#816, #855, #856, #857, #858). The program owns release
gates and cross-repository rollup and must not be used as a catch-all implementation task.

## Canonical tracker roles

| Tracker | Owns | Must never own |
| --- | --- | --- |
| **Beads** (canonical store: `OpenCoven/coven-cave` embedded Dolt database `cave`) | implementation dependency graph; task/quest assignment and active execution ownership; current priority and blocked state; branch/worktree linkage; interaction/delivery evidence references; generated GitHub mirror synchronization inputs | public acceptance criteria, cross-repository issue links, or any role as a runtime ledger |
| **GitHub** | public outcome and rationale; canonical acceptance criteria and release gates; cross-repository issue links; durable PR/release/conformance evidence links; design/governance decisions | mutable execution state that belongs to the Coven runtime |
| **Coven runtime** | automation definitions and revisions; occurrences, runs, attempts, leases, approvals, artifacts, events, receipts | — tracker data is never queried as production automation state |

`.beads/issues.jsonl` in `OpenCoven/coven-cave` is a public-scrubbed review export —
never canonical state and never a hand-edited sync mechanism. Tracker changes land only
through reviewed PRs (see
[the #859 status/decision record](../superpowers/plans/2026-08-30-issue-859-coven-automations-v1-tracker-operationalization.md)).

## Sync metadata

- **Last synchronization:** 2026-09-01T18:40:00Z (UTC)
- **Source branch:** `docs/859-automations-v1-mapping` (based on upstream `main`)
- **Machine-readable mapping:** [`coven-automations-v1.mapping.json`](./coven-automations-v1.mapping.json) (schema `coven.automations-v1.tracker-mapping`, version 1)
- **Drift check:** `node docs/roadmaps/drift-check.mjs` (add `--beads-export <issues.jsonl>` to cross-check an export; `--strict` to fail on pending provisioning; `--selftest` verifies detection rules) — runs locally and in CI without ambient production credentials
- **Beads tool reference:** Beads 1.2.2 (Homebrew), live bead `schema_version` 1, verified during the
  [OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220) seed run
  (Cave PR OpenCoven/coven-cave#5277); pre/post
  embedded-Dolt commit OIDs are recorded in that Cave PR and the seed-run evidence (kept out of this
  file per the repository secret guard)
- **Writer:** exactly one canonical writer/process for this setup (Decision D1 in the
  #859 status/decision record); concurrent independent migrations and direct writes from
  unrelated worktrees are refused

## P0 / P1 / P2 policy

- **P0:** current v1 correctness, security, data-loss/duplicate-execution risk, authority violation, broken migration, or certification blocker.
- **P1:** committed SDK/product/docs/ecosystem work required to make the certified core usable and operable.
- **P2:** post-v1 expansion or research that must not silently enter the release critical path (event triggers, multi-host routing, hosted execution, broad external action adapters).

Every P0 bead must have: one accountable owner; one canonical GitHub outcome; explicit
dependencies; a current acceptance gate; an active or explicitly blocked disposition;
evidence requirements; and no contradictory closed public mirror.

## Outcome mapping

The table below is the canonical Bead ↔ GitHub mapping (also available as JSON):

<!-- BEGIN GENERATED:MAPPING-TABLE v1 -- regenerate with: node docs/roadmaps/drift-check.mjs --render (do not edit by hand) -->
| Outcome | GitHub | Priority | Bead label | Bead ID | Dependencies | Disposition |
| --- | --- | --- | --- | --- | --- | --- |
| program | [OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854) | P0 | `automations-v1/program` | cave-hlv.9 | (none) | active:release-gate-ownership |
| foundation | [OpenCoven/coven#816](https://github.com/OpenCoven/coven/issues/816) | P0 | `automations-v1/foundation` | cave-stsf7 | (none) | closed:verified-foundation |
| authority | [OpenCoven/coven#857](https://github.com/OpenCoven/coven/issues/857) | P0 | `automations-v1/authority` | cave-dbkng | foundation, protocol | blocked:pending-protocol-and-identity-authority-profiles (bead cave-dbkng) |
| certification | [OpenCoven/coven#858](https://github.com/OpenCoven/coven/issues/858) | P0 | `automations-v1/certification` | cave-x28j6 | protocol, scheduler, authority | blocked:pending-protocol-scheduler-trust (bead cave-x28j6) |
| protocol | [OpenCoven/coven#855](https://github.com/OpenCoven/coven/issues/855) | P0 | `automations-v1/protocol` | cave-tm1y0 | foundation | ready:foundation-verified (bead cave-tm1y0 unblocked in the Cave graph) |
| scheduler | [OpenCoven/coven#856](https://github.com/OpenCoven/coven/issues/856) | P0 | `automations-v1/scheduler` | cave-1sh6p | foundation, protocol | blocked:pending-protocol (bead cave-1sh6p; foundation verified) |

_Cross-repository child outcomes: none created yet. One Bead per SDK, Cave, Psyche, docs, organization-canary, Familiar Contract, and Threads outcome under the program is mapped here one-to-one as each is created._
<!-- END GENERATED:MAPPING-TABLE -->

Bead IDs are provisioned in Cave's canonical Beads/Dolt graph through
[OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220)
(Cave PR OpenCoven/coven-cave#5277):
every outcome above carries a live bead id, so `node docs/roadmaps/drift-check.mjs --strict`
reports no `W010`. Dependency direction was proven with `bd dep list` (both directions),
`bd dep cycles` (none), and `bd ready --json`.

## Dependency graph

Minimum canonical P0 graph (from OpenCoven/coven#859):

```text
#816 foundation
  ├─ #855 protocol
  ├─ #856 scheduler reliability
  └─ #857 identity + authority

#855 ─┬─> #856
      └─> #857

#855 + #856 + #857 -> #858 certification
#858 -> v1 release gate

#855 -> SDK read/types/changefeed          (P1)
#855 + #857 -> SDK mutations/approvals     (P1)
#855 + #856 + #857 -> Cave oversight/recovery (P1)
#855 + #857 -> Psyche adapter              (P1)
upstream Familiar/Threads profiles -> #857 (cross-repo, when created)
```

Exact dependencies are recorded per outcome in the mapping file
(`depends_on` slugs; `depends_on_external` for cross-repository outcomes). P1 ecosystem
beads must declare their exact dependencies rather than inheriting them from the broad
program parent.

## Release gates

1. **Foundation reconciled** — #816 evidence checklist complete (landed series linked,
   clean-clone test verification, migration/rollback proof, daemon wiring proof, unified
   manual/scheduled run path, stale-lease recovery, delivery-failure non-success,
   compatibility facade reconciled with
   [OpenCoven/coven-cave#4990](https://github.com/OpenCoven/coven-cave/issues/4990)).
2. **Protocol specified** — #855 schemas/state machines/idempotency/changefeed reviewed
   and test-covered.
3. **Scheduler hardened** — #856 time/retry/cancel/fencing/recovery behaviors proven,
   including no duplicate execution.
4. **Authority bound** — #857 principal/familiar/authority/approval/receipt binding with
   fail-closed negative tests.
5. **Certification** — #858 conformance/chaos/SLO/operator-diagnostics suites run without
   ambient production credentials; the v1 release gate report is generated from them.
6. **Program rollup** — the final #854 release rollup can be generated from reconciled
   tracker state and exact evidence; no P2 work has leaked onto the critical path.

## Active blockers

- **Remote Dolt propagation deferred** — the delivery beads and their `surface:shared`
  ownership are provisioned and dependency-verified in Cave's canonical embedded-Dolt graph
  (Cave PR OpenCoven/coven-cave#5277;
  local embedded-Dolt commit (recorded in Cave PR #5277), the durable source of truth). Only the
  cross-machine `pnpm beads:sync` push to `refs/dolt/data` is deferred: it is non-fast-forward
  (remote `04cb957…` is ahead from concurrent sessions) and `bd dolt pull` did not complete
  non-interactively. The remote was not force-pushed. No competing Beads database may be
  initialized in `OpenCoven/coven`.
- **Foundation reconciled (resolved)** —
  [#816](https://github.com/OpenCoven/coven/issues/816) is CLOSED and represented as
  verified-foundation (bead `cave-stsf7`, closed; PR
  [#896](https://github.com/OpenCoven/coven/pull/896); exact merge SHA recorded in Cave PR
  #5277). Downstream P0 workstreams are unblocked by it
  in the Cave graph.
- **Cross-repository profiles** — the Familiar Contract
  (OpenCoven/familiar-contract#17, bead
  `cave-6jswi`) and Threads
  (OpenCoven/coven-threads#29, bead
  `cave-m9tw3`) profile outcomes that #857 must depend on now exist and are recorded in the
  authority outcome's `depends_on_external`.

## Evidence and completion semantics

A bead may close only when the corresponding GitHub acceptance criteria are satisfied or
the outcome is explicitly cancelled/superseded with rationale. Required evidence includes,
as applicable: PR/merge commit and exact source revision; exact verification
commands/results; schema/vector/conformance artifact revisions; migration/rollback proof;
cross-repository canaries; security/privacy/authority impact; release artifact digest and
certification report; remaining known limitations. A GitHub issue is not closed merely
because a bead has no active assignee or a partial implementation landed.

## Drift detection

```sh
# verify the committed mapping, the generated roadmap block, and (optionally) an export
node docs/roadmaps/drift-check.mjs
node docs/roadmaps/drift-check.mjs --beads-export .beads/issues.jsonl   # coven-cave checkout
node docs/roadmaps/drift-check.mjs --strict                             # pending provisioning also fails
node docs/roadmaps/drift-check.mjs --selftest                           # verify detection rules
```

The check reports identifiers, statuses, priorities, links, and evidence references only,
requires no network or credentials, and flags: state disagreement (bead closed while the
GitHub outcome is open and vice versa), priority disagreement, P0 beads without an active
P0 outcome (owner/gate/disposition missing), outcomes without exactly one bead mapping,
unknown/ambiguous parent or dependency mappings, dependency cycles, completed work
lacking PR/test/release evidence, generated mirror bodies edited outside the generator
contract, and tracker output containing secrets or sensitive payloads. Severity policy:
`error` fails CI; `warning` (the pending-provisioning `W010`) is reported without failing
until provisioning is declared, after which the missing-mapping class escalates to `error`.
All outcomes are now provisioned, so `--strict` currently reports no findings.
