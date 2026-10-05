---
source_adjacent_reason: "Retains producer-owned contracts, acceptance, or historical evidence with current GitHub issue tracking."
---

# Automations program tracking and delivery evidence

The prior tracking procedure is superseded by #1222. GitHub issues in each
owning repository now hold outcomes, assignees, dependency relationships,
acceptance criteria, and delivery evidence. The original decision record is
retained in Git history at
`b5ea52c7:docs/superpowers/plans/2026-08-30-issue-859-coven-automations-v1-tracker-operationalization.md`.

#859 records the accepted historical operationalization work. #854 remains
the release rollup; implementation outcomes remain separate issues. The
reviewed program snapshot is `docs/roadmaps/coven-automations-v1.mapping.json`.

Before editing, inspect the issue and open PRs, create a task worktree, and
acquire the issue-keyed Coven claim. Record blockers with GitHub issue
relationships and links. Preserve exact PR, test, artifact, and release
references before closing an outcome. A closed prerequisite profile is not
proof that the production consumer or final release has been certified.

Validate the committed graph and generated roadmap with:

```sh
node docs/roadmaps/drift-check.mjs --strict
node docs/roadmaps/drift-check.mjs --selftest
```

The checker can compare a local GitHub REST JSON issue-array snapshot through
`--issues-export PATH`. It requires no network access or production credentials.
It enforces issue identity, program membership, dependencies, owner and gate
metadata, closure evidence, generated-table consistency, and privacy limits.

Tracker data never becomes runtime authority. Coven owns definitions,
occurrences, runs, attempts, leases, approvals, events, artifacts, and receipts.
