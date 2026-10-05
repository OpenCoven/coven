---
title: "Work tracking"
summary: "GitHub issue ownership, dependencies, delivery evidence, and the local tracker migration receipt."
read_when:
  - Choosing or claiming work
  - Reconciling remaining tasks and delivery evidence
source_adjacent_reason: "Defines repository coordination and the migration receipt alongside the program validator."
---

# Work tracking

GitHub issues are Coven's sole work tracker. Keep priority, assignees, blockers,
parent/child outcomes, acceptance criteria, and PR/test/release evidence on the
issue in the owning repository. Inspect the issue and existing PRs before
creating work. Enter an isolated task worktree and acquire the shared
`coven claim acquire issue-<N>` token before editing; the claim coordinates
repository writes and does not replace durable issue ownership.

Use native GitHub blocker/sub-issue relationships and explicit issue links.
Close an outcome only after its acceptance evidence is recorded, or with an
explicit superseded/cancelled disposition. Passing CI or an expired claim is
not sufficient proof of delivery. Runtime definitions, occurrences, runs,
attempts, approvals, and receipts remain Coven-owned production state.

The Automations graph is a reviewed GitHub snapshot in
`roadmaps/coven-automations-v1.mapping.json`. Validate it with
`node docs/roadmaps/drift-check.mjs --strict` and `--selftest`; optionally compare
a local GitHub REST issue-array snapshot with `--issues-export PATH`.

## Migration receipt (2026-10-05)

#1222 owns this migration. All 24 previously non-closed local records have a
GitHub disposition: 21 new issues and three reused documentation issues.
The 109-record database, audit history, configuration, source export, and
migration mapping are retained in a private recovery archive outside the
project. Historical closed records are archived rather than recreated as work.

| Remaining record scope | GitHub issue | Disposition |
| --- | --- | --- |
| [BUG] GitHub Release creation fails: gh rejects --notes-from-tag with --repo | #1223 | Closed: repaired by #848 and published v0.4.1 release |
| Build and certify the clean v0.4.1 release candidate | #1224 | Open: retained scope and acceptance; revalidate dated observations |
| Produce authenticated three-account certification packet | #1225 | Open: retained scope and acceptance; revalidate dated observations |
| Release v0.4.1 with certified three-harness onboarding | #1226 | Open: retained scope and acceptance; revalidate dated observations |
| [BUG] Windows Rust suite fails no-op PRs repeatedly | #1227 | Open: retained scope and acceptance; revalidate dated observations |
| [BUG] setup_cli flakes 3-4 of 42 under parallel load | #1228 | Open: retained scope and acceptance; revalidate dated observations |
| [BUG] publish-npm-test.mjs hangs on macos-arm64 in the onboarding smoke | #1229 | Open: retained scope and acceptance; revalidate dated observations |
| Remove duplicate local public docs | #776 | Closed: accepted existing documentation outcome |
| Progressive documentation and E2E experience | #670 | Closed: accepted existing documentation outcome |
| Prove real-Coven authority conformance before Psyche session integration | #1230 | Open: retained scope and acceptance; revalidate dated observations |
| Prove Telegram parity, migrate safely, and release Psyche | #1231 | Open: retained scope and acceptance; revalidate dated observations |
| Complete Telegram media, interactions, and operator action parity | #1232 | Open: retained scope and acceptance; revalidate dated observations |
| Harden Psyche reliability, security, and operations | #1233 | Open: retained scope and acceptance; revalidate dated observations |
| Implement Telegram delivery, formatting, replies, and streaming previews | #1234 | Open: retained scope and acceptance; revalidate dated observations |
| Implement typed Telegram transports and durable ingress | #1235 | Open: retained scope and acceptance; revalidate dated observations |
| Implement fail-closed Telegram access, routing, and command semantics | #1236 | Open: retained scope and acceptance; revalidate dated observations |
| Bind Psyche identity and conversations to Coven-native sessions | #1237 | Open: retained scope and acceptance; revalidate dated observations |
| Build Psyche: Coven-native Rust agent with comprehensive Telegram support | #1238 | Open: retained scope and acceptance; revalidate dated observations |
| Extend three-harness parity coverage to Windows | #1239 | Open: retained scope and acceptance; revalidate dated observations |
| [BUG] Sub-100ms timing budgets passed into code under test flake under load | #1240 | Open: retained scope and acceptance; revalidate dated observations |
| Define comprehensive integration certification | #779 | Closed: accepted existing documentation outcome |
| Adopt the threads-55s channel fix once it lands upstream | #1241 | Open: retained scope and acceptance; revalidate dated observations |
| Wire coven-memory promotion through the threads-core admission gate | #1242 | Open: retained scope and acceptance; revalidate dated observations |
| Bump coven-threads-core dependency pin past threads-76z fix | #1243 | Open: retained scope and acceptance; revalidate dated observations |

The v0.4.1 certification and RC convergence issues retain their explicit
**waived, not satisfied** evidence. Historical Psyche implementation tasks
require reconciliation with the current owning-repository roadmap and keep
all prior approval/conformance gates. Migration does not authorize production
implementation or turn old observations into current proof.

## AgentFS work attribution

AgentFS create requests, session bindings, and operation provenance now use
optional `issueRef`, such as `OpenCoven/coven#684`. Commit materialization emits
`Coven-Issue`. This is attribution metadata; it grants no runtime authority.

An existing writable delta upgrades its retired work-reference column and
index atomically on the next metadata write, preserving recorded values and
filesystem data. Read-only inspection can still return historical attribution
without modifying the database. Old opaque references remain historical
identifiers; new writes should use the canonical GitHub issue reference.
