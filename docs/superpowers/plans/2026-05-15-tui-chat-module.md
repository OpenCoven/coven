# TUI Chat Module Carve-out Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `crates/coven-cli/src/chat.rs` (1111 lines) into a 4-file module under a new `tui/` namespace, with zero behavior changes.

**Architecture:** Pure code motion. Three sequential commits: (1) scaffold empty new files + wire `mod tui;` into main.rs, (2) move all content from `chat.rs` into the new files while making `chat.rs` a re-export shim, (3) delete `chat.rs` + guardrail and update `main.rs`'s callsite.

**Tech Stack:** Rust 2021 edition. No new dependencies. Same ratatui 0.30 / crossterm 0.29 as Phase 1.

**Spec:** [`docs/superpowers/specs/2026-05-15-tui-chat-module-design.md`](../specs/2026-05-15-tui-chat-module-design.md)

**Branch:** `feat/tui-chat-module`, stacked on `feat/tui-theme-module`. PR cannot merge until #56 lands.

**Worktree:** `/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module`

---

## File Map

| File | Action | Notes |
|---|---|---|
| `crates/coven-cli/src/tui/mod.rs` | **Create** (~10 lines) | Module-level doc + `pub mod chat;` |
| `crates/coven-cli/src/tui/chat/mod.rs` | **Create** (~40 lines) | `pub fn run_chat` + re-exports of `MessageRole`/`ChatMessage`/`AgentInfo` |
| `crates/coven-cli/src/tui/chat/app.rs` | **Create** (~530 lines) | All state, behavior, helpers, tests |
| `crates/coven-cli/src/tui/chat/render.rs` | **Create** (~380 lines) | All 7 `render_*` functions |
| `crates/coven-cli/src/tui/chat/events.rs` | **Create** (~150 lines) | `run_event_loop` |
| `crates/coven-cli/src/chat.rs` | **Delete** (currently 1111 lines) | Replaced by the module above |
| `crates/coven-cli/src/main.rs` | **Modify** (~2 lines) | `mod chat;` → `mod tui;` (alphabetical reorder); `chat::run_chat()` → `tui::chat::run_chat()` |

No other files change. No tests are added; one test (the guardrail) is removed.

---

## Critical working-directory note

ALL `cd`, `cargo`, and `git` commands in this plan run from:

```
/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
```

The first action in every task is to `cd` there and verify `git rev-parse --abbrev-ref HEAD` is `feat/tui-chat-module`. Otherwise STOP and report BLOCKED. (This is the lesson from Phase 1 where some implementer subagents wrote to the main checkout by accident.)

---

## Task 1: Scaffold the new module structure

Create empty/skeleton files for the new module and wire it into `main.rs`. After this task, both `mod chat;` (pointing to the old `chat.rs`) and `mod tui;` (pointing to the new mostly-empty module) coexist. Build passes with warnings about unused items in the new files.

**Files:**
- Create: `crates/coven-cli/src/tui/mod.rs`
- Create: `crates/coven-cli/src/tui/chat/mod.rs`
- Create: `crates/coven-cli/src/tui/chat/app.rs`
- Create: `crates/coven-cli/src/tui/chat/render.rs`
- Create: `crates/coven-cli/src/tui/chat/events.rs`
- Modify: `crates/coven-cli/src/main.rs` (add `mod tui;` declaration)

- [ ] **Step 1: cd into the worktree and verify branch**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
pwd
git rev-parse --abbrev-ref HEAD
```

Expected:
```
/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
feat/tui-chat-module
```

If either differs, STOP and report BLOCKED. Do not modify files outside this worktree.

- [ ] **Step 2: Create `crates/coven-cli/src/tui/mod.rs`**

Write this exact content:

```rust
//! TUI surfaces for the coven CLI. Currently hosts the chat module; Phases 3–4
//! will land the launcher and session-browser carve-outs from main.rs here.

