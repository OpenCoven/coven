---
title: "Eval-loop v2 design and delivery reconciliation"
summary: "Historical design, landed Rust CLI behavior, and deferred integration work for eval-loop v2."
read_when:
  - Reviewing the eval-loop v2 implementation or its remaining integration work
source_adjacent_reason: "Preserves the cross-module design and delivery reconciliation for the Rust eval-loop CLI and daemon."
---

# eval-loop v2: held-out split, noise floor, non-destructive harness-agnostic driver

**Status:** PR 1 delivered in [#1267](https://github.com/OpenCoven/coven/pull/1267) (`24445c96`, closed #1266); later integrations remain deferred. **Original author:** Cody. **Design date:** 2026-10-06; reconciled 2026-10-11.
**Replaces:** an unpublished v1 draft (same day); §0 records what its review found.
**Code:** `crates/coven-cli/src/eval_loop_cli.rs`, `crates/coven-cli/src/eval_loop.rs`, `coven eval-loop` in `crates/coven-cli/src/main.rs`.
**Read §13 first** — it lists where the landed implementation is stricter than the design text below. Where they differ, the code and §13 win.
This preserves the original proposal and records delivery differences in §13. The proposed follow-ups in §10 are not delivery claims.

## 0. What the critical review of v1 found

| # | v1 defect | v2 fix |
|---|---|---|
| 1 | **Self-contradiction.** `cases --split test` was gated on `proposal.json`, but the noise estimate needs test cases *before* any proposal exists. | `noise start` has its own emit path. The gate applies only inside an iteration. |
| 2 | **No baseline.** `metric_before` had no defined origin per iteration. | `evals/<track>/baseline.json`, written by `noise finalize`, refreshed on every ACCEPT. Carries per-case **train** scores so `begin` can surface train failures without re-scoring. |
| 3 | **Silent semantic change** of TSV cols 4–6 (whole set → test split). | Explicit: a row with ≥ 17 columns is v2 and cols 4–6 are test-split; ≤ 11 columns is v1. New header. |
| 4 | **`SKIP` rows in results.tsv** broke iteration numbering and `last_run`. | Refusals go to `eval-loop/skips.jsonl`; DTO exposes `skips[]` + `total_skipped`. `results.tsv` = real iterations only. |
| 5 | **CLI ran `git reset --hard`** — a destructive op inside an autonomous loop. | **The CLI never mutates git.** `decide` records `revert_required` + `base_commit` + `head_commit`; the harness reverts; the next `begin` verifies by ancestry (see #14, §5.7) and refuses `dirty_baseline` otherwise. |
| 6 | **Leak check** was O(n²) and under-specified. | Whitespace-normalised, 60-char windows at stride 20 over strings ≥ 40 chars. Shorter strings are reported as `leak_check: partial`. |
| 7 | `trace_unreviewed` required a human to edit YAML frontmatter. | `coven eval-loop ack` stamps `acknowledged_at` on the last `decision.json`. |
| 8 | `--familiar` resolution depended on `familiars.toml`. | `--workspace <path>` is the primary, harness-agnostic input; `--familiar <id>` is sugar that resolves through `cockpit_sources::familiar_workspace`. |
| 9 | Missing validations. | `scores_incomplete` (every case id must be scored), `score_out_of_range` ([0,1]), `duplicate_case_id`, `cases_invalid` (neither `expected` nor `rubric`). |
| 10 | `grade --exact|--contains` padded PR 1. | Deferred to PR 1b. |
| 11 | Noise staleness was time-only. | Hash + 30 days (refuse) **plus** advisory `noise_aging` after 5 ACCEPTs since measurement (warn, do not refuse). |
| 12 | Minimums were stated without arithmetic. | `test_pct = 30`, `min_test = 10`, `min_train = 20` ⇒ ≥ ~34 cases in practice; `init` prints this. |
| 14 | **Found while implementing:** comparing `HEAD == baseline.commit` breaks the moment the harness commits the loop's own bookkeeping files (noise/baseline/results), which it must. | §5.7 now uses **ancestry**: after a REVERT the reverted `head_commit` must not be an ancestor of HEAD; after an ACCEPT it must be. The dirty-tree check excludes `evals/`, `eval-loop/`, `results.tsv`, `memory/eval-loop-traces/`. `baseline.commit` also pins the implementation for calibration-drift checks (§13); bookkeeping-only commits do not invalidate it. |
| 13 | Information barrier was aspirational. | `context.json` carries train data only; test per-case scores live under `eval-loop/noise/` and `iterations/*/scores/test.jsonl`, never in `baseline.json`. The leak check is the mechanical backstop; the SKILL text is the social one. We do not claim the harness *cannot* read a file. |

## 1. Problem (unchanged)

The loop at `coven@4d59c28c crates/coven-cli/src/eval_loop.rs` records `metric_before → metric_after → ACCEPT|REVERT` on one undivided set with no noise estimate, no headroom check, and **no driver** — the daemon writes `run.json`/`run.lock` and waits for a runtime that does not exist. `nova/results.tsv` is header-only.

## 2. Goals / non-goals

**Goals:** every ACCEPT is justified by a held-out test delta above a measured noise floor · the proposer sees train data only, with a leak check · one iteration is runnable from any harness with only shell access to `coven`, file I/O, and the harness's own model · additive compatibility with v1 rows and the three daemon routes · **zero destructive operations in the CLI**.

**Non-goals:** Cave Evals UI (retired; `evals-removal.test.ts` guards it) · LLM calls from the daemon or CLI · auto-promotion to `main`.

## 3. Division of labour

| Step | Who | Model? |
|---|---|---|
| Validate set, split, headroom, noise freshness, baseline/HEAD check, lock | `coven eval-loop begin` | no |
| Propose ONE change, apply, commit on loop branch | harness | yes |
| Emit cases per split | `coven eval-loop cases` | no |
| Run + judge each case → scores | harness | yes |
| Metrics, accept rule, leak check, TSV row, trace, unlock | `coven eval-loop decide` | no |
| K repeated baseline scorings | `coven eval-loop noise start/finalize` + harness | scoring only |
| Human read receipt | `coven eval-loop ack` | no |
| History | `coven eval-loop status` / daemon GET | no |

Rule: deterministic work in the Rust CLI; model work in the harness; files are the only interface.

## 4. Layout

```
<workspace>/
  results.tsv
  evals/<track>/
    cases.jsonl        # {id, source, input, expected?, rubric?, tags?}
    judge.md           # rubric template for LLM-judge cases
    config.json        # {salt, test_pct, min_test, min_train, saturation, noise_max_floor, noise_max_age_days}
    noise.json         # {eval_set_hash, measurement_hash, runs, mean, stddev, floor, measured_at, accepts_since}
    baseline.json      # {eval_set_hash, measurement_hash, commit?, test_mean, train_mean, train_cases:{id:score}, measured_at, source}
  eval-loop/
    run.json  run.lock            # daemon contract, unchanged
    mutation.lock                 # exclusive fs2 lock shared by the CLI and daemon routes (§13)
    skips.jsonl                   # {timestamp, track, reason, detail, run_id?}
    noise/<track>/plan.json       # carries generation, result, completed (§13)
    noise/<track>/<generation>/pass-<k>.jsonl, train.jsonl
    iterations/<track>/<n>/context.json | proposal.json | scoring.json | scores/{train,test}.jsonl | finalization.json | decision.json
  memory/eval-loop-traces/YYYY-MM-DD-<track>-<n>.md
```

## 5. Guardrails

**5.1 Split.** `test iff blake3(salt ‖ id)[0..8] as u64 % 100 < test_pct`. Salt is created once by `init`; changing it invalidates `noise.json` and `baseline.json`. Refuse `insufficient_cases` below `min_test`/`min_train`.

**5.2 Noise floor.** `noise start --runs K` (default 3) writes `plan.json` and prints the test and train case lists. Harness writes `pass-1..K.jsonl` (test) and `train.jsonl` with no change applied. `noise finalize` (pass files live under a per-calibration `<generation>` directory): per-pass mean; `stddev` = sample stddev of pass means (K−1); `floor = max(2·stddev, 0.01)`; writes `noise.json` and `baseline.json` (`source: "noise"`). Refusals at `begin`: `noise_missing`, `noise_stale` (hash mismatch or > `noise_max_age_days`), `noise_too_high` (floor > `noise_max_floor`, default 0.15).

**5.3 Headroom.** Refuse `saturated` when `baseline.test_mean ≥ saturation` (default 0.95).

**5.4 Information barrier.** `context.json` contains: run/iteration ids, baseline means, floor, `train_failures[]` (id, input, expected?, rubric?, score — up to 20, ascending score), `request_buckets`, paths. Never a test row. `cases --split test` refuses `proposal_missing` until `proposal.json` exists. `decide` leak-checks the loop diff (`git diff <base_commit> HEAD`, read-only; or the contents of `proposal.files[]` when the workspace is not a git repo) against every test case's `input`/`expected` using §0-6 windows → `REVERT / leak_detected`.

**5.5 Accept rule.**
```
Δtest = test_after − baseline.test_mean ; Δtrain = train_after − baseline.train_mean
REVERT  leak_detected
REVERT  too_many_files (files > 3 unless allow_multi_file)
REVERT  buckets_missing (when requested)
ACCEPT  Δtest > floor and Δtrain ≥ 0                 → reason accepted
REVERT  Δtrain > floor and Δtest ≤ floor             → reason overfit
REVERT  Δtest < −floor                              → reason regression
REVERT  otherwise                                   → reason noise
```
Order of evaluation is as listed; the first matching reason wins.

**5.6 Stall.** After 3 consecutive REVERTs on a track `begin` sets `request_buckets: true`; `decide` requires non-empty `proposal.failure_buckets[]`.

**5.7 Git safety.** In a Git workspace, `begin` requires: (a) a clean tree outside the loop's bookkeeping paths — `git status --porcelain --untracked-files=all -- . ':(exclude)evals' ':(exclude)eval-loop' ':(exclude)results.tsv' ':(exclude)memory/eval-loop-traces'` is empty, else `dirty_baseline`; (b) **ancestry consistency with the last decision**: if it was a REVERT whose `head_commit ≠ base_commit`, `git merge-base --is-ancestor <head_commit> HEAD` must be *false* (the harness reverted), else `dirty_baseline {reverted_commit}`; if it was an ACCEPT, it must be *true*, else `dirty_baseline {accepted_commit}`. SHA equality with `baseline.commit` is **not** required, but implementation changes since that commit cause `dirty_baseline`; committing bookkeeping between iterations is expected. `decide` records `base_commit` (HEAD at `begin`) and `head_commit` (HEAD at `decide`); on REVERT it sets `revert_required: true` and the harness resets to `base_commit`. The CLI runs only `git rev-parse`, `git status --porcelain`, `git branch --show-current`, `git diff`, `git show`, `git merge-base --is-ancestor`. Non-git workspaces skip (a)/(b) and record `git: false`.

**5.8 Human gate.** `begin` refuses `trace_unreviewed` when the previous `decision.json` for the track lacks `acknowledged_at`, unless `--ack-unreviewed`. `ack` stamps it.

## 6. `results.tsv` v2

Cols 0–10 as v1. v2 appends:

| idx | column | notes |
|---|---|---|
| 11 | `metric_train_before` | |
| 12 | `metric_train_after` | |
| 13 | `noise_floor` | |
| 14 | `eval_set_hash` | blake3 hex of cases.jsonl bytes |
| 15 | `decision_reason` | §5.5 reasons |
| 16 | `run_id` | daemon `run.json.runId` when adopted, else CLI uuid |

Cols 4–6 are **test-split** in v2 rows. Fields are sanitised (`\t`, `\n`, `\r` → space). Header is written when the file is created; the parser ignores it. Outcome stays `ACCEPT|REVERT`.

## 7. Daemon DTO (`eval_loop.rs`, additive)

`LoopIterationDto` += `metric_train_before`, `metric_train_after`, `noise_floor`, `eval_set_hash`, `decision_reason`, `run_id` (all `Option`, skipped when `None`). `EvalLoopStateDto` += `skips: Vec<SkipDto>`, `total_skipped`, `noise: Vec<NoiseDto>` (one per track with a `noise.json`). `get_eval_loop_state` returns `Some` when `skips.jsonl` contains a valid refusal record even without `results.tsv`. Existing daemon routes remain available; run enqueue/clear operations now share the mutation lock with the CLI (§13).

## 8. CLI

Common: `--workspace <path>` **or** `--familiar <id>`; `--track synthesis|prompt|memory`; JSON on stdout. Exit `0` ok · `3` refused (`{"refused":"<reason>","detail":…}`) · `1` error.

```
coven eval-loop init     --track T [--test-pct 30]
coven eval-loop noise    start    --track T [--runs 3]
coven eval-loop noise    finalize --track T
coven eval-loop begin    --track T [--from-run-json] [--ack-unreviewed]
coven eval-loop cases    --track T --split train|test
coven eval-loop decide   --track T
coven eval-loop ack      --track T
coven eval-loop status   --track T [--last N]
```

Every mutating subcommand, and the daemon's `POST run` / `DELETE run-lock`, first takes the exclusive `eval-loop/mutation.lock`. Run lock: `begin` creates `run.json`+`run.lock` when absent (so Cave's GET shows `running`); with `--from-run-json` it adopts the daemon's `runId`/`track`. `decide` removes both. An existing lock from another run → refuse `run_in_progress`.

## 9. Proposed harness protocol

The following is protocol pseudocode: supply `--workspace` (or `--familiar`) and `--track` to each command, and write proposal/scores to the paths returned by the CLI.

```
coven eval-loop begin --track prompt                  # exit 3 → stop and inspect the returned refusal
cat eval-loop/iterations/prompt/<n>/context.json      # train failures, baseline, floor, paths
# propose ONE change, edit, git commit on eval-loop/<track>/<run8>
write proposal.json {summary, reasoning, files[], failure_buckets?[], allow_multi_file?}
coven eval-loop cases --split train → run+judge each → scores/train.jsonl   {id, score, notes?}
coven eval-loop cases --split test  → run+judge each → scores/test.jsonl
coven eval-loop decide                                 # ACCEPT keeps; REVERT → the harness resets to `base_commit` (the CLI never does)
coven eval-loop ack                                    # after a human reads the trace
```

The CLI exposes a shell/file protocol and makes no model or network calls. Coven's supported harness set remains Codex, Claude Code, and GitHub Copilot CLI. This design does not certify end-to-end use by those harnesses or implement adapters for additional harnesses, Cave scheduling, or Nova. A daemon run can be adopted through `begin --from-run-json` only when the run id and track match.

## 10. PR plan

- **PR 1 — landed as #1267:** §6 schema + §7 DTO + §8 CLI + unit tests, plus the §13 hardening. `docs/reference/api.md` carries the DTO note.
- **PR 1b:** `grade --exact|--contains`.
- **PR 2 (familiar skill, outside this repo):** rewrite the `eval-loop` SKILL.md to the §9 protocol, including the §13 commit-before-scoring step. `init` already writes a `judge.md` template.
- **First target:** `prompt` track on the Cave thread self-report reflection prompt; ~30 archived threads hand-labelled.

## 11. Original acceptance criteria (PR 1)

1. v1 (11-col) and v2 (17-col) rows parse together; new fields `None` for v1.
2. Split is deterministic for `(salt,id)`; a salt, config, or `judge.md` change ⇒ `begin` refuses `noise_stale` (via `measurement_hash`, §13).
3. `begin` refusals each produce a `skips.jsonl` line and exit 3: `insufficient_cases`, `noise_missing`, `noise_stale`, `noise_too_high`, `saturated`, `trace_unreviewed`, `iteration_open`, `run_in_progress`, `dirty_baseline` (git fixture: un-reverted REVERT commit; uncommitted non-bookkeeping edit; bookkeeping-only changes pass).
4. `noise finalize` with passes [0.70,0.72,0.74] → stddev 0.02, floor 0.04; `baseline.json` written.
5. `decide`: train +0.20 / test 0.00 ⇒ `REVERT overfit`; test +0.10 vs floor 0.04 ⇒ `ACCEPT`, baseline refreshed; test −0.10 ⇒ `regression`.
6. Leak: a proposal file containing a test `expected` (≥ 40 chars) ⇒ `REVERT leak_detected`.
7. `cases --split test` before `proposal.json` ⇒ exit 3 `proposal_missing`.
8. Missing one score id ⇒ error `scores_incomplete`; score 1.2 ⇒ `score_out_of_range`.
9. `get_eval_loop_state` exposes `skips`, `total_skipped`, `noise` and still returns `None` for an empty workspace.
10. Focused eval-loop tests and existing compatibility coverage pass. #1267 records the actual delivery verification; the original test-count estimate is not a current invariant.

## 12. Open questions (defaults chosen)

Judge ≠ model under test — harness choice, recorded in `proposal.json.judge`. Per-track `test_pct` — `config.json`. SKIPs vs 10-iteration cap — SKIPs don't count.

## 13. As landed (#1267) — differences from the text above

#1267 delivered the Rust CLI/DTO work in PR 1, with the differences below. It did not deliver PR 1b, the external familiar skill, or the proposed first-target evaluation. Each item is in `crates/coven-cli/src/eval_loop_cli.rs` unless noted.

| Area | Design text | Landed behaviour |
|---|---|---|
| Invalidation | `noise.json`/`baseline.json` keyed on `eval_set_hash` (cases bytes) | Also keyed on `measurement_hash`, blake3 over length-prefixed `cases.jsonl` + `config.json` + `judge.md`. A split or grader change makes calibration stale. `eval_set_hash` keeps its cases-only meaning in `results.tsv`. |
| Calibration recovery | `noise finalize` writes `noise.json` + `baseline.json` | The result is recorded in `plan.json` first, then published with `completed: true`. A crash in between leaves `calibration_pending`; `noise start` and `begin` refuse until `noise finalize` is retried. Each `noise start` creates a fresh `<generation>` directory, so stale pass files cannot be reused. |
| Calibration implementation | Original draft treated `baseline.commit` as informational | Calibration pins the Git implementation. `noise finalize` refuses implementation drift since `noise start`; `begin` requires the calibrated implementation outside bookkeeping paths. |
| Refusal recording | Every refusal assumed to append `skips.jsonl` | `begin` records normal admission refusals through its skip helper. Early `calibration_pending` and adopted-run ownership refusals return directly; always inspect command JSON/exit status. |
| Scoring binding | `cases --split test` gated only on `proposal.json` | In git workspaces the first `cases --split test` writes `scoring.json` = {HEAD, blake3(proposal.json), measurement_hash}. Later `cases --split test` and `decide` validate that binding (`proposal_changed`) and require a clean tree outside bookkeeping (`dirty_proposal`). `cases --split train` does not perform scoring-binding checks. Evaluation-input drift can return `noise_stale` before binding validation; `decide` refuses `scoring_not_started` if no binding exists. |
| Decision recovery | `decide` appends TSV, refreshes baseline, writes trace, unlocks | `decide` first writes `finalization.json` (decision, TSV row, baseline/noise updates, trace). Publication is idempotent (the row is skipped if a v2 row with the same track + `run_id` exists) and `decision.json` is written last, so completion is visible only when every artifact exists. A retried `decide` resumes; `cases` refuses `finalization_pending` meanwhile. |
| Run ownership | `run_in_progress` for a foreign lock | `run_ownership_mismatch` when `run.lock` and `run.json` no longer both name this iteration's `run_id` and track (cleared or re-enqueued run). Checked by `begin --from-run-json`, `cases`, `decide`, and finalization; a replacement run is never released. |
| Concurrency | not specified | Exclusive `eval-loop/mutation.lock` (fs2) shared by every mutating CLI subcommand and the daemon routes (`crates/coven-cli/src/eval_loop.rs` `mutation_lock`). |
| Other refusals | — | `not_initialized` (no `config.json`), `no_active_iteration` (`cases`/`decide` before `begin`), `nothing_to_ack`. |

Unchanged from the design: split rule, floor `max(2·stddev, 0.01)`, defaults (`test_pct` 30, `min_test` 10, `min_train` 20, `saturation` 0.95, `noise_max_floor` 0.15, `noise_max_age_days` 30), `noise_aging` after 5 ACCEPTs, accept-rule order, leak windows (40/60/20), stall after 3 REVERTs, ancestry-based git check with bookkeeping excludes, the 17-column TSV header, and exit code 3 for refusals.
