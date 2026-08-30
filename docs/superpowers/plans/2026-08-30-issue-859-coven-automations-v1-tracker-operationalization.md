# Issue #859 Status/Decision Record — Operationalize Coven Automations v1 in Beads and GitHub roadmap mirrors

**Date:** 2026-08-30
**Author:** Timothy Wayne Gregg
**Scope:** Investigation and reviewed tracker-setup deliverables for
[OpenCoven/coven#859](https://github.com/OpenCoven/coven/issues/859) ("P0 control:
Operationalize Coven Automations v1 through Cave's canonical Beads graph and GitHub
mirrors"). Facts only; each claim carries an evidence link or exact command.

---

## 1. What was inspected

- Upstream `OpenCoven/coven` `main` at `1364cec` (`1364cec9dbaf1e2aca2e4544dec0e1ce807d859c`), cloned 2026-08-30.
- GitHub issues [OpenCoven/coven#854](https://github.com/OpenCoven/coven/issues/854) (opened 2026-08-30T13:36:04Z), [#816](https://github.com/OpenCoven/coven/issues/816) (2026-08-24T15:32:52Z), [#855](https://github.com/OpenCoven/coven/issues/855), [#856](https://github.com/OpenCoven/coven/issues/856), [#857](https://github.com/OpenCoven/coven/issues/857), [#858](https://github.com/OpenCoven/coven/issues/858) (all opened 2026-08-30T13:37–13:41Z), and [#859](https://github.com/OpenCoven/coven/issues/859) (2026-08-30T13:41:50Z); all open, all assigned to `BunsDev`, no labels, no milestones.
- The single comment on #859 (BunsDev, 2026-08-30T14:05:44Z,
  [comment 5469149789](https://github.com/OpenCoven/coven/issues/859#issuecomment-5469149789))
  — the operational correction redirecting the canonical Beads store to Cave.
- `OpenCoven/coven-cave` issues #5219 (roadmap/operating contract, opened 2026-08-30T14:03:02Z) and #5220 (Beads/Dolt seeding, opened 2026-08-30T14:04:34Z); both open.
- `OpenCoven/coven-cave` local checkout `.beads/` directory: `issues.jsonl` (public-scrubbed export), `config.yaml` (sync remote `git+https://github.com/OpenCoven/coven-cave.git`), README, hooks.
- Upstream CI state on the base SHA (see §4).
- Search for existing work on #859: no open PRs reference 859 (`search/issues` and `repos/OpenCoven/coven/pulls?state=open`, 2026-08-30); no `859` branch on `CompleteDotTech/coven`.

## 2. What exists on `main` today

**The native automations foundation is implemented on `main`; the tracker graph is not.**

- `crates/coven-cli/src/automations/` exists on main: 11 Rust files, 2,719 lines total
  (`definition.rs`, `store.rs`, `occurrences.rs`, `rrule.rs`, `schedule.rs`, `runner.rs`,
  `runs.rs`, `health.rs`, `import_legacy.rs`, `daemon_tick.rs`, `mod.rs`), plus daemon
  integration in `crates/coven-cli/src/daemon.rs`.
- The landed series is dated 2026-08-28 and is enumerated by
  [#816's program-status section](https://github.com/OpenCoven/coven/issues/816)
  (in the issue body, updated 2026-08-30): PR [#846](https://github.com/OpenCoven/coven/pull/846) (routine
  definitions and control actions, part 1 — commit `882fc83`) and PR
  [#847](https://github.com/OpenCoven/coven/pull/847) (legacy import — merge commit
  `58bc547`), with parts 5–8 as commits `39b8618`, `bd3b47d`, `a4a71af`, `1de50a8`.
  #816's comment enumerates what landed: versioned routine definitions; SQLite
  definition/occurrence/lease/run storage; RRULE planning; unique occurrence fencing and
  bounded claim leases; expired-lease recovery, latest-only misfire, overlap refusal;
  daemon recurring tick and scheduled dispatch; shared manual/scheduled launch path;
  familiar ID propagation; bounded logs and atomic output delivery; health and run-history
  projections; source-preserving paused legacy import; `coven.automations.*` control
  actions; Cave's migration away from direct Codex ownership
  ([OpenCoven/coven-cave#4990](https://github.com/OpenCoven/coven-cave/issues/4990)).
- #816 itself states it "remains open for **foundation reconciliation and exact
  evidence**, not because the original architecture is still absent", and lists the
  evidence required before closing (linked commit/PR series with exact final revision,
  clean-clone test verification, migration/rollback proof, daemon wiring proof on
  supported platforms, unified run-path proof, stale-lease proof, delivery-failure
  non-success proof, criterion-by-criterion reconciliation).
- **No Beads store exists in `OpenCoven/coven`** — no `.beads/` directory on `main`.
  Per the #859 operational correction this is correct: the canonical Beads graph is Cave's
  embedded-Dolt database `cave` in `OpenCoven/coven-cave`; a competing Beads database must
  not be initialized in `OpenCoven/coven`.
- The Automations v1 delivery epic and its `surface:shared` beads **do not exist yet**:
  the `coven-cave` public-scrubbed export (`.beads/issues.jsonl`, 4 entries checked
  2026-08-30) contains only the `cave-hlv` Beads-operating epic (`cave-hlv`,
  `cave-hlv.1` in_progress, `cave-hlv.2`/`.3` deferred); no bead references
  `OpenCoven/coven` issues #854/#816/#855–#858. Provisioning is owned by
  [OpenCoven/coven-cave#5220](https://github.com/OpenCoven/coven-cave/issues/5220).
  The live Dolt database could not be read from this environment (no `bd`/`dolt` binary
  available); the export is the review-visible state.
- `docs/roadmaps/` did not exist on `main`; the repo's record location is
  `docs/superpowers/plans/<date>-<slug>.md` (47 existing records, 2026-05-04 → 2026-08-23).
- No SDK, Cave, Psyche, docs, organization-canary, Familiar Contract, or Threads child issues
  existed under #854 at investigation time (issue-body search returned only #859 and the
  P0 graph).
- `docs/ROADMAP.md` is the public product roadmap (last updated 2026-05-26) and does not
  cover the Automations v1 delivery graph.

## 3. Issue state and the operational correction

[#859](https://github.com/OpenCoven/coven/issues/859) was opened 2026-08-30T13:41:50Z.
Its Phase 1 says to "inspect the current Coven Beads schema/version"; the
[operational correction](https://github.com/OpenCoven/coven/issues/859#issuecomment-5469149789)
(2026-08-30T14:05:44Z) resolves the ambiguity:

- the canonical familiar execution queue is **Cave's embedded-Dolt Beads graph**;
  references to inspecting "the current Coven Beads schema" mean inspecting Cave's
  canonical Beads/Dolt schema and workflow through #5220;
- roadmap and reviewed operating contract: OpenCoven/coven-cave issue #5219;
- seeding, dependency verification (`bd dep` help, `bd dep list`, `bd ready --json`),
  bounded `pnpm beads:sync`, and before/after `refs/dolt/data` OIDs: #5220;
- `.beads/issues.jsonl` is a public-scrubbed review export, never canonical state;
- do **not** initialize a competing Beads database in `OpenCoven/coven`; the original
  tracker roles and acceptance gates remain valid.

Consequently #859's GitHub-side deliverables (roadmap artifact, machine-readable mapping,
drift detection, and this record) land in this repository through review, while bead
creation stays with #5220 in `OpenCoven/coven-cave`.

## 4. Pre-change integrity/status report (2026-08-30)

| Check | Result |
| --- | --- |
| Working tree | clean before this change (only `docs/roadmaps/` additions by this branch) |
| Base SHA | `1364cec9dbaf1e2aca2e4544dec0e1ce807d859c` (even with upstream `main` and fork `main`) |
| `docs/superpowers/plans/` | 47 records present, latest `2026-08-23-maintenance-participant.md` |
| `docs/roadmaps/` | absent on base (created by this branch per #859's suggested path) |
| Upstream CI on base SHA | `CI` run 33309176793 (2026-08-30T11:32:21Z) **failed** at "Classify changes" — `scripts/classify-ci-changes.py` raised `ValueError: no paths provided` on the empty-diff `push` to `main` for commit `1364cec` ("chore: preserve consolidated branch ancestry"). This is a push-event classification edge case, not a PR-path failure: PR classification uses `PR_BASE_SHA...PR_HEAD_SHA`, which always contains paths. No other CI workflow run failed on the base SHA; the `Engine bump` workflow succeeded on the same SHA at 2026-08-30T13:48:51Z. |
| Upstream open PRs touching this area | none found for issue #859 |
| Fork (`CompleteDotTech/coven`) branch state | `main` mirrors upstream `main` at the same SHA; no `859` branch existed before this work |
| Beads export cross-check | `node docs/roadmaps/drift-check.mjs --beads-export .beads/issues.jsonl` (run against the `coven-cave` checkout export) → no errors; confirms zero Automations v1 beads and no sensitive payloads in the export |

## 5. Decisions

- **D1 — one canonical writer.** Exactly one checkout/process is designated schema
  migrator and canonical writer for this setup; persisted tracker changes land only
  through reviewed PRs (this branch/PR for GitHub-side artifacts; #5220's checkout for
  Bead-side provisioning). Concurrent independent migrations and direct writes from
  unrelated worktrees are refused. The `coven claim` registry could not be used here
  (no Rust toolchain in this environment to build the CLI); REST deconfliction (§1) plus
  a dedicated clone and branch satisfy the anti-duplication intent.
- **D2 — reuse, don't duplicate.** No bead mapping #816 was found in the review-visible
  export, so nothing is duplicated: #816's mapping entry is declared once in the mapping
  file with `provisioning` pointing at #5220, which owns the reuse-check against the live
  Dolt store before creating anything.
- **D3 — no competing Beads store.** No `.beads/` is initialized in `OpenCoven/coven`;
  the operational correction is honored verbatim.
- **D4 — mapping lives in both worlds correctly.** GitHub owns the public roadmap
  artifact and the machine-readable mapping contract (this PR); Beads owns the
  implementation dependency graph and execution state once provisioned. The mapping file
  is the reconciliation contract; Bead IDs stay `null` (warn-level `W010`) until #5220
  declares them, after which a one-line reviewed change flips provisioning to `done` and
  missing-mapping drift escalates to `error`.
- **D5 — drift detection without credentials.** `docs/roadmaps/drift-check.mjs` is
  dependency-free, offline, and CI-safe; it verifies mapping-internal invariants, the
  generated roadmap block (generator-contract enforcement), and an optional local Beads
  export, and scans tracker output for sensitive payloads. `--selftest` proves every
  detection class with fixtures (all 11 fixtures and 6 sensitive-payload rules pass).
- **D6 — severity policy.** `error` findings fail CI; `warning` findings (today: pending
  provisioning) report without failing. This keeps the check honest (it reports the real
  gap) without keeping CI red on work owned by another repository.
- **D7 — no premature closure.** This PR references #859 without `Closes`; the issue's
  own completion semantics (#816 stays open until its evidence checklist is done; #859
  spans provisioning in `coven-cave`) mean nothing is closed by tracker work alone.

## 6. Verdict against #859's acceptance criteria

| # | Criterion | Verdict |
| --- | --- | --- |
| 1 | #854, #816, #855–#858 each map to exactly one Bead | **PARTIAL** — the one-to-one contract is committed (mapping file: 6 outcomes, unique slugs/labels/refs, enforced by `E001`/`E002`), but bead IDs are `null` until #5220 provisions them; drift check reports `W010` per outcome. |
| 2 | Cross-repository child outcomes map one-to-one as created | **SATISFIED (vacuously today)** — no child outcomes existed at sync time; `cross_repository_children` is empty and the policy + `E101` enforcement are in place. |
| 3 | Dependencies and P0/P1/P2 priorities match the canonical roadmap | **SATISFIED** — mapping `depends_on` mirrors #859's minimum graph; the roadmap table is generated from the same file, and any hand edit is flagged `E008`. |
| 4 | One writer/schema owner and reviewed change path documented | **SATISFIED** — D1 above; also recorded in the roadmap sync metadata. |
| 5 | Roadmap artifact and machine-readable mapping committed through review | **SATISFIED BY THIS PR** — `docs/roadmaps/coven-automations-v1.md` + `coven-automations-v1.mapping.json` + `drift-check.mjs`. |
| 6 | Drift detection catches state, priority, parent, evidence, generated-mirror disagreement | **SATISFIED** — classes `E001`–`E009` (plus `E100`–`E104` in export mode) cover state, priority, parent/dependency, evidence, generated-mirror, and sensitive payloads; proven by `--selftest`. |
| 7 | No tracker data treated as automation runtime truth | **SATISFIED (documented)** — canonical tracker roles in the roadmap and mapping invariants state it; the Coven runtime owns occurrences/runs/leases/approvals/receipts. |
| 8 | Final #854 release rollup generable from reconciled state and exact evidence | **PENDING** — the mapping carries evidence links and gates so the rollup is generable in principle, but certification evidence does not exist yet (no #855–#858 outcomes started; #816's evidence checklist open). |

## 7. What remains

1. Provision the Automations v1 delivery epic and six `surface:shared` beads in Cave's
   canonical Beads/Dolt graph with dependency verification and bounded
   `pnpm beads:sync`, recording before/after `refs/dolt/data` OIDs —
   OpenCoven/coven-cave issue #5220
   (P0, on the critical path for criterion 1).
2. Land Cave's reviewed operating contract — OpenCoven/coven-cave issue #5219.
3. Fill #816's evidence checklist (criterion 1's "close only after" condition and the
   foundation release gate).
4. After provisioning: set the real bead IDs in
   `docs/roadmaps/coven-automations-v1.mapping.json` (one reviewed change; `W010` clears;
   `E101` escalation arms).
5. Wire `node docs/roadmaps/drift-check.mjs` into relevant PR CI and the weekly program
   rollup cadence once provisioning exists (the check itself needs no credentials).
6. Create the P1 cross-repository outcomes (SDK, Cave, Psyche, docs,
   organization-canary, Familiar Contract, Threads) under #854 and map them one-to-one as
   they appear, with explicit `depends_on`/`depends_on_external` edges.
7. #855–#858 implementation work per the graph below.

## 8. Critical path (before/after P0 dependency graph)

**Before this PR:** no recorded graph in this repository — the P0 ordering existed only
in #859's prose; the bead side did not exist at all.

**After this PR (GitHub side recorded; bead provisioning pending → #5220):**

```text
#854 program (P0 control — release gates, rollup)
  └─ gate 1: #816 foundation (P0 — landed 2026-08-28, evidence reconciliation open)
       ├─ #855 protocol (P0, depends: foundation)
       │    ├─ #856 scheduler (P0, depends: foundation, protocol)
       │    └─ #857 authority (P0, depends: foundation, protocol; + upstream Familiar/Threads profiles when created)
       └─ #858 certification (P0, depends: protocol, scheduler, authority)
            └─ v1 release gate (owned by #854)
```

## 9. Initial evidence packet

- Pre-change tracker report: §4 above plus the `drift-check` export cross-check (no
  Automations v1 beads; export contains only the `cave-hlv` operating epic).
- Created/reused bead IDs: none created by this repository (forbidden by the operational
  correction); provisioning delegated to OpenCoven/coven-cave#5220 (D2/D3).
- Beads version/schema: Beads 1.2.2, schema v53, as recorded in
  [2026-08-20-coven-v0.4.1-release-program.md](./2026-08-20-coven-v0.4.1-release-program.md);
  live verification owned by #5220.
- Reviewed PR: this branch —
  `agent/issue-859-p0-control-operationalize-coven-automations-v1` based on `1364cec`.
- Drift report: `node docs/roadmaps/drift-check.mjs` → 0 errors, 6 × `W010` (pending
  provisioning); `--selftest` → pass.
- Final mapping: `docs/roadmaps/coven-automations-v1.mapping.json` (schema version 1).
- Before/after P0 dependency graph: §8.
