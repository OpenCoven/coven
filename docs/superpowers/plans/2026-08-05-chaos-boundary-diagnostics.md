# Chaos Benchmark Event Writer Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent transient SQLite writer contention from killing event
ingestion, and turn any remaining `sessions_32` output timeout into actionable
boundary evidence.

**Architecture:** The daemon's single event-writer worker retries only
`SQLITE_BUSY` and `SQLITE_LOCKED` commit failures within a fixed attempt budget,
retaining its batch until success or final failure. The existing benchmark
remains authoritative for scenario execution; small exported formatting helpers
make diagnostic behavior directly unit-testable.

**Tech Stack:** Rust, rusqlite, Node.js ESM, Node test runner, Coven local socket
API

---

### Task 1: Add bounded boundary diagnostics

**Files:**
- Modify: `scripts/benchmark-chaos.mjs`
- Modify: `scripts/benchmark-chaos.test.mjs`

- [x] **Step 1: Write failing diagnostic tests**

Add tests for the harness marker ordering, event-writer health formatting, and
the combined timeout evidence.

- [x] **Step 2: Verify the tests fail**

Run:

```bash
node --test scripts/benchmark-chaos.test.mjs
```

Expected: FAIL because the diagnostic helpers do not exist.

- [x] **Step 3: Implement the marker and diagnostic helpers**

Write the fixed marker before harness output, count markers on timeout, query
`/api/v1/health`, and append bounded event-writer fields to the existing session
diagnostic.

- [x] **Step 4: Run benchmark unit tests**

Run:

```bash
node --test scripts/benchmark-cli.test.mjs scripts/benchmark-chaos.test.mjs
```

Expected: PASS.

- [x] **Step 5: Run repository gates**

Run the Rust formatting, clippy, workspace tests, secret scan, and staged
privacy guard required by `AGENTS.md`.

### Task 2: Recover from transient SQLite writer locks

**Files:**
- Modify: `crates/coven-cli/src/event_writer.rs`

- [x] **Step 1: Write failing retry tests**

Create real competing SQLite connections and cover transient lock recovery,
persistent lock exhaustion, and non-lock fail-fast behavior.

- [x] **Step 2: Verify the tests fail**

Run the focused event-writer test filter before defining the retry helper.

- [x] **Step 3: Implement bounded lock-only retry**

Retry a batch only when the error chain preserves a rusqlite
`DatabaseBusy`/`DatabaseLocked` code. Keep the batch owned by the worker and
preserve the existing permanent failure path after the final attempt.

- [x] **Step 4: Run event-writer tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
```

Expected: PASS.

### Task 3: Prove the real concurrency path and publish

- [x] **Step 1: Run the real 1/8/32 collector repeatedly**

Build the debug CLI and run `scripts/benchmark-chaos.mjs` at least three times.
Every report must include passing `sessions_1`, `sessions_8`, and `sessions_32`
scenarios.

- [x] **Step 2: Run repository gates**

Run formatting, clippy, full workspace tests, benchmark unit tests, secret scan,
and the staged privacy guard.

- [ ] **Step 3: Commit, push, and open a PR**

Open a focused PR that links and closes issue #615, explains the lock-only retry
boundary, and includes the repeated real-benchmark evidence.
