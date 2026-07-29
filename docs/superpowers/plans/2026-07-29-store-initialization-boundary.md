# Store Initialization Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move SQLite schema and migration work out of daemon request opens while preserving fresh-install, upgrade, and concurrent-start guarantees.

**Architecture:** Split `store::open_store` into a lightweight connection open and an explicit idempotent `store::initialize_store` boundary. Daemon startup runs the initializer while holding its existing serve lock; a process-local initialized-path cache keeps legacy CLI callers safe without repeating schema work for request connections. Initialization takes a SQLite immediate transaction so independently-started callers serialize migrations.

**Tech Stack:** Rust, rusqlite, SQLite WAL/transactions, existing daemon serve lock, Node benchmark harness.

---

### Task 1: Define and test the initialization boundary

**Files:**
- Modify: `crates/coven-cli/src/store.rs:332-604`
- Test: `crates/coven-cli/src/store.rs:3070-3475`

- [ ] **Step 1: Write failing boundary tests**

Add tests proving `open_initialized_store` does not create schema, `initialize_store` creates a usable fresh database, and repeated initialization preserves a session record.

```rust
assert!(open_initialized_store(&path)?.execute("SELECT 1 FROM sessions", []).is_err());
initialize_store(&path)?;
let conn = open_initialized_store(&path)?;
assert_eq!(list_sessions(&conn)?.len(), 0);
```

- [ ] **Step 2: Run the new test before implementation**

Run: `cargo test -p coven-cli store_initialization_boundary -- --nocapture`

Expected: FAIL because `open_initialized_store` and `initialize_store` do not exist.

- [ ] **Step 3: Implement explicit initialization and lightweight opens**

Extract the existing parent creation, WAL configuration, schema DDL, compatibility columns, Ward schemas, and FTS backfill into `initialize_store`. Wrap that sequence in `BEGIN IMMEDIATE` / `COMMIT`, rolling back if any required migration fails. Keep `open_initialized_store` limited to `Connection::open`, `busy_timeout`, and `foreign_keys`; it must not execute schema DDL or `journal_mode`.

- [ ] **Step 4: Keep CLI callers backward-compatible without request-path work**

Make `open_store` initialize an unseen path once per process, then delegate to `open_initialized_store`. Record only successful initialization in a synchronized path cache, so standalone CLI commands still initialize fresh or upgraded stores and repeated daemon request opens only take the lightweight branch.

- [ ] **Step 5: Run focused store tests**

Run: `cargo test -p coven-cli store::tests -- --nocapture`

Expected: PASS, including legacy Ward schema and FTS migration tests.

### Task 2: Establish daemon startup as the authoritative initialization point

**Files:**
- Modify: `crates/coven-cli/src/daemon.rs:2191-2235`
- Test: `crates/coven-cli/src/daemon.rs` daemon serve tests

- [ ] **Step 1: Write a failing daemon-startup regression**

Add a helper-level test that calls the daemon initialization seam for a temporary Coven home, then verifies a subsequent request-style store open can read the sessions table.

```rust
initialize_daemon_store(temp.path())?;
let conn = store::open_initialized_store(&temp.path().join("coven.sqlite3"))?;
assert!(store::list_sessions(&conn)?.is_empty());
```

- [ ] **Step 2: Run the regression before implementation**

Run: `cargo test -p coven-cli daemon_store_initialization -- --nocapture`

Expected: FAIL because the startup seam is absent.

- [ ] **Step 3: Initialize after the serve lock and before socket acceptance**

Call the new store initializer in `serve_forever` immediately after `acquire_serve_lock` and before `bind_api_socket`. Keep the recovery sweep, scheduler startup, API listeners, WAL, and status-file ordering otherwise unchanged.

- [ ] **Step 4: Run daemon and API focused regressions**

Run: `cargo test -p coven-cli 'daemon::tests|api::tests::sessions_endpoint' -- --nocapture`

Expected: PASS.

### Task 3: Verify concurrency and measurable request-path behavior

**Files:**
- Modify: `crates/coven-cli/src/store.rs` tests and test-only telemetry

- [ ] **Step 1: Add concurrent initialization coverage**

Spawn two threads that call `initialize_store` on the same fresh path, join both successfully, then open the resulting store and verify the current Ward component version and sessions table are present.

```rust
let handles = (0..2).map(|_| std::thread::spawn(move || initialize_store(&path))).collect::<Vec<_>>();
for handle in handles { handle.join().expect("thread panicked")?; }
```

- [ ] **Step 2: Add request-open telemetry regression**

Expose test-only initialization-count helpers. Initialize once, perform multiple `open_store` calls, and assert the count remains one; this proves request-path opens do not replay DDL/migrations.

- [ ] **Step 3: Run the existing repeated bounded sessions-read benchmark**

Measure repeated `/api/v1/sessions?limit=100` requests against a daemon that has already completed startup initialization. The existing benchmark already has this scenario and status-code assertion, so record its result without changing the harness.

- [ ] **Step 4: Run focused tests and the benchmark**

Run: `cargo test -p coven-cli store_initialization -- --nocapture` and the existing benchmark command.

Expected: PASS.

### Task 4: Validate, commit, and deliver

**Files:**
- Modify: `crates/coven-cli/src/store.rs`
- Modify: `crates/coven-cli/src/daemon.rs`

- [ ] **Step 1: Run full repository gates**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --locked && python scripts/check-secrets.py`

Expected: every command exits 0.

- [ ] **Step 2: Run staged privacy validation**

Run: `python3 scripts/check-coven-privacy.py --staged`

Expected: `Coven privacy guard passed`.

- [ ] **Step 3: Commit and open the issue-linked PR**

Run: `git commit -m "perf: move store initialization off request paths"` followed by `git push -u origin perf/527-store-initialization` and `gh pr create --fill --body "Closes #527"`.

Expected: a PR exists with green required checks.

- [ ] **Step 4: Resolve review feedback and merge**

Reply to each verified Copilot thread, resolve only addressed threads, re-run required gates after every code change, then squash merge only with a clean merge state and green checks. Release `issue-527` after verifying the issue is closed.
