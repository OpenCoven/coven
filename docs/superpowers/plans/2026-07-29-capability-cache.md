# Capability Discovery Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make capability discovery return a complete cached response on hot reads without holding a lock during filesystem scans.

**Architecture:** Key immutable complete response snapshots by both the Coven home and harness home. A `RwLock<HashMap<CacheKey, CapabilitySnapshot>>` serves valid snapshots under a shared lock. Expired, missing, or forced refreshes build the whole response outside all cache locks, then atomically replace one key's snapshot under a short write lock.

**Tech Stack:** Rust, `std::sync::RwLock`, `HashMap`, existing capability scanners, Node benchmark harness.

---

### Task 1: Define a complete, path-scoped cache snapshot

**Files:**
- Modify: `crates/coven-cli/src/capabilities.rs:13-205`
- Test: `crates/coven-cli/src/capabilities.rs` test module

- [x] **Step 1: Write failing cache-boundary tests**

Add fixture helpers and tests that exercise a closure-based refresh seam without changing process `HOME`:

```rust
#[test]
fn fresh_cache_hit_returns_the_complete_snapshot_without_building() {
    let _serial = cache_test_lock().lock().unwrap();
    let key = CacheKey::new(PathBuf::from("/coven"), PathBuf::from("/harness"));
    clear_cache_for_tests();
    let expected = fixture_response("first", "2026-07-29T00:00:00Z");

    assert_eq!(get_or_refresh(key.clone(), false, || expected.clone()).scanned_at, expected.scanned_at);
    let hit = get_or_refresh(key, false, || panic!("fresh cache must not build"));
    assert_eq!(hit.coven_skills[0].id, expected.coven_skills[0].id);
    assert_eq!(
        hit.harness_capabilities[0].harness_id,
        expected.harness_capabilities[0].harness_id
    );
}
```

Add a second test which starts `get_or_refresh(key, true, ...)` in a thread whose builder waits on a channel; while it waits, call `get_or_refresh(key, false, || panic!(...))` and assert it returns the old snapshot before releasing the refresh thread. Add a third test that refreshes a fixture response, then checks `get_one_for_home` returns the manifest from that same replacement response.

- [x] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p coven-cli 'capabilities::tests::fresh_cache_hit_returns_the_complete_snapshot_without_building|capabilities::tests::refresh_does_not_block_fresh_reader|capabilities::tests::get_one_reads_the_published_response_snapshot' -- --nocapture`

Expected: FAIL because `CacheKey`, `get_or_refresh`, and the test helpers do not exist.

- [x] **Step 3: Replace the manifest-only mutex cache**

Replace `CapabilityCache`, `Mutex`, and `cache()` with these types and helpers:

```rust
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    coven_home: PathBuf,
    harness_home: PathBuf,
}

#[derive(Clone)]
struct CapabilitySnapshot {
    response: CapabilitiesResponse,
    built_at: Instant,
}

static CACHE: OnceLock<RwLock<HashMap<CacheKey, CapabilitySnapshot>>> = OnceLock::new();

fn get_or_refresh(
    key: CacheKey,
    refresh: bool,
    build: impl FnOnce() -> CapabilitiesResponse,
) -> CapabilitiesResponse {
    if !refresh {
        let guard = cache().read().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(snapshot) = guard.get(&key).filter(|snapshot| snapshot.built_at.elapsed() < CACHE_TTL) {
            return snapshot.response.clone();
        }
    }

    let response = build();
    cache().write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(
        key,
        CapabilitySnapshot { response: response.clone(), built_at: Instant::now() },
    );
    response
}
```

Keep the builder invocation outside both lock scopes. Do not introduce a refresh mutex: duplicate concurrent scans are acceptable and keep readers non-blocking.

- [x] **Step 4: Run the cache-boundary tests**

Run: `cargo test -p coven-cli capabilities::tests -- --nocapture`

Expected: PASS, including the new hot-hit, atomic replacement, and blocked-refresh reader tests.

- [x] **Step 5: Commit the cache boundary**

```bash
git add crates/coven-cli/src/capabilities.rs
git commit -m "perf: cache complete capability discovery snapshots"
```

### Task 2: Route all capability reads through the immutable response

**Files:**
- Modify: `crates/coven-cli/src/capabilities.rs:130-205`
- Test: `crates/coven-cli/src/capabilities.rs` test module
- Test: `crates/coven-cli/src/api.rs:7170-7235`

- [x] **Step 1: Write failing route-semantics tests**

Add a test that supplies distinct fixture responses to `get_all_for_home` and verifies a warm response preserves both `coven_skills` and `scanned_at`. Add an API test that requests `/api/v1/capabilities/harnesses?refresh=1` after a warm request and verifies the response remains a complete aggregate.

```rust
let response = get_all_for_home(coven_home, harness_home, false);
assert_eq!(response.scanned_at, "2026-07-29T00:00:00Z");
assert_eq!(response.coven_skills, vec![fixture_skill("cached")]);
assert_eq!(response.harness_capabilities[0].harness_id, "codex");
```

- [x] **Step 2: Run the focused route tests to verify the missing behavior**

Run: `cargo test -p coven-cli 'capabilities::tests::warm_response_keeps_skills_and_scan_time|api::tests::harness_capability_aggregate_accepts_refresh_query' -- --nocapture`

Expected: FAIL because hot responses still call `scan_skills` and regenerate `scanned_at`.

- [x] **Step 3: Build and publish a complete response**

Factor scanner collection into `build_response(coven_home, harness_home) -> CapabilitiesResponse`. It must call every existing harness scanner, collect `scan_skills(coven_home).unwrap_or_default()`, and assign `utc_now_iso()` once. Implement:

```rust
pub fn get_all(coven_home: &Path, refresh: bool) -> CapabilitiesResponse {
    let harness_home = dirs_home();
    get_all_for_home(coven_home, &harness_home, refresh)
}