pub mod chat;
```

- [ ] **Step 3: Create `crates/coven-cli/src/tui/chat/mod.rs` as a temporary stub**

This file is a stub for Task 1. It will be filled with `run_chat` and re-exports in Task 2. For now it must compile without warnings even though nothing references its submodules yet.

Write this exact content:

```rust
//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` here manages the raw-terminal
//! lifecycle.

#![allow(dead_code)]

mod app;
mod events;
mod render;
```

The `#![allow(dead_code)]` is temporary — it gets removed in Task 2 Step 5 when `run_chat` lands here and consumes the submodules. The submodules are declared private (`mod`, not `pub mod`) because no code outside `tui::chat` needs to reach into `tui::chat::app::*`.

- [ ] **Step 4: Create three empty submodule files**

Each one must be valid Rust that compiles standalone. Write each file with just a doc comment and a `// placeholder` line (replaced in Task 2):

**`crates/coven-cli/src/tui/chat/app.rs`:**

```rust
//! Chat application state, behavior, and tests. Populated in Task 2 of the
//! chat-module carve-out (see plans/2026-05-15-tui-chat-module.md).

// placeholder — content lands in Task 2
```

**`crates/coven-cli/src/tui/chat/render.rs`:**

```rust
//! Chat TUI render functions. Populated in Task 2 of the chat-module
//! carve-out (see plans/2026-05-15-tui-chat-module.md).

// placeholder — content lands in Task 2
```

**`crates/coven-cli/src/tui/chat/events.rs`:**

```rust
//! Chat TUI event loop. Populated in Task 2 of the chat-module
//! carve-out (see plans/2026-05-15-tui-chat-module.md).

// placeholder — content lands in Task 2
```

- [ ] **Step 5: Add `mod tui;` to main.rs**

