# Coven CLI Performance Baselines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a fixture-backed, machine-readable baseline runner for the real Coven CLI and daemon without making unstable timing a required CI gate.

**Architecture:** A Node 24 script owns temporary homes, process timing, daemon lifecycle, fixture API calls, and JSON report generation. An ignored Rust test emits deterministic chat scheduling metrics from the existing event-loop cadence; it does not change normal interactive behavior.

**Tech Stack:** Node 24 ESM, `node:test`, Node `http` Unix-socket requests, existing Rust smoke fixtures, `cargo test`.

---

## File structure

- Create: `scripts/benchmark-cli.mjs` — CLI runner, temporary fixture owner, report serializer.
- Create: `scripts/benchmark-cli.test.mjs` — unit tests for argument parsing, percentile calculation, report redaction, and fake-binary timing.
- Modify: `crates/coven-cli/src/tui/chat/events.rs` — ignored test-only scheduling metric and focused cadence tests.
- Modify: `README.md` — contributor-facing invocation and non-gating policy.
- Modify: `.github/workflows/ci.yml` — optional artifact-only benchmark job after the runner is deterministic on Linux.

### Task 1: Specify and test the Node runner's pure report helpers

**Files:**
- Create: `scripts/benchmark-cli.mjs`
- Create: `scripts/benchmark-cli.test.mjs`

- [ ] **Step 1: Write failing tests for options and summary statistics**

```js
import assert from 'node:assert/strict';
import test from 'node:test';
import { parseOptions, summarizeSamples } from './benchmark-cli.mjs';

test('summarizeSamples reports deterministic median and nearest-rank p95', () => {
  assert.deepEqual(summarizeSamples([9, 1, 5, 3, 7]), {
    minMs: 1,
    medianMs: 5,
    p95Ms: 9,
    maxMs: 9
  });
});

test('parseOptions rejects an absent binary and non-positive iteration count', () => {
  assert.throws(() => parseOptions(['--iterations=0']), /--binary is required/);
  assert.throws(() => parseOptions(['--binary=/tmp/coven', '--iterations=0']), /positive integer/);
});
```

- [ ] **Step 2: Run the focused test and verify failure**

Run: `node --test scripts/benchmark-cli.test.mjs`

Expected: FAIL because `benchmark-cli.mjs` does not export the requested helpers.

- [ ] **Step 3: Implement pure, redacting helpers**

```js
export function summarizeSamples(samples) {
  const sorted = [...samples].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return {
    minMs: sorted[0],
    medianMs: sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2,
    p95Ms: sorted[Math.ceil(sorted.length * 0.95) - 1],
    maxMs: sorted.at(-1)
  };
}
```

Implement `parseOptions` with `--binary`, `--iterations`, `--output`, and
`--session-counts`, rejecting unknown flags and non-positive values. Implement
`reportEnvironment` without serializing `HOME`, `COVEN_HOME`, or temporary paths.

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `node --test scripts/benchmark-cli.test.mjs`

Expected: PASS.

### Task 2: Add isolated process timing and report writing

**Files:**
- Modify: `scripts/benchmark-cli.mjs`
- Modify: `scripts/benchmark-cli.test.mjs`

- [ ] **Step 1: Write a failing fake-binary test**

```js
test('runScenario preserves only timing and exit metadata', async () => {
  const report = await runScenario({ command: fakeBinary, args: ['--help'], iterations: 2 });
  assert.equal(report.samplesMs.length, 2);
  assert.deepEqual(Object.keys(report).sort(), ['exitCodes', 'samplesMs', 'summary']);
});
```

- [ ] **Step 2: Run the focused test and verify failure**

Run: `node --test scripts/benchmark-cli.test.mjs`

Expected: FAIL because `runScenario` is absent.

- [ ] **Step 3: Implement process timing and atomic report output**

Use `spawnSync` with explicit `cwd`, a temporary `COVEN_HOME`, a controlled
`HOME`/`USERPROFILE`, and a command timeout. Measure with `process.hrtime.bigint()`;
record milliseconds rounded to three decimal places. Write JSON through a sibling
temporary file followed by rename. Fail if a scenario exits outside its declared
allowed exit codes.

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `node --test scripts/benchmark-cli.test.mjs`

Expected: PASS.

### Task 3: Add daemon fixture population through the public socket API

**Files:**
- Modify: `scripts/benchmark-cli.mjs`
- Modify: `scripts/benchmark-cli.test.mjs`