fn get_one_for_home(
    coven_home: &Path,
    harness_home: &Path,
    harness_id: &str,
    refresh: bool,
) -> Option<HarnessCapabilityManifest> {
    get_all_for_home(coven_home, harness_home, refresh)
        .harness_capabilities
        .into_iter()
        .find(|manifest| manifest.harness_id == harness_id)
}
```

Make `get_one` call this helper rather than take a second cache lock. Update `invalidate` to clear all keyed snapshots. Preserve the existing missing-harness `None` behavior and scanner warning/error treatment.

- [x] **Step 4: Run route and capability tests**

Run: `cargo test -p coven-cli 'capabilities::tests|api::tests::routes_harness_capability_aggregate_to_json|api::tests::harness_capability_aggregate_accepts_refresh_query|api::tests::routes_single_harness_capability_manifest' -- --nocapture`

Expected: PASS.

- [x] **Step 5: Commit the response integration**

```bash
git add crates/coven-cli/src/capabilities.rs crates/coven-cli/src/api.rs
git commit -m "perf: serve hot capability responses from snapshots"
```

### Task 3: Add a benchmarked hot capability route

**Files:**
- Modify: `scripts/benchmark-cli.mjs:508-663`
- Test: `scripts/benchmark-cli.test.mjs:430-530`

- [x] **Step 1: Write the failing benchmark-harness test**

Extend `collectBenchmarkScenarios`'s dependency injection with `measureCapabilities` and assert the aggregate report contains a `capabilities_hot` response after startup:

```js
assert.deepEqual(Object.keys(scenarios), [
  'help', 'harness_first_output', 'event_tail_2', 'sessions_2', 'capabilities_hot'
]);
assert.deepEqual(calls.at(-1), ['capabilities', '/fixture/root', 2]);
```

- [x] **Step 2: Run the Node test to verify it fails**

Run: `node --test scripts/benchmark-cli.test.mjs --test-name-pattern='collectBenchmarkScenarios merges core and daemon fixture reports'`

Expected: FAIL because no capability measurement is injected or reported.

- [x] **Step 3: Add the warmed capability measurement**

Add `measureCapabilityReads` that starts an isolated daemon, makes one unmeasured `GET /api/v1/capabilities/harnesses` warm-up call, then calls `runSocketScenario` on the same path for the requested iterations. Wire its report into `collectBenchmarkScenarios` as `capabilities_hot` when session fixtures are enabled. Always stop the daemon in `finally`.

```js
await measure({ socketPath, path: '/api/v1/capabilities/harnesses', iterations: 1 });
return measure({ socketPath, path: '/api/v1/capabilities/harnesses', iterations });
```

- [x] **Step 4: Run benchmark unit tests and a local report**

Run:

```bash
node --test scripts/benchmark-cli.test.mjs
CARGO_TARGET_DIR=/tmp/coven-issue-528-check cargo build -p coven-cli --locked
node scripts/benchmark-cli.mjs --binary /tmp/coven-issue-528-check/debug/coven --iterations=3 --session-counts=100 --output /tmp/coven-issue-528-benchmark.json
jq '.scenarios.capabilities_hot' /tmp/coven-issue-528-benchmark.json
```

Expected: tests pass and the report contains only timing, status-code, and summary data for `capabilities_hot`.

- [x] **Step 5: Commit the performance evidence**

```bash
git add scripts/benchmark-cli.mjs scripts/benchmark-cli.test.mjs
git commit -m "perf: benchmark hot capability discovery"
```

### Task 4: Validate and deliver issue #528

**Files:**
- Modify: `crates/coven-cli/src/capabilities.rs`
- Modify: `scripts/benchmark-cli.mjs`
- Modify: `scripts/benchmark-cli.test.mjs`

- [x] **Step 1: Run all repository gates**

Run:

```bash
CARGO_TARGET_DIR=/tmp/coven-issue-528-check cargo fmt --check
CARGO_TARGET_DIR=/tmp/coven-issue-528-check cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=/tmp/coven-issue-528-check cargo test --workspace --locked
node --test scripts/benchmark-cli.test.mjs
python scripts/check-secrets.py
```

Expected: every command exits 0.

- [ ] **Step 2: Stage and validate privacy**

Run:

```bash
git add crates/coven-cli/src/capabilities.rs scripts/benchmark-cli.mjs scripts/benchmark-cli.test.mjs docs/superpowers/specs/2026-07-29-capability-cache-design.md docs/superpowers/plans/2026-07-29-capability-cache.md
git diff --cached --check
python3 scripts/check-coven-privacy.py --staged
```

Expected: no whitespace errors and `Coven privacy guard passed`.

- [ ] **Step 3: Open the issue-linked PR**

Run:

```bash
git push -u origin perf/528-capability-cache
gh pr create --repo OpenCoven/coven --base main --title "perf: cache capability discovery snapshots" --body "Closes #528"
```

Expected: a single-purpose PR exists and links issue #528.

- [ ] **Step 4: Resolve review feedback and merge**

For each actionable Copilot or maintainer review thread: make the smallest verified fix, reply with the commit and test evidence, resolve the thread only after the fix is pushed, and wait for all required checks. Then verify `mergeStateStatus` is `CLEAN` and squash merge the PR:

```bash
gh pr merge <number> --repo OpenCoven/coven --squash --subject "perf: cache capability discovery snapshots" --body "Closes #528."
```

Expected: the PR is merged, #528 is closed, and `COVEN_AGENT_ID=buns coven claim release issue-528` succeeds.
