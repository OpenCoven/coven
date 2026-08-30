# Issue #807 Status Record — Shipped Reliability, Recovery, and Usefulness Scorecard

> **For agentic workers:** This is a dated status/decision record, not an
> implementation plan. It documents what exists on `main` relative to
> [OpenCoven/coven#807](https://github.com/OpenCoven/coven/issues/807) at the
> recorded base SHA. Facts and evidence links only; it proposes no SLOs and
> quotes no measurements.

**Date:** 2026-08-30
**Issue:** [#807 — P1: establish Coven shipped reliability, recovery, and usefulness scorecard](https://github.com/OpenCoven/coven/issues/807) (open; created 2026-08-24; 0 comments; no labels)
**Base inspected:** `main` @ [`1364cec`](https://github.com/OpenCoven/coven/commit/1364cec9) ("chore: preserve consolidated branch ancestry", 2026-08-30 06:31 -0500)
**Related issues at inspection:** [#805](https://github.com/OpenCoven/coven/issues/805) (P0 exact-commit release governance — **open**), [#779](https://github.com/OpenCoven/coven/issues/779) (installed-artifact E2E certification — **open**)
**Method:** REST-only GitHub API (search + pulls + branches for deconfliction; issues, releases, release-by-tag lookups) plus local inspection of the clone at the base SHA. No PR existed for #807 and no fork branch referenced it at inspection time (search/issues, `pulls?state=open`, `branches?per_page=100`, 2026-08-30). No benchmarks were executed for this record; every statement below is an existence/capability statement about code and docs on `main`, not a measurement.

---

## What exists on main today

### 1. Non-gating benchmark/trend corpus (the "strong test corpus" the issue refers to)

| Instrument | What it covers | Evidence |
| --- | --- | --- |
| `scripts/benchmark-cli.mjs` | Command startup, cold daemon start-to-health, session-listing, event-tail, harness-first-output timings; per-run min/median/p95/max; disposable `COVEN_HOME`s, fake Codex fixture, env redaction. Last touched 2026-08-11. | [`scripts/benchmark-cli.mjs`](https://github.com/OpenCoven/coven/blob/1364cec9/scripts/benchmark-cli.mjs); [README §"CLI performance baselines"](https://github.com/OpenCoven/coven/blob/1364cec9/README.md) (lines ~702–721) |
| `scripts/benchmark-chaos.mjs` (report schema v3) | 1/8/32 concurrent deterministic sessions; launch-to-first-output percentiles; throughput; cancellation-to-terminal latency; SQLite file growth; writer connection/transaction deltas; sampled writer backlog; sampled daemon RSS via `coven pc top --json` (no process names/command lines retained); deterministic equivalents for free-disk watermark, SQLite lock/retry, persisted-session crash recovery. Last touched 2026-08-08. | [README §"Concurrent runtime baseline"](https://github.com/OpenCoven/coven/blob/1364cec9/README.md) (lines ~723–756); [`scripts/benchmark-chaos.mjs`](https://github.com/OpenCoven/coven/blob/1364cec9/scripts/benchmark-chaos.mjs) |
| CI collection (non-gating) | Both collectors run in the `performance-baseline` CI job with `continue-on-error: true` (ci.yml lines 248, 281); artifacts uploaded, no wall-clock gate. The deterministic fixture tests (`benchmark-cli.test.mjs`, `benchmark-chaos.test.mjs`) do gate. | [`.github/workflows/ci.yml`](https://github.com/OpenCoven/coven/blob/1364cec9/.github/workflows/ci.yml) |
| Deterministic Rust metric test | Ignored test `benchmark_schedule_metrics_emit_json` prints deterministic TUI poll/draw counters. | README lines ~709, 716–717 |

The README already states the separation the issue demands: outputs are
"trend data", benchmark p50/p95/p99 "do not replace that product-level
timeout" (the Cave managed-start 8-second deadline), and chaos coverage
entries "remain separate from trend measurements, so a timing artifact cannot
be mistaken for a passing failure-path test".

**Relevance to #807 and gap:** these are **benchmark condition/input**
instruments. They are per-run JSON artifacts — no history is retained on
`main`, no multi-run trend table exists, and the default sample count is 3
iterations (`--iterations 3`), which the issue's non-goals correctly disallow
quoting as product statistics.

### 2. Health/readiness and recovery surfaces (instrumentation for journey rows)

- `GET /api/v1/health` includes a `storage` object: SQLite/WAL sizes, free
  space, oldest retained event, prune/checkpoint ages, writer backlog;
  `storage.status` becomes `critical` with `maintenanceBlocked: true` below
  256 MiB free. Recovery logging rotates at 4 MiB with three archives.
  [docs/daemon/health.md](https://github.com/OpenCoven/coven/blob/1364cec9/docs/daemon/health.md).
- `coven doctor` gives first-run readiness guidance with no harness on PATH
  ([docs/reference/cli-doctor.md](https://github.com/OpenCoven/coven/blob/1364cec9/docs/reference/cli-doctor.md));
  `coven setup <provider> --verify-only --report-json` emits a **redacted
  certification report carrying only harness, cli_version, platform,
  candidate_commit, duration, exit_class, completed** (v0.4.1 release notes).
- Recovery/operations docs and landed plans: orphan recovery, session handoff
  and cursor recovery, upgrades, diagnostics under
  [docs/daemon/](https://github.com/OpenCoven/coven/tree/1364cec9/docs/daemon);
  plans `2026-08-01-incomplete-work-recovery`, `2026-08-06-session-handoff-cursor`,
  `2026-08-05-output-truncation-markers`, `2026-08-09-pty-sigterm-load-resilience`,
  `2026-08-03-mobile-pairing-retry-recovery`,
  `2026-08-03-universal-runtime-capability-recovery` (all in
  [docs/superpowers/plans/](https://github.com/OpenCoven/coven/tree/1364cec9/docs/superpowers/plans)).

### 3. Release certification and structured-receipt pattern

- [`scripts/certify-release.sh`](https://github.com/OpenCoven/coven/blob/1364cec9/scripts/certify-release.sh)
  (added 2026-08-29, commit [`e0ad4b0`](https://github.com/OpenCoven/coven/commit/e0ad4b0)):
  three-harness release certification packet; runs `coven setup
  <provider> --verify-only --report-json` against real accounts and verifies
  every report certifies the tagged commit. Operator-run local step (needs a
  TTY; costs real provider turns).
- The [v0.4.1 release program plan](https://github.com/OpenCoven/coven/blob/1364cec9/docs/superpowers/plans/2026-08-20-coven-v0.4.1-release-program.md)
  (2026-08-20) specifies a `release-evidence/v0.4.1-certification.json`
  structured receipt bound to a frozen SHA. **`release-evidence/` is not
  committed on `main` at the base SHA** — the receipt exists as a program
  pattern, not a repo artifact.
- Published releases observed via REST (2026-08-30): v0.4.1
  (2026-08-28T15:29:09Z; 4 platform tarballs + `SHA256SUMS`), v0.4.0/v0.3.x
  (2026-08-24), v0.2.5 (2026-08-09). The v0.4.1 release body documents the
  redacted `--report-json` certification contract.

### 4. Packaged-artifact journey evidence

- [`scripts/user-journey-e2e.mjs`](https://github.com/OpenCoven/coven/blob/1364cec9/scripts/user-journey-e2e.mjs)
  (updated 2026-08-28, commit [`8ae39cd`](https://github.com/OpenCoven/coven/commit/8ae39cd)):
  hermetic npm-package journey — help contract, first-run `doctor` guidance,
  fake Codex + engine fixture, daemon lifecycle, a real packaged `coven run`
  turn, sessions/show/events/log inspection, archive/summon/sacrifice, bounded
  `--cwd` rejection, daemon cleanup. Binary **pass/fail** journey coverage —
  it does not yet emit stage-level timing/failure-stage observations.
- [`scripts/release-stress.mjs`](https://github.com/OpenCoven/coven/blob/1364cec9/scripts/release-stress.mjs)
  + [`release-stress.yml`](https://github.com/OpenCoven/coven/blob/1364cec9/.github/workflows/release-stress.yml)
  (added 2026-08-24 — the same day #807 was filed): bounded reliability stress
  workflow, `workflow_dispatch`, OS matrix.

### 5. AgentFS / boundary posture

- `crates/coven-afs` with dedicated CI jobs `afs-mount-linux` / `afs-mount-macos`
  (clippy+tests under the mount feature; a real-mount probe is
  informational-only), plus `scripts/afs-mount-e2e.sh` / `afs-mount-smoke.sh`
  and plan `2026-08-09-afs-macos-consent-confirmation`. The mount backend is
  feature-gated with an informational probe rather than inheriting a generic
  green test count — matching the issue's posture, though no certification
  matrix for credential-observation/case-insensitivity/handle-reuse outcomes
  is published.

### 6. CI routing context

`scripts/classify-ci-changes.py` routes docs-only changes away from the
Rust/Windows/macOS/AFS matrix (relevant to landing the scorecard document
itself); the policy guard (secret scan + privacy guard) runs on PRs.

## What does not exist (grep- and path-verified at `1364cec`, 2026-08-30)

- **No scorecard document anywhere** — case-insensitive grep for `scorecard`
  across docs/, specs/, scripts/, crates/, workflows: 0 hits.
- **No metric-contract records** — no adopted metric carries the issue's
  required fields (definition, numerator/denominator, cohort, window, source,
  privacy treatment, owner-approved target, confidence, breach action).
- **No retained trend/observation history** — benchmark results are per-run
  CI artifacts; nothing on `main` accumulates samples across runs.
- **No usefulness/outcome measurement** — no opt-in beta telemetry or study
  harness exists (consistent with the issue's non-goals).
- **No escaped-defect / discovery-source / rollback tracking**; #805 and
  #779, which would feed the release-quality rows, are both open.

## Verdict against #807 acceptance criteria

| # | Criterion (paraphrased) | Verdict | Basis |
| --- | --- | --- | --- |
| 1 | One current scorecard with definition/source/window/owner/confidence per row | **Not met** | No scorecard artifact exists (grep: 0 hits). |
| 2 | Journey / operation-reliability / recovery / unknown / output-loss / compatibility / release-quality rows have a baseline or are `not yet measured` with owner | **Not met as a decision view** | Instruments exist (§2–§4 above) but no view publishes baselines or an owned not-yet-measured registry. |
| 3 | Benchmark inputs/targets never displayed as achieved product results | **Partial** | Nothing violates it (no scorecard exists); existing convention already enforces the separation (README "trend data", chaos coverage vs. trend separation, 8 s product deadline noted). |
| 4 | Release certification (#779) populates the scorecard from a structured receipt | **Partial** | `certify-release.sh` (2026-08-29) + the redacted `--report-json` contract and the release-program receipt pattern exist; no receipt→scorecard pipeline, and no receipt is committed. |
| 5 | No privacy-sensitive prompts/credentials/content required to compute metrics | **Met for existing instruments** | By construction: disposable homes, fake harness fixtures, env redaction, `pc top --json` without process names, redacted certification reports carrying only the fields listed above. |
| 6 | Thresholds have explicit actions, not decorative dashboards | **Not met** | No thresholds adopted anywhere; README explicitly keeps baselines non-gating until they exist. |

**Overall:** #807 is **not satisfied on `main`** as of `1364cec` (2026-08-30).
The measurement corpus is substantially stronger than the issue's framing
suggests (chaos diagnostics already cover output-loss, cancellation,
backpressure, and crash-recovery determinism), but the decision-grade
scorecard — the actual deliverable — does not exist.

## What remains (critical path, dependency-ordered)

1. **Create the scorecard** at a decided home (e.g. `docs/development/` or
   `docs/reference/`) with the five row labels (**Observed current /
   Historical observation / Target/SLO / Benchmark condition/input / Not yet
   measured**), seeded from the instruments in §1–§4; everything without an
   adopted baseline ships as `not yet measured` with an owner.
2. **Attach the metric contract** to every row (definition, cohort, window,
   source+privacy treatment, confidence, breach action). No target/SLO row may
   be created without an owner-approved decision.
3. **Structured receipts → scorecard**: emit machine-readable receipts from
   `certify-release.sh`, `user-journey-e2e.mjs`, and the benchmark collectors
   (the `release-evidence/` pattern from the v0.4.1 program), so the scorecard
   links raw evidence instead of embedding tables.
4. **Sample counts before statistics**: raise benchmark iteration counts and
   accumulate multi-run chaos samples (currently 3 iterations default; per-run
   artifacts only) before any p95/p99 is quoted as an observed product value.
5. **Release-quality rows** land after #805 (exact-commit governance) and
   #779 (per-platform artifact certification) provide their evidence feeds.
6. **Usefulness rows** stay `not yet measured` until an accountable product
   decision defines the cohort and opt-in mechanism (the issue forbids
   inventing adoption targets).
7. **Regression budgets with explicit breach actions** must precede any
   performance check becoming gating.

## Decision

This record establishes the verified status of #807 for planning. It does not
implement the scorecard and introduces no measurements, targets, or SLOs. The
next dependent step is the implementation PR described in "What remains"
items 1–3; items 4–7 are explicitly blocked on the named decisions/issues, not
on further investigation.