- [ ] **Step 1: Write a failing request-shape test**

```js
test('external session fixture request uses the versioned sessions endpoint', () => {
  assert.deepEqual(externalSessionRequest('s-1').path, '/api/v1/sessions/external');
  assert.equal(externalSessionRequest('s-1').method, 'POST');
});
```

- [ ] **Step 2: Run the focused test and verify failure**

Run: `node --test scripts/benchmark-cli.test.mjs`

Expected: FAIL because the request factory is absent.

- [ ] **Step 3: Implement lifecycle and fixtures**

Start `coven daemon start`, wait for health through its local socket, and stop it
in `finally`. Implement HTTP-over-Unix-socket requests with Node `http.request`.
Register deterministic external sessions through `/api/v1/sessions/external` for
the 100, 1,000, and 10,000 session-list fixtures. For event-tail fixtures,
launch a controlled live `codex` shim through `POST /sessions`, seed safe input
events through `POST /sessions/:id/input`, advance to the final bounded page via
the documented events cursor, and measure that read. Kill the live fixture before
stopping its daemon. Do not write SQLite files directly and do not add a
fixture-only daemon endpoint.

- [ ] **Step 4: Run the runner against a prebuilt binary**

Run: `cargo build -p coven-cli --locked && node scripts/benchmark-cli.mjs --binary target/debug/coven --iterations 3 --output /tmp/coven-perf.json`

Expected: exit 0 and a JSON report with command, daemon, list, event-tail, and launch scenarios.

### Task 4: Add deterministic chat scheduling metrics without changing the event loop

**Files:**
- Modify: `crates/coven-cli/src/tui/chat/events.rs`

- [ ] **Step 1: Write a failing ignored metric test**

```rust
#[test]
#[ignore]
fn benchmark_schedule_metrics_emit_json() {
    println!("COVEN_BENCHMARK_TUI={}", serde_json::json!({
        "idle": { "draws": 100, "polls": 100, "durationMs": 10_000 },
        "streaming": { "draws": 100, "polls": 100, "durationMs": 10_000 }
    }));
}
```

- [ ] **Step 2: Run the focused Rust test and verify failure**

Run: `cargo test -p coven-cli tui::chat::events::tests::benchmark_schedule_metrics_emit_json --locked -- --ignored --nocapture`

Expected: FAIL because the metric test is absent.

- [ ] **Step 3: Implement a test-only cadence model**

Inside the existing `events.rs` test module, add a pure helper that models the
current 100 ms poll and draw-before-poll loop for a supplied duration and active
state. The ignored test must print one `COVEN_BENCHMARK_TUI=` JSON line. Keep
`run_event_loop` byte-for-byte behaviorally unchanged; #529 owns its eventual
dirty-frame scheduling replacement.

- [ ] **Step 4: Run focused and existing chat tests**

Run: `cargo test -p coven-cli tui::chat --locked && cargo test -p coven-cli tui::chat::events::tests::benchmark_schedule_metrics_emit_json --locked -- --ignored --nocapture`

Expected: PASS, including the deterministic metric line.

### Task 5: Document and wire artifact-only CI execution

**Files:**
- Modify: `README.md`
- Modify: `.github/workflows/ci.yml`
- Modify: `scripts/benchmark-cli.test.mjs`

- [ ] **Step 1: Write a failing documentation-contract test**

```js
test('README documents the non-gating benchmark invocation', () => {
  assert.match(readFileSync('README.md', 'utf8'), /benchmark-cli\.mjs/);
  assert.match(readFileSync('README.md', 'utf8'), /trend data/i);
});
```

- [ ] **Step 2: Run the test and verify failure**

Run: `node --test scripts/benchmark-cli.test.mjs`

Expected: FAIL because README has no benchmark guidance.

- [ ] **Step 3: Add non-gating guidance and CI artifact upload**

Document the local command, prebuilt-binary prerequisite, isolated-fixture
contract, and that timings are trend data. Add a Linux CI job that builds the
binary, runs three iterations, and uploads the JSON artifact. The job must fail
on runner/fixture/report-shape errors but must contain no wall-clock threshold.

- [ ] **Step 4: Run the complete verification bundle**

Run:

```bash
node --test scripts/benchmark-cli.test.mjs
cargo test -p coven-cli tui::chat --locked
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
git diff --check
```

Expected: every command exits 0.