Find this block in `crates/coven-cli/src/main.rs` (around lines 31–33 after Phase 1's `mod theme;` insertion):

```rust
mod store;
mod theme;
mod verification;
```

Insert `mod tui;` alphabetically between `theme` and `verification`:

```rust
mod store;
mod theme;
mod tui;
mod verification;
```

Do NOT remove `mod chat;` yet (Task 3 handles that). Both modules coexist after Task 1.

- [ ] **Step 6: Verify the crate builds**

```bash
cargo build -p coven-cli 2>&1 | tail -20
```

Expected: builds cleanly. Some "unused import" warnings on `crate::tui::chat` or its submodules are acceptable in Task 1 — those will be consumed in Task 3.

If you see actual errors (not warnings), STOP and report BLOCKED with the error text.

- [ ] **Step 7: Run all tests**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Expected: all existing tests pass. The chat module (still at `src/chat.rs`) and its tests are untouched. Test count: 172 unit + 4 smoke = 176 (same as Phase 1 end).

- [ ] **Step 8: Commit**

```bash
git add crates/coven-cli/src/tui/ crates/coven-cli/src/main.rs
git commit -m "refactor(tui): scaffold tui/chat module structure

Empty submodule skeleton for the chat carve-out. Old chat.rs remains
the active implementation; this commit only adds the new file tree and
wires mod tui; into main.rs. Task 2 of the chat-module plan moves the
content; Task 3 deletes the old file.
"
```

- [ ] **Step 9: Verify commit landed on the correct branch**

```bash
git log --oneline -2
git rev-parse --abbrev-ref HEAD
```

Expected: the new commit is on top, and HEAD is on `feat/tui-chat-module`. If not, STOP and report.

---

## Task 2: Move all content from `chat.rs` into the new module files

This is the bulk of the work. The strategy: copy each section of the old `chat.rs` into its target new file, fix imports + visibility, then replace `chat.rs` with a re-export shim (`pub use crate::tui::chat::*;`) so the old `chat::run_chat()` callsite in `main.rs` keeps working through Task 2. Task 3 deletes the shim and updates the callsite.

**Files:**
- Modify: `crates/coven-cli/src/tui/chat/mod.rs` (replace stub with run_chat + re-exports)
- Modify: `crates/coven-cli/src/tui/chat/app.rs` (replace placeholder with state code)
- Modify: `crates/coven-cli/src/tui/chat/render.rs` (replace placeholder with renderers)
- Modify: `crates/coven-cli/src/tui/chat/events.rs` (replace placeholder with event loop)
- Modify: `crates/coven-cli/src/chat.rs` (reduce to a re-export shim)

- [ ] **Step 1: cd to worktree and verify branch**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
git rev-parse --abbrev-ref HEAD
```

Expected: `feat/tui-chat-module`. Else STOP.

- [ ] **Step 2: Populate `tui/chat/app.rs`**

Open `crates/coven-cli/src/chat.rs` and copy the following ranges (line numbers reference the **current** chat.rs as of commit `9bcb69a`):

- Lines 33–85 (the data types: `MessageRole`, `ChatMessage`, `AgentInfo`, `InputMode`, `SlashCommandResult`, `App`)
- Line 86 (the `SPINNER_FRAMES` constant)
- Lines 88–457 (the `impl App` block)
- Lines 459–471 (`fn discover_agents`)
- Lines 990–992 (`fn timestamp_now`)
- Lines 994–1002 (`fn truncate_str`)
- Lines 1004–1111 (the entire `#[cfg(test)] mod tests` block)

Replace the placeholder in `crates/coven-cli/src/tui/chat/app.rs` with this content, in this order:

1. Top-of-file doc comment + use statements (replace the imports from chat.rs with only what app.rs needs):

```rust
//! Chat application state, behavior, and helpers. Owns `App` and all of its
//! methods; provides `discover_agents` and the spinner-frame data.

use crate::harness;
```

2. The data types from lines 33–69 of chat.rs. **Visibility changes (from the spec):**
   - `pub enum MessageRole` → keep `pub` (re-exported via mod.rs in next step)
   - `pub struct ChatMessage` → keep `pub`
   - `pub struct AgentInfo` → keep `pub`
   - `enum InputMode` → no change (private, stays `enum`)
   - `enum SlashCommandResult` → no change (private)

3. `struct App` (lines 71–85): change visibility from private to `pub(super)`:

```rust
pub(super) struct App {
    // ... unchanged fields ...
}
```

4. `const SPINNER_FRAMES: &[&str] = ...` (line 86): change to `pub(super)`:

```rust
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
```
(Or copy the exact glyphs from chat.rs:86 — the spinner frames are the same Braille pattern characters.)

5. `impl App` (lines 88–457): paste unchanged.

6. `fn discover_agents` (lines 459–471): change to `pub(super)`:

```rust
pub(super) fn discover_agents() -> Vec<AgentInfo> {
    // ... unchanged body ...
}
```

7. `fn timestamp_now` (lines 990–992): keep private:

```rust
fn timestamp_now() -> String {
    // ... unchanged body ...
}
```

8. `fn truncate_str` (lines 994–1002): keep private:

```rust
fn truncate_str(s: &str, max: usize) -> &str {
    // ... unchanged body ...
}
```

9. The test module (lines 1004–1111) — paste with these surgical changes:
   - **Delete** the `chat_module_stays_single_file_to_avoid_rust_module_ambiguity` test (lines 1035–1059).
   - **Delete** the `use std::path::Path;` import inside `mod tests` (only that test used it; the other tests don't).
   - Keep all four behavioral tests (`unknown_slash_command_returns_command_name_for_feedback`, `handle_input_clears_unknown_slash_command_and_reports_it`, `agent_command_without_argument_opens_picker_on_active_agent`, `unavailable_agent_selection_keeps_current_active_agent`) and both helpers (`app_with_agents`, `agent`) unchanged.

- [ ] **Step 3: Populate `tui/chat/render.rs`**

Copy the following ranges from `chat.rs` into the new `render.rs`:

- Lines 473–510 (`fn render_ui`)
- Lines 512–538 (`fn render_status_bar`)
- Lines 540–636 (`fn render_messages`)
- Lines 638–672 (`fn render_input`)
- Lines 674–700 (`fn render_hint_bar`)
- Lines 702–779 (`fn render_help_overlay`)
- Lines 781–838 (`fn render_agent_select`)

Replace the placeholder in `render.rs` with:

1. Top-of-file doc comment + imports. The renderers need ratatui types and the theme module:

```rust
//! Chat TUI render functions. Pure view code; reads `App` state and emits
//! ratatui widgets. The entry point is `render_ui`; the other render_* fns
//! are private helpers it composes.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use crate::theme::{self, AGENT_LABEL, DIM, HINT_KEY, PRIMARY, PRIMARY_STRONG, SURFACE, SURFACE_STRONG, USER_LABEL};

use super::app::{App, AgentInfo, InputMode, MessageRole, SPINNER_FRAMES};
```

2. Change `fn render_ui` to `pub(super) fn render_ui` (called by `events.rs` next):

```rust
pub(super) fn render_ui(f: &mut Frame, app: &mut App) {
    // ... unchanged body ...
}
```

3. All other `render_*` functions stay private (`fn`, not `pub`). Paste them unchanged.

- [ ] **Step 4: Populate `tui/chat/events.rs`**

Copy lines 863–988 from `chat.rs` (the `run_event_loop` function) into `events.rs`.

Replace the placeholder with:

```rust
//! Chat TUI event loop. Reads keyboard events via crossterm and dispatches
//! to `App` methods; calls `render_ui` between events.

use std::io::Stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::app::{App, SlashCommandResult};
use super::render::render_ui;
```

Then paste `run_event_loop` with this signature change:

```rust
pub(super) fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    // ... unchanged body ...
}
```

(Today's signature in chat.rs line 863 starts the body with `terminal:` and `app:` parameters — keep those.)

- [ ] **Step 5: Populate `tui/chat/mod.rs`**

Replace the stub created in Task 1 with the real content. The mod.rs contains `run_chat` and the public re-exports.

```rust
//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` manages the raw-terminal lifecycle.

mod app;
mod events;
mod render;

// Re-export the public types so callers see them at `tui::chat::*` instead of
// having to reach into `tui::chat::app::*`. Matches the surface of the old
// `chat::*` module from before the carve-out.
pub use app::{AgentInfo, ChatMessage, MessageRole};

use std::io::stdout;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::App;
use events::run_event_loop;
```

Then paste the `pub fn run_chat()` body from chat.rs lines 840–861 unchanged. The body calls `App::new()` and `run_event_loop(...)` — both are imported in the `use` block above, so no body edits are needed.

Remove the `#![allow(dead_code)]` from the top of mod.rs that was added in Task 1.

- [ ] **Step 6: Replace `chat.rs` with a re-export shim**

Replace the entire contents of `crates/coven-cli/src/chat.rs` (1111 lines) with these 3 lines:

```rust
//! Temporary re-export shim during the Phase 2 carve-out. Removed in Task 3
//! of the chat-module plan; do not add new content here.

pub use crate::tui::chat::*;
```

This keeps `main.rs`'s `chat::run_chat()` callsite working (it now resolves to `tui::chat::run_chat` through the glob re-export). The shim deletes in Task 3.

- [ ] **Step 7: Verify the crate builds**

```bash
cargo build -p coven-cli 2>&1 | tail -30
```

Expected: builds cleanly with no errors. Some warnings may remain (e.g. "unused import" if a `use` is now redundant). If you see errors, the most likely cause is:

- A `pub(super)` item that needs `pub` for the shim re-export. The chat.rs shim `pub use crate::tui::chat::*;` re-exports only `pub` items, not `pub(super)`. The items `run_chat`, `MessageRole`, `ChatMessage`, `AgentInfo` must be `pub` at `tui::chat::*` for the shim to find them.
- A missing import in one of the new files. Cross-reference the imports section in each file against what the spec lists.

- [ ] **Step 8: Run all tests**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Expected: **175 unit tests** + 4 smoke tests pass (one fewer unit test than the start of Task 2 — the deleted guardrail).

If a test fails, the most likely cause is the test file no longer compiles (private access to `App` fields was previously legal but now requires the test to be in the same file as `App` — it is, in `app.rs`, so this should work).

- [ ] **Step 9: Commit**

```bash
git add crates/coven-cli/src/tui/ crates/coven-cli/src/chat.rs
git commit -m "refactor(tui): move chat.rs content into tui/chat/* submodule

Pure code motion. chat.rs becomes a re-export shim that points at
crate::tui::chat::* so main.rs's existing chat::run_chat() call keeps
working. The shim and the old mod chat; declaration get deleted in
Task 3 along with the guardrail test (which fails as soon as chat.rs
is removed).
"
```

- [ ] **Step 10: Verify commit on correct branch**

```bash
git log --oneline -3
git rev-parse --abbrev-ref HEAD
```

Expected: new commit on top, HEAD = `feat/tui-chat-module`.

---

## Task 3: Delete `chat.rs` and update `main.rs`

After Task 2, `chat.rs` is just a re-export shim. This task deletes it, removes `mod chat;` from main.rs, updates the callsite to `tui::chat::run_chat()`, and verifies the final acceptance criteria.

**Files:**
- Delete: `crates/coven-cli/src/chat.rs`
- Modify: `crates/coven-cli/src/main.rs` (remove `mod chat;`, update line 150)

- [ ] **Step 1: cd to worktree, verify branch**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
git rev-parse --abbrev-ref HEAD
```

Expected: `feat/tui-chat-module`.

- [ ] **Step 2: Delete `crates/coven-cli/src/chat.rs`**

```bash
rm crates/coven-cli/src/chat.rs
git status --short
```

Expected: shows `D  crates/coven-cli/src/chat.rs`.

- [ ] **Step 3: Update `main.rs` — remove `mod chat;`**

In `crates/coven-cli/src/main.rs`, find the `mod` declarations block (around lines 21–35). Delete the line `mod chat;`. The remaining mod block should look like:

```rust
mod api;
mod control_plane;
mod daemon;
mod harness;
mod openclaw_repo;
mod patch;
mod pc;
mod project;
mod pty_runner;
mod store;
mod theme;
mod tui;
mod verification;
```

(Note: `mod chat;` was between `mod api;` and `mod control_plane;` originally.)

- [ ] **Step 4: Update `main.rs` — change the chat callsite**

In `main.rs` find line 150 (approximate — exact line shifts when `mod chat;` is removed):

```rust
Some(Command::Chat) => chat::run_chat(),
```

Replace with:

```rust
Some(Command::Chat) => tui::chat::run_chat(),
```

This is the **only** callsite in main.rs that uses the chat module. Grep to confirm:

```bash
grep -nE '\bchat::' crates/coven-cli/src/main.rs
```

Expected output: one line, the new `tui::chat::run_chat()`. If you see additional matches, they need replacing too.

- [ ] **Step 5: Verify the crate builds**

```bash
cargo build -p coven-cli 2>&1 | tail -20
```

Expected: builds cleanly with zero warnings.

If you see:
- "unresolved module `chat`" — you missed Step 3 (the `mod chat;` deletion) or Step 4 (the callsite update). Re-grep.
- "file not found: chat.rs" — the build is still looking for chat.rs. Confirm `mod chat;` is gone from main.rs.
- "function `run_chat` is private" — `run_chat` in `tui/chat/mod.rs` is not `pub`. Check Task 2's `mod.rs` content; the `pub fn run_chat` signature is required.

- [ ] **Step 6: Run all tests**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Expected: **175 unit tests + 4 smoke tests pass** (same as Task 2's count).

- [ ] **Step 7: Run clippy**

```bash
cargo clippy -p coven-cli --no-deps 2>&1 | tail -10
```

Expected: zero warnings (no regression from Phase 1's clean state).

- [ ] **Step 8: Verify acceptance criteria via filesystem checks**

```bash
# Criterion 1: chat.rs is gone
test -e crates/coven-cli/src/chat.rs && echo "FAIL: chat.rs still exists" || echo "ok: chat.rs deleted"

# Criterion 2: tui/mod.rs exists with the expected content
cat crates/coven-cli/src/tui/mod.rs

# Criterion 3: tui/chat/ has exactly 4 .rs files
ls crates/coven-cli/src/tui/chat/

# Criterion 4: main.rs uses tui::chat::run_chat
grep -nE 'tui::chat::run_chat|chat::run_chat' crates/coven-cli/src/main.rs
```

Expected:
- `ok: chat.rs deleted`
- `tui/mod.rs` shows the doc comment + `pub mod chat;`
- `ls` shows exactly `mod.rs  app.rs  render.rs  events.rs` (4 files, no extras)
- Last grep shows one line, with `tui::chat::run_chat()`

- [ ] **Step 9: Verify the deleted guardrail test is gone**

```bash
grep -rn 'chat_module_stays_single_file' crates/coven-cli/src/ 2>&1 || echo "ok: guardrail test deleted"
```

Expected: `ok: guardrail test deleted`. If anything matches, the guardrail still exists somewhere (it should have been removed when copying tests into `app.rs` in Task 2 Step 2). Delete it now and re-run.

- [ ] **Step 10: Commit**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/chat.rs
git commit -m "refactor(tui): delete chat.rs shim and finalize chat carve-out

Removes the re-export shim from Task 2, drops mod chat; from main.rs,
and points the Chat command at tui::chat::run_chat() directly. The
guardrail test (which previously prevented this split) was removed in
Task 2 when its containing module file was rewritten.

Acceptance criteria from the design spec all met:
- src/chat.rs deleted
- src/tui/chat/ has exactly mod.rs, app.rs, render.rs, events.rs
- 175 unit + 4 smoke tests pass
- cargo clippy clean
"
```

- [ ] **Step 11: Verify final state**

```bash
git log --oneline -4
git rev-parse --abbrev-ref HEAD
git status --short
```

Expected: 3 new commits on top of Phase 1's tip (`9bcb69a`):
```
<sha3> refactor(tui): delete chat.rs shim and finalize chat carve-out
<sha2> refactor(tui): move chat.rs content into tui/chat/* submodule
<sha1> refactor(tui): scaffold tui/chat module structure
9bcb69a chore(theme): silence dead-code warnings for future-use tokens
```

Branch is `feat/tui-chat-module`. Status is clean (no uncommitted changes).

---

## Done

When Task 3 completes, every acceptance criterion in the spec is met:

1. ✅ `src/chat.rs` no longer exists — Task 3 Step 2.
2. ✅ `src/tui/mod.rs` exists with `pub mod chat;` — Task 1 Step 2.
3. ✅ `src/tui/chat/` contains exactly `mod.rs`, `app.rs`, `render.rs`, `events.rs` — Tasks 1–2.
4. ✅ `src/main.rs` has `mod tui;` and `tui::chat::run_chat()` — Tasks 1 + 3.
5. ✅ `cargo build -p coven-cli` succeeds cleanly — Task 3 Step 5.
6. ✅ `cargo test -p coven-cli` passes; unit count drops by exactly one — Task 3 Step 6.
7. ✅ `cargo clippy -p coven-cli --no-deps` produces zero warnings — Task 3 Step 7.
8. ✅ No item newly exposed beyond today's surface — Tasks 2–3 visibility rules.
9. ⏳ Manual: launching `coven chat` shows the same TUI as before. Not automatable; verify by eye if convenient.

After Task 3, push to origin and open a PR stacked on #56.
