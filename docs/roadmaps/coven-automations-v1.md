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

_Last synchronized: 2026-08-30T15:05:00Z (see the sync metadata block below)_

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

- **Last synchronization:** 2026-08-30T15:05:00Z (UTC)
- **Source branch:** `agent/issue-859-p0-control-operationalize-coven-automations-v1` (based on upstream `main` at `1364cec`)
- **Machine-readable mapping:** [`coven-automations-v1.mapping.json`](./coven-automations-v1.mapping.json) (schema `coven.automations-v1.tracker-mapping`, version 1)
- **Drift check:** `node docs/roadmaps/drift-check.mjs` (add `--beads-export <issues.jsonl>` to cross-check an export; `--selftest` verifies detection rules) — runs locally and in CI without ambient production credentials
- **Beads tool reference:** Beads 1.2.2, schema v53, as recorded in
  [the v0.4.1 release program record](../superpowers/plans/2026-08-20-coven-v0.4.1-release-program.md);
  live schema/version verification and bead provisioning are owned by
  [OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220)
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
| program | [OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854) | P0 | `automations-v1/program` | (pending provisioning) | (none) | active:release-gate-ownership |
| foundation | [OpenCoven/coven#816](https://github.com/OpenCoven/coven/issues/816) | P0 | `automations-v1/foundation` | (pending provisioning) | (none) | active:reconciling-landed-evidence |
| authority | [OpenCoven/coven#857](https://github.com/OpenCoven/coven/issues/857) | P0 | `automations-v1/authority` | (pending provisioning) | foundation, protocol | blocked:pending-foundation-reconciliation-and-bead-provisioning |
| certification | [OpenCoven/coven#858](https://github.com/OpenCoven/coven/issues/858) | P0 | `automations-v1/certification` | (pending provisioning) | protocol, scheduler, authority | blocked:pending-p0-workstreams-and-bead-provisioning |
| protocol | [OpenCoven/coven#855](https://github.com/OpenCoven/coven/issues/855) | P0 | `automations-v1/protocol` | (pending provisioning) | foundation | blocked:pending-foundation-reconciliation-and-bead-provisioning |
| scheduler | [OpenCoven/coven#856](https://github.com/OpenCoven/coven/issues/856) | P0 | `automations-v1/scheduler` | (pending provisioning) | foundation, protocol | blocked:pending-foundation-reconciliation-and-bead-provisioning |

_Cross-repository child outcomes: none created yet. One Bead per SDK, Cave, Psyche, docs, organization-canary, Familiar Contract, and Threads outcome under the program is mapped here one-to-one as each is created._
<!-- END GENERATED:MAPPING-TABLE -->

Bead IDs are pending until provisioning lands through
[OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220) — the
mapping records the contract (`surface:shared`, exact GitHub links, one-to-one outcomes)
and the drift check reports the gap (`W010`) until IDs are declared.

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

- **Bead provisioning pending** — the Automations v1 delivery epic and its
  `surface:shared` beads do not exist yet in Cave's canonical Beads/Dolt graph;
  [OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220)
  owns creation, dependency verification (`bd dep list`, `bd ready --json`), bounded
  `pnpm beads:sync` evidence, and before/after `refs/dolt/data` OIDs. No competing Beads
  database may be initialized in `OpenCoven/coven`.
- **Foundation evidence reconciliation** —
  [#816](https://github.com/OpenCoven/coven/issues/816) implementation landed on main
  (2026-08-28) but its evidence checklist (clean-clone verification, migration proof,
  daemon wiring proof, run-path proof) is still open.
- **Cross-repository profiles** — the Familiar Contract and Threads profile outcomes that
  #857 must depend on do not exist yet; `depends_on_external` stays empty and explicit
  until they are created.

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
`error` fails CI; `warning` (currently the pending-provisioning `W010`) is reported
without failing until provisioning is declared, after which the missing-mapping class
escalates to `error`.
