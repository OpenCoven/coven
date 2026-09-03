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

_Last synchronized: 2026-09-03T06:09:12Z (see the sync metadata block below)_

> [!WARNING]
> **Generated content.** The mapping table in the marked block below is generated from
> `docs/roadmaps/coven-automations-v1.mapping.json` by
> `node docs/roadmaps/drift-check.mjs --render`. Edit the mapping, never the block.
> Mutable run status is deliberately **not** duplicated here: authoritative state lives
> in the trackers and is linked from this document.

## Program

**Coven Automations v1 — reliable, identity-bound familiar routines**
([OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854)) — the release
rollup outcome, bound one-to-one to Cave bead `cave-hlv.9`.
Tracker operationalization control:
[OpenCoven/coven#859](https://github.com/OpenCoven/coven/issues/859) — the completed
program-control outcome, bound one-to-one to closed Cave bead `cave-hlv.10`. #854 owns
release gates and cross-repository rollup; #859 established tracker reconciliation, drift
control, and evidence semantics. Neither is an implementation catch-all.
Parent of the initial P0 graph (#816, #855, #856, #857, #858) plus seven cross-repository
children (Familiar Contract, Threads, SDK, Cave, Psyche, docs, organization canaries).

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

- **Last synchronization:** 2026-09-03T06:09:12Z (UTC)
- **Source branch:** `docs/coven-automations-critical-path` (based on upstream `main`)
- **Machine-readable mapping:** [`coven-automations-v1.mapping.json`](./coven-automations-v1.mapping.json) (schema `coven.automations-v1.tracker-mapping`, version 1)
- **Drift check:** `node docs/roadmaps/drift-check.mjs` (add `--beads-export <issues.jsonl>` to cross-check an export; `--strict` to fail on pending provisioning; `--selftest` verifies detection rules) — runs locally and in CI without ambient production credentials
- **Beads tool reference:** checksum-verified Beads 1.3.0-rc.1, required for the live
  schema-v66 Cave store; Homebrew Beads 1.2.2 is schema-v53-only and was not used for
  writes. The graph was seeded through
  [OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220)
  (Cave PRs OpenCoven/coven-cave#5277 and OpenCoven/coven-cave#5278); pre/post
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
| program | [OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854) | P0 | `automations-v1/program` | cave-hlv.9 | program-control, foundation, protocol, scheduler, familiar-embodiment, automation-authority, authority, certification, sdk, cave-oversight, psyche-adapter, documentation, organization-canaries | active:release-gate-ownership |
| program-control | [OpenCoven/coven#859](https://github.com/OpenCoven/coven/issues/859) | P0 | `automations-v1/program-control` | cave-hlv.10 | (none) | closed:tracker-operationalized |
| foundation | [OpenCoven/coven#816](https://github.com/OpenCoven/coven/issues/816) | P0 | `automations-v1/foundation` | cave-stsf7 | (none) | closed:verified-foundation |
| authority | [OpenCoven/coven#857](https://github.com/OpenCoven/coven/issues/857) | P0 | `automations-v1/authority` | cave-dbkng | protocol, familiar-embodiment, automation-authority | blocked:pending-protocol-and-identity-authority-profiles (bead cave-dbkng) |
| certification | [OpenCoven/coven#858](https://github.com/OpenCoven/coven/issues/858) | P0 | `automations-v1/certification` | cave-x28j6 | protocol, scheduler, authority | blocked:pending-protocol-scheduler-trust (bead cave-x28j6) |
| protocol | [OpenCoven/coven#855](https://github.com/OpenCoven/coven/issues/855) | P0 | `automations-v1/protocol` | cave-tm1y0 | foundation | ready:foundation-verified (bead cave-tm1y0 unblocked in the Cave graph) |
| scheduler | [OpenCoven/coven#856](https://github.com/OpenCoven/coven/issues/856) | P0 | `automations-v1/scheduler` | cave-1sh6p | foundation, protocol | blocked:pending-protocol (bead cave-1sh6p; foundation verified) |

_Cross-repository child outcomes (7), mapped one-to-one in the same canonical Cave Beads graph. Each carries a live bead id, an acceptance gate, a disposition, and an evidence list that stays empty until exact PR/test/release references exist._

| Outcome | GitHub | Priority | Bead label | Bead ID | Dependencies | Disposition |
| --- | --- | --- | --- | --- | --- | --- |
| automation-authority | `OpenCoven/coven-threads#29` | P0 | `automations-v1/automation-authority` | cave-m9tw3 | familiar-embodiment | blocked:pending-familiar-embodiment (bead cave-m9tw3) |
| familiar-embodiment | `OpenCoven/familiar-contract#17` | P0 | `automations-v1/familiar-embodiment` | cave-6jswi | foundation | ready:foundation-verified (bead cave-6jswi unblocked in the Cave graph) |
| cave-oversight | `OpenCoven/coven-cave#5217` | P1 | `automations-v1/cave-oversight` | cave-e52qp | certification | blocked:pending-certification (bead cave-e52qp) |
| documentation | `OpenCoven/coven-docs#76` | P1 | `automations-v1/documentation` | cave-qwnxq | certification | blocked:pending-certification (bead cave-qwnxq) |
| organization-canaries | `OpenCoven/.github#2` | P1 | `automations-v1/organization-canaries` | cave-xqbs4 | program-control, certification | blocked:pending-certification (program control closed; bead cave-xqbs4) |
| psyche-adapter | `OpenCoven/psyche#18` | P1 | `automations-v1/psyche-adapter` | cave-yaul2 | certification | blocked:pending-certification (bead cave-yaul2) |
| sdk | `OpenCoven/sdk#80` | P1 | `automations-v1/sdk` | cave-90hwl | certification | blocked:pending-certification (bead cave-90hwl) |
<!-- END GENERATED:MAPPING-TABLE -->

Bead IDs are provisioned in Cave's canonical Beads/Dolt graph through
[OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220)
(Cave PR OpenCoven/coven-cave#5277):
all 14 rows above — seven in-repository outcomes and seven cross-repository children —
carry a live bead id, so `node docs/roadmaps/drift-check.mjs --strict`
reports no `W010`. Dependency direction is proven with `bd dep list` (both directions),
`bd dep cycles` (none), and `bd ready --json`: 30 blocked-first edges across the 15
canonical beads (the 14 rows above plus the `OpenCoven/coven-cave#5220` seed bead
`cave-tmegk`). `cave-hlv.9` and `cave-hlv.10` additionally carry a pre-existing
parent-epic edge to the closed Cave epic `cave-hlv`, which is outside the Automations v1
program set and is not a v1 release blocker; it is recorded in each row's
`depends_on_external`.

The mapping's membership is pinned inside the checker, not inside the file it checks:
`drift-check.mjs` fails with `E011` if the `#859` program-control outcome or any of the
seven cross-repository children is deleted, emptied, or filed in the wrong section.

## Dependency graph

Live blocked-first graph, as verified in Cave's canonical Beads/Dolt store
(`bd dep list --direction down`; each arrow means "left must complete before right"):

```text
#816 foundation (closed, verified)
  ├─> #855 protocol
  ├─> #856 scheduler reliability
  └─> familiar-contract#17 familiar embodiment

#855 protocol ─┬─> #856 scheduler reliability
               ├─> #857 authority
               └─> #858 certification

familiar-contract#17 ─┬─> coven-threads#29 automation authority
                      └─> #857 authority
coven-threads#29 ─────> #857 authority

#855 + #856 + #857 ──> #858 certification

#858 certification ─┬─> sdk#80                    (P1)
                    ├─> coven-cave#5217           (P1)
                    ├─> psyche#18                 (P1)
                    ├─> coven-docs#76             (P1)
                    └─> .github#2                 (P1)

#859 program control ─┬─> .github#2               (P1)
                      └─> #854 release rollup

#816, #855, #856, #857, #858, familiar-contract#17, coven-threads#29,
sdk#80, coven-cave#5217, psyche#18, coven-docs#76, .github#2, #859
                      ──> #854 release rollup (13 in-program edges)
```

P2 expansion (event triggers, multi-host routing, hosted execution, broad external action
adapters) has no edge into this graph.

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

- **Remote Dolt propagation (resolved)** — the delivery beads and their `surface:shared`
  ownership are provisioned and dependency-verified in Cave's canonical embedded-Dolt graph
  (Cave PR OpenCoven/coven-cave#5277). Cross-machine propagation is now durable: the documented
  wrapper `pnpm beads:sync` completed pull+push (exit 0, ~24s) on 2026-09-01, local embedded Dolt
  `main` and `remotes/origin/main` are equal, `dolt diff --summary` is empty, and the shared remote
  `refs/dolt/data` carries the seeded 15-bead / 30-edge graph. The remote was not force-pushed, no
  concurrent Dolt commit was discarded, and local state was re-verified intact afterward. Exact
  transient OIDs live in the seed-run execution ledger and Cave PR #5277, not in this file. No
  competing Beads database may be initialized in `OpenCoven/coven`.
- **Tracker operationalization (resolved)** — Cave PR OpenCoven/coven-cave#5278
  reconciled the terminal Cave mapping, OpenCoven/coven-cave#5220 and seed bead
  `cave-tmegk` are closed, Coven PRs #900 and #901 carry the public mapping and drift
  controls, and program-control #859 / bead `cave-hlv.10` closed on 2026-09-03 after a
  fresh schema-v66 read and durable `pnpm beads:sync`.
- **Foundation reconciled (resolved)** —
  [#816](https://github.com/OpenCoven/coven/issues/816) is CLOSED and represented as
  verified-foundation (bead `cave-stsf7`, closed; PR
  [#896](https://github.com/OpenCoven/coven/pull/896); exact merge SHA recorded in Cave PR
  #5277). Downstream P0 workstreams are unblocked by it
  in the Cave graph.
- **Cross-repository profiles** — the Familiar Contract
  (`OpenCoven/familiar-contract#17`, bead `cave-6jswi`) and Threads
  (`OpenCoven/coven-threads#29`, bead `cave-m9tw3`) profile outcomes that #857 depends on
  exist, are provisioned, and are mapped here as first-class cross-repository children
  rather than untyped external references. All seven cross-repository children
  (`OpenCoven/familiar-contract#17`, `OpenCoven/coven-threads#29`, `OpenCoven/sdk#80`,
  `OpenCoven/coven-cave#5217`, `OpenCoven/psyche#18`, `OpenCoven/coven-docs#76`,
  `OpenCoven/.github#2`) are listed in the generated table above with their exact bead
  ids, priorities, dependencies, dispositions, and evidence semantics. They are cited by
  fully-qualified `owner/repo#number` reference rather than absolute URL, which is this
  tracker's cross-repository citation convention and keeps the repository secret guard
  (`scripts/check-secrets.py`) clean.

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
one bead claimed by two outcomes, missing required program members (`E011` — the `#859`
program-control binding and the seven cross-repository children are pinned inside the
checker so a row cannot be deleted into compliance), unknown/ambiguous parent or dependency
mappings, dependency cycles, completed work lacking PR/test/release evidence, generated
mirror bodies edited outside the generator contract, and tracker output containing secrets
or sensitive payloads. Severity policy: `error` fails CI in both default and `--strict`
mode; `warning` (the pending-provisioning `W010`, which covers cross-repository children
as well as in-repository outcomes) is reported without failing until provisioning is
declared, after which the missing-mapping class escalates to `error`.
All 14 mapped rows are provisioned, so `--strict` currently reports no findings.
