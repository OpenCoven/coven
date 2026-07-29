# Paginated Session History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the default session API and chat overlay bounded at 10,000 stored sessions while preserving explicit CLI and API all-history workflows.

**Architecture:** Introduce a keyset-paginated `SessionPage` at the store boundary, ordered by the existing `(created_at DESC, id DESC)` contract. Keep the legacy bare-array API response for unqualified `GET /api/v1/sessions`; return the new envelope only when `limit` or `cursor` is requested, and move the chat client to that bounded envelope. Cache owned overlay entries in `App` whenever its session list changes, so rendering only reads the cache.

**Tech Stack:** Rust, rusqlite keyset queries, serde JSON, Ratatui TUI, Node benchmark harness.

---

### Task 1: Define and test the store pagination contract

**Files:**
- Modify: `crates/coven-cli/src/store.rs:1928-1971`
- Test: `crates/coven-cli/src/store.rs` unit-test module

- [ ] **Step 1: Write failing store tests for the ordered first page, cursor continuation, archived filter, and invalid limits.**

```rust
let first = list_session_page(&conn, SessionListQuery { limit: 2, cursor: None, include_archived: false })?;
assert_eq!(ids(&first.sessions), ["newest", "middle"]);
assert_eq!(first.next_cursor.as_deref(), Some("<middle-created-at>|middle"));

let second = list_session_page(&conn, SessionListQuery { limit: 2, cursor: first.next_cursor.as_deref(), include_archived: false })?;
assert_eq!(ids(&second.sessions), ["oldest"]);
assert!(second.next_cursor.is_none());
```

- [ ] **Step 2: Run the focused tests and verify they fail because `SessionListQuery` and `list_session_page` do not exist.**

Run: `cargo test -p coven-cli store::tests::list_session_page`

Expected: compile failure naming the missing pagination API.

- [ ] **Step 3: Add the minimal store types and keyset query.**

```rust
pub const DEFAULT_SESSION_PAGE_LIMIT: usize = 100;
pub const MAX_SESSION_PAGE_LIMIT: usize = 1_000;

pub struct SessionListQuery<'a> {
    pub limit: usize,
    pub cursor: Option<&'a str>,
    pub include_archived: bool,
}

pub struct SessionPage {
    pub sessions: Vec<SessionRecord>,
    pub next_cursor: Option<String>,
}
```

Use `LIMIT limit + 1`, retain the first `limit` rows, and encode/decode the last returned row's `(created_at, id)` as URL-safe base64 JSON so a caller-supplied ID cannot make a delimiter cursor ambiguous. The continuation predicate must be `(created_at < ?1 OR (created_at = ?1 AND id < ?2))`, matching the existing descending ordering. Reject zero, over-`MAX_SESSION_PAGE_LIMIT`, and malformed cursors before querying.

- [ ] **Step 4: Run the focused store tests and verify they pass.**

Run: `cargo test -p coven-cli store::tests::list_session_page`

Expected: PASS.

- [ ] **Step 5: Commit the store-only change.**

```bash
git add crates/coven-cli/src/store.rs
git commit -m "feat: paginate session store listings"
```

### Task 2: Add the backward-compatible API envelope

**Files:**
- Modify: `crates/coven-cli/src/api.rs:639-644`
- Test: `crates/coven-cli/src/api.rs` unit-test module

- [ ] **Step 1: Write failing API tests for legacy and paginated responses.**

```rust
let legacy = handle_request("GET", "/api/v1/sessions", home, None)?;
assert!(serde_json::from_str::<Vec<store::SessionRecord>>(&legacy.body).is_ok());

let page = handle_request("GET", "/api/v1/sessions?limit=2", home, None)?;
let page: SessionPageResponse = serde_json::from_str(&page.body)?;
assert_eq!(page.sessions.len(), 2);
assert!(page.next_cursor.is_some());
```

- [ ] **Step 2: Run the focused API tests and verify they fail because paginated session responses are not implemented.**

Run: `cargo test -p coven-cli api::tests::get_sessions`

Expected: assertion failure because the response is still an array.

- [ ] **Step 3: Parse `limit`, `cursor`, and `includeArchived` only for `GET /sessions`.**

Keep an unqualified request on `store::list_sessions` so existing clients keep their array response. For a request with `limit` or `cursor`, call `list_session_page` and serialize:

```rust
#[derive(Serialize, Deserialize)]
pub struct SessionPageResponse {
    pub sessions: Vec<store::SessionRecord>,
    pub next_cursor: Option<String>,
}
```

Reject malformed query values with the existing structured 400-response path. `includeArchived=true` is supported only in the explicit paginated path; local `coven sessions --all` remains its existing full-history store workflow.

- [ ] **Step 4: Run the focused API tests and verify they pass.**

Run: `cargo test -p coven-cli api::tests::get_sessions`

Expected: PASS.

- [ ] **Step 5: Commit the API change.**

```bash
git add crates/coven-cli/src/api.rs
git commit -m "feat: add paginated sessions API"
```

### Task 3: Move the chat overlay to bounded sessions and cache rows

**Files:**
- Modify: `crates/coven-cli/src/tui/chat/client.rs:102-106,274-276`
- Modify: `crates/coven-cli/src/tui/chat/app.rs:227-228,1557-1565`
- Modify: `crates/coven-cli/src/tui/chat/render.rs:1013-1192`
- Test: `crates/coven-cli/src/tui/chat/app.rs` and `crates/coven-cli/src/tui/chat/render.rs` unit-test modules

