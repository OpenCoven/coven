---
title: "Coven Automations v1 delivery roadmap"
summary: "Reviewed GitHub issue graph, ownership, dependencies, acceptance, and release evidence."
read_when:
  - Working on Coven Automations v1
  - Reconciling issue state and delivery evidence
  - Running the tracker drift check
source_adjacent_reason: "Owns the producer program dependency and acceptance graph next to its validator."
---

# Coven Automations v1 delivery roadmap

_Last synchronized: 2026-10-05. This is a reviewed GitHub snapshot, not live
runtime status or production certification._

The program is **OpenCoven/coven#854**. Each implementation or consumer outcome
has its own issue in the owning repository. #859 records historical tracker
operationalization; #1222 migrates the remaining local work and establishes
GitHub-only tracking.

## Ownership

| Owner | Authoritative state |
| --- | --- |
| GitHub issues | outcomes, priority, assignees, blockers, parent/child work, acceptance criteria, PR and delivery evidence |
| Coven claims and task worktrees | short-lived repository write coordination and isolated changes |
| Coven Rust runtime | definitions, revisions, occurrences, runs, attempts, leases, approvals, artifacts, events, and receipts |

Tracker state never authorizes a runtime action. Clients consume the versioned
runtime contract; they do not read the worklist as production lifecycle truth.

## Reviewed issue graph

The mapping table is generated from `coven-automations-v1.mapping.json` by
`node docs/roadmaps/drift-check.mjs --render`. Edit the mapping and regenerate;
do not edit generated rows by hand. Required membership is pinned in the
checker so removing an outcome cannot make the program appear complete.

<!-- BEGIN GENERATED:MAPPING-TABLE v1 -- regenerate with: node docs/roadmaps/drift-check.mjs --render (do not edit by hand) -->
| Outcome | GitHub | Priority | Dependencies | Disposition |
| --- | --- | --- | --- | --- |
| program | [OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854) | P0 | program-control, foundation, protocol, scheduler, familiar-embodiment, automation-authority, authority, certification, sdk, cave-oversight, psyche-adapter, documentation, organization-canaries | active:release-gate-ownership |
| program-control | [OpenCoven/coven#859](https://github.com/OpenCoven/coven/issues/859) | P0 | (none) | closed:accepted-github-outcome |
| foundation | [OpenCoven/coven#816](https://github.com/OpenCoven/coven/issues/816) | P0 | (none) | closed:accepted-github-outcome |
| protocol | [OpenCoven/coven#855](https://github.com/OpenCoven/coven/issues/855) | P0 | foundation | closed:accepted-github-outcome |
| scheduler | [OpenCoven/coven#856](https://github.com/OpenCoven/coven/issues/856) | P0 | foundation, protocol | closed:accepted-github-outcome |
| authority | [OpenCoven/coven#857](https://github.com/OpenCoven/coven/issues/857) | P0 | protocol, familiar-embodiment, automation-authority | open:acceptance-incomplete |
| certification | [OpenCoven/coven#858](https://github.com/OpenCoven/coven/issues/858) | P0 | protocol, scheduler, authority | open:acceptance-incomplete |
| familiar-embodiment | `OpenCoven/familiar-contract#17` | P0 | foundation | closed:accepted-github-outcome |
| automation-authority | `OpenCoven/coven-threads#29` | P0 | familiar-embodiment | closed:accepted-github-outcome |
| sdk | `OpenCoven/sdk#80` | P1 | certification | open:acceptance-incomplete |
| cave-oversight | `OpenCoven/coven-cave#5217` | P1 | certification | open:acceptance-incomplete |
| psyche-adapter | `OpenCoven/psyche#18` | P1 | certification | open:acceptance-incomplete |
| documentation | `OpenCoven/coven-docs#76` | P1 | certification | open:acceptance-incomplete |
| organization-canaries | `OpenCoven/.github#2` | P1 | program-control, certification | open:acceptance-incomplete |
<!-- END GENERATED:MAPPING-TABLE -->

The closed foundation, protocol, scheduler, identity-profile, and authority-profile
issues are delivered prerequisites. Their closure does not complete #857's
production authority integration, #1054's held commands, #1137's ordinary-chat
context admission, or #858's exact-artifact certification.

## Dependency and priority rules

- **P0:** correctness, authority, data-loss/duplicate-execution risk, migration,
  and certification blockers.
- **P1:** committed SDK, product, operator, documentation, and ecosystem outcomes
  required to make the certified core usable.
- **P2:** event/webhook triggers, multi-host routing, hosted execution, and broad
  external action adapters; they do not silently become release prerequisites.

Each P0 issue has an accountable owner, explicit prerequisites, an acceptance
gate, a current disposition, and delivery evidence requirements. Keep blocker
relationships on GitHub and the reviewed `depends_on` graph consistent.
Cross-repository consumers wait on certification; the release rollup waits on
all declared program outcomes. Closed historical control work is not a new
implementation blocker.

## Verification and refresh

```sh
node docs/roadmaps/drift-check.mjs --strict
node docs/roadmaps/drift-check.mjs --selftest
```

The checker validates issue identity, required program membership, duplicate
outcomes, acyclic prerequisites, P0 ownership/gates, closure evidence,
generated-table consistency, and privacy limits. It requires no network access
or production credentials. `--issues-export PATH` optionally compares a local
GitHub REST JSON array containing each declared issue's `number`, `html_url`,
and `state`. Pull-request entries are ignored. A missing or conflicting issue
snapshot fails the comparison.

Before final rollup, read the current issues and exact PR/test/release evidence,
refresh the reviewed mapping, and regenerate the table. A passing check of a
committed snapshot proves consistency of that snapshot, not live release
readiness.

## Release gates

1. Foundation evidence covers durable state, daemon wiring, migration/rollback,
   manual/scheduled execution, stale leases, and truthful delivery failure.
2. Protocol schemas, state machines, idempotency, and changefeed are executable.
3. Scheduler evidence covers time/DST, retry, cancel, fencing, recovery, and no
   duplicate execution.
4. #857 proves principal/familiar/authority/approval/receipt binding through the
   production path, with fail-closed negative cases.
5. #858 runs independent conformance, chaos, privacy/security, load/SLO, and
   operator diagnostics against the exact release artifact.
6. SDK, Cave, Psyche, docs, and organization canaries pin immutable evidence.
7. #854's release rollup reconciles those outcomes and excludes P2 expansion.

Current source gaps and the executable audit path are documented in
[Native Automations readiness](coven-automations-readiness.md). The wider
execution sequence remains in the critical-path plan; its dated source
snapshot is historical, while this issue graph has the current tracking role.
