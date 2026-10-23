---
title: "Bounded doctor receipt commitment inspection"
summary: "Reuse committed receipt validation without exposing evidence bodies or activating authority."
read_when:
  - Implementing safe Automations doctor receipt diagnostics
source_adjacent_reason: "Records the bounded read and evidence scope for issue 1355."
---

# Doctor Receipt Commitments

The doctor currently checks reciprocal receipt/run indexes but does not verify committed receipt bodies. Inspect the complete bounded receipt cohort using the existing Rust `read_receipt` validator: canonical receipt/event integrity, indexes, run/attempt identity and terminal correlation, and reciprocal commitment. Keep authority-sidecar/provider verification explicitly unproven. This is one required part of full issue #1355; all other doctor and #858 acceptance remains open.

1. Preserve behavioral RED cases for a genuine committed receipt, independent body/event/index/run/attempt tampering, malformed keys, row/materialization limits and limit restoration.
2. Add a private observation module. Collect at most 256 receipt keys, bound receipt/event body bytes for the entire cohort, and temporarily lower SQLite's string/blob/row length limit while reusing the validator. Restore the previous limit on all normal/error paths; never raise a stricter caller limit. Use the existing read snapshot and cooperative instruction/time budget. Do not claim a hard filesystem deadline or ratified SLO.
3. Emit only counts and fixed gap/finding codes. Keep missing prerequisites, invalid metadata, exhausted budgets, inconsistent commitments and authority verification distinct. Preserve independently available delivery/link facts. Add closed DTO validation and minimization regressions.
4. Exercise genuine terminal fixtures through owner-local daemon/CLI, including complete durable before/after equality. Keep TCP refusal, no autostart, no writable opener, migration, checkpoint or recovery side effect. Update API/operator scope and runbook.
5. Run required local guards, independent source review, signed/DCO commit, clean-source audit, exact-head CI, protected landing, fetched-main proof and owned retirement. Do not close #1355 or #858 before their original complete acceptance is proven.