- [ ] **Step 1: Write failing chat tests for the bounded endpoint and cache invalidation.**

```rust
app.refresh_sessions();
assert_eq!(client.calls.borrow().last(), Some(&"GET /api/v1/sessions?limit=100".into()));
assert_eq!(app.session_overlay_entries.len(), 1);

app.remove_session_from_list("turn-2");
assert_eq!(app.session_overlay_entries.len(), 1);
assert_eq!(app.session_overlay_entries[0].turn_count, 1);
```

Also construct 10,000 records in a render test, refresh once, render twice, and assert the cached entry set is reused rather than calling the collapse helper during either frame.

- [ ] **Step 2: Run the focused chat tests and verify they fail.**

Run: `cargo test -p coven-cli tui::chat::`

Expected: assertion failure because the client requests the unbounded endpoint and the renderer rebuilds entries per frame.

- [ ] **Step 3: Add `list_recent_sessions(limit)` to `ChatClient` and cache owned overlay rows in `App`.**

Make `DaemonChatClient` request `/api/v1/sessions?limit=100` and deserialize `SessionPageResponse`. Convert `SessionOverlayEntry` to an owned, render-ready type containing only the representative session index/id and turn count, rebuild it inside a single `rebuild_session_overlay_entries` helper, and call that helper after `refresh_sessions`, local removal, and any local insert/update that changes `sessions`.

`render_session_overlay` must iterate `app.session_overlay_entries` directly; it must not call a collapse helper or allocate grouping maps during a frame.

- [ ] **Step 4: Run the focused chat tests and verify they pass.**

Run: `cargo test -p coven-cli tui::chat::`

Expected: PASS.

- [ ] **Step 5: Commit the chat change.**

```bash
git add crates/coven-cli/src/tui/chat/app.rs crates/coven-cli/src/tui/chat/client.rs crates/coven-cli/src/tui/chat/render.rs
git commit -m "perf: bound and cache chat session overlay"
```

### Task 4: Measure and validate the 10k default path

**Files:**
- Modify: `scripts/benchmark-cli.mjs:535-540`
- Modify: `scripts/benchmark-cli.test.mjs`

- [ ] **Step 1: Write a failing benchmark-helper test that expects the default sessions scenario to use a bounded page.**

```js
assert.equal(sessionListRequest().path, '/api/v1/sessions?limit=100');
```

- [ ] **Step 2: Run the focused benchmark tests and verify they fail.**

Run: `node --test scripts/benchmark-cli.test.mjs --test-name-pattern "session.*list"`

Expected: assertion failure because the measured path is `/api/v1/sessions`.

- [ ] **Step 3: Change only the default performance scenario to measure the bounded API.**

Keep a separate explicit all-history measurement if the benchmark needs historical comparison. Record the returned session count and response bytes in the JSON report so the 10k fixture proves the default payload is bounded.

- [ ] **Step 4: Run the focused benchmark tests and one real report.**

Run: `node --test scripts/benchmark-cli.test.mjs --test-name-pattern "session.*list"`

Run: `node scripts/benchmark-cli.mjs --binary target/debug/coven --iterations=1 --output /tmp/coven-issue-526-benchmark.json`

Expected: PASS; `sessions_10000` reports the bounded default request and response.

- [ ] **Step 5: Commit the benchmark update.**

```bash
git add scripts/benchmark-cli.mjs scripts/benchmark-cli.test.mjs
git commit -m "perf: measure bounded session listings"
```

### Task 5: Run repository gates and publish

**Files:**
- Modify: only files from Tasks 1-4

- [ ] **Step 1: Rebase on current `origin/main` and inspect the complete diff.**

Run: `git fetch origin main && git rebase origin/main && git diff --check && git status --short`

Expected: clean rebase and no whitespace errors.

- [ ] **Step 2: Run the required gates.**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --locked && python scripts/check-secrets.py`

Expected: all commands exit 0.

- [ ] **Step 3: Stage the scoped files and run the staged privacy guard.**

Run: `git add crates/coven-cli/src/store.rs crates/coven-cli/src/api.rs crates/coven-cli/src/tui/chat/app.rs crates/coven-cli/src/tui/chat/client.rs crates/coven-cli/src/tui/chat/render.rs scripts/benchmark-cli.mjs scripts/benchmark-cli.test.mjs && python3 scripts/check-coven-privacy.py --staged`

Expected: `Coven privacy guard passed`.

- [ ] **Step 4: Push and open a PR that closes #526.**

Run: `git push -u origin perf/526-paginate-session-history`

Expected: branch is available for a PR with the benchmark evidence and compatibility note.

## Self-review

- Spec coverage: Task 1 covers pagination, deterministic ordering, archive filtering, and 10k-safe query bounds. Task 2 preserves legacy API clients and adds the cursor response. Task 3 moves the overlay to the bounded path and invalidates its cache only when session state changes. Task 4 captures the required measurement evidence. Task 5 applies the repository gates and PR workflow.
- Placeholder scan: no TODO/TBD or generic test steps remain; each task names paths, test behavior, and commands.
- Type consistency: `SessionListQuery`, `SessionPage`, and `SessionPageResponse` are introduced before callers use them; the chat client is the sole consumer of the envelope in this change.
