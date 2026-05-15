# TUI Chat Module Carve-out — Design

**Status:** Approved — ready for implementation plan
**Date:** 2026-05-15
**Scope:** Phase 2 of the TUI structural-cleanup effort. Stacked on Phase 1 ([`feat/tui-theme-module`](https://github.com/OpenCoven/coven/pull/56)); cannot merge until that lands.
**Approach:** Pure code motion. No renames, no signature changes, no behavior changes.

---

## Problem

`crates/coven-cli/src/chat.rs` is 1111 lines after Phase 1's theme migration. It is a single file holding the ratatui-based chat TUI's data types, application state, ~370 lines of render code across 7 render functions, the event loop, the public entry point, and tests. The file's responsibilities — state model, view, controller, and lifecycle — are all collocated.

Symptoms that motivate the split:

- The file is unwieldy for navigation and review. Render code (lines 473–838) is a single contiguous block.
- `impl App` (lines 88–457) is 369 lines on its own.
- An existing regression test (`chat_module_stays_single_file_to_avoid_rust_module_ambiguity`, added in `fa786f1`) actively prevents the split — its deletion is the trigger for this work.

The new `crate::theme` module landed in Phase 1 already demonstrates the pattern we want for TUI surfaces: one logical responsibility per file, callsites import via `use`.

## Non-goals

Explicitly out of scope for Phase 2 and must not creep in:

- **Behavior changes** of any kind. Renderers produce the same output. The event loop processes the same keys. The CLI behaves identically.
- **API tightening.** `pub enum MessageRole`, `pub struct ChatMessage`, `pub struct AgentInfo` stay `pub` even though no caller outside the chat module imports them today. Tightening to `pub(super)` is a separate concern (a candidate for a follow-up PR; see Phase 2.1).
- **Helper extraction.** `render_messages` (the largest renderer at ~98 lines) is not refactored. Internal helper functions are not pulled out.
- **Splitting `main.rs`.** Phase 3 — out of scope.
- **Launcher / session-browser carve-out.** Phase 4 — out of scope. (We do create the `tui/` parent module in anticipation, but only `tui::chat` lives under it for now.)
- **New tests.** Phase 2 inherits the existing tests and deletes the guardrail. No new behavioral tests are added.

## Constraints

- **No item from the chat module is newly exposed beyond `tui::chat::run_chat`.** Visibility is preserved exactly as today (pure code motion).
- **The crate continues to be a single binary.** No new workspace members, no library exposure.
- **`cargo clippy -p coven-cli --no-deps` produces zero warnings**, preserving the post–Phase 1 clean state.

## Module layout

```
crates/coven-cli/src/
├── tui/
│   ├── mod.rs            (~10 lines: `pub mod chat;`)
│   └── chat/
│       ├── mod.rs        (~40 lines: pub fn run_chat + raw-terminal lifecycle)
│       ├── app.rs        (~530 lines: state, behavior, helpers, tests)
│       ├── render.rs     (~380 lines: 7 render fns)
│       └── events.rs     (~150 lines: event loop)
├── main.rs   (one edit: `mod chat;` → `mod tui;` and `chat::run_chat()` → `tui::chat::run_chat()`)
└── ... (other files unchanged)
```

`crates/coven-cli/src/chat.rs` is deleted entirely. The Rust compiler enforces non-coexistence of `src/chat.rs` and `src/chat/mod.rs`, so deletion of the single-file form is mandatory once the directory form lands. (We're using the `src/tui/chat/` form, not `src/chat/`, but the principle is the same: no `src/chat.rs` may remain.)

## Per-file content mapping

### `tui/mod.rs` (new)

```rust
//! TUI surfaces for the coven CLI. Currently hosts the chat module; Phases 3–4
//! will land the launcher and session-browser carve-outs from main.rs here.

pub mod chat;
```

### `tui/chat/mod.rs`

Holds the public entry point and the raw-terminal lifecycle (enable raw mode, enter alt screen, build App, run loop, restore terminal on drop).

| From `chat.rs` | New location |
|---|---|
| Lines 840–861 (`pub fn run_chat`) | `tui/chat/mod.rs` |
| (module declarations) | `mod app; mod events; mod render;` |

Imports needed:
```rust
use std::io::stdout;
use anyhow::Result;
use crossterm::{
    execute,
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
```

### `tui/chat/app.rs`

State, behavior, lifecycle helpers, and tests. The "data + methods" half of the module.

| From `chat.rs` | New location | Visibility |
|---|---|---|
| Lines 33–38 (MessageRole) | `app.rs` | `pub` (preserved from today) |
| Lines 40–46 (ChatMessage) | `app.rs` | `pub` (preserved) |
| Lines 48–54 (AgentInfo) | `app.rs` | `pub` (preserved) |
| Lines 56–60 (InputMode) | `app.rs` | private `enum` (preserved) |
| Lines 62–69 (SlashCommandResult) | `app.rs` | private `enum` (preserved) |
| Lines 71–85 (App struct) | `app.rs` | `pub(super)` — was module-private in chat.rs; now must cross the new file boundary into `render.rs` and `events.rs` |
| Line 86 (SPINNER_FRAMES) | `app.rs` | `pub(super)` (used by both `App::tick` and `render_status_bar`) |
| Lines 88–457 (impl App) | `app.rs` | unchanged |
| Lines 459–471 (discover_agents) | `app.rs` | `pub(super)` (called by `run_chat` in `mod.rs`) |
| Lines 990–992 (timestamp_now) | `app.rs` | `pub(super)` is unnecessary — only callers are in `app.rs` itself. Keep private. |
| Lines 994–1002 (truncate_str) | `app.rs` | same — only called by `App::simulate_agent_response`. Keep private. |
| Lines 1004–1111 (mod tests) | `app.rs` (after dropping the guardrail) | `#[cfg(test)] mod tests` |

**Visibility note.** Pure code motion preserves observable behavior. But the items `App`, `SPINNER_FRAMES`, `discover_agents`, `render_ui`, `run_event_loop`, and the `MessageRole`/`ChatMessage`/`AgentInfo` types that were previously module-private (or only crate-pub-but-unused) must now have visibility appropriate to crossing the new submodule boundary. The new visibility is the tightest that still works:

- Items consumed only inside `app.rs`: stay private (timestamp_now, truncate_str, InputMode, SlashCommandResult).
- Items consumed across `app.rs`/`render.rs`/`events.rs`: `pub(super)` (App, SPINNER_FRAMES, MessageRole, AgentInfo, discover_agents).
- Items consumed by `mod.rs`: `pub(super)` (run_event_loop in events.rs, render_ui in render.rs, App + discover_agents).
- The previously-`pub` types `MessageRole`, `ChatMessage`, `AgentInfo`: this is the one judgment call. They were `pub` at the crate level today (visible as `chat::MessageRole` etc.). Approach A's "preserve visibility" goal says they must remain crate-visible after the move. **Decision:** declare them `pub` inside `app.rs`, and re-export them via `pub use app::{MessageRole, ChatMessage, AgentInfo};` in `tui/chat/mod.rs`. The crate-visible path stays short (`tui::chat::ChatMessage` rather than `tui::chat::app::ChatMessage`), matching today's surface modulo the `tui::` prefix.

### `tui/chat/render.rs`

All 7 render functions and SPINNER_FRAMES consumer. Pure view code.

| From `chat.rs` | New location | Visibility |
|---|---|---|
| Lines 473–510 (render_ui) | `render.rs` | `pub(super)` (called by `events.rs` via `run_event_loop`) |
| Lines 512–538 (render_status_bar) | `render.rs` | private `fn` (preserved) |
| Lines 540–636 (render_messages) | `render.rs` | private `fn` |
| Lines 638–672 (render_input) | `render.rs` | private `fn` |
| Lines 674–700 (render_hint_bar) | `render.rs` | private `fn` |
| Lines 702–779 (render_help_overlay) | `render.rs` | private `fn` |
| Lines 781–838 (render_agent_select) | `render.rs` | private `fn` |

Imports needed:
```rust
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

### `tui/chat/events.rs`

The event loop.

| From `chat.rs` | New location | Visibility |
|---|---|---|
| Lines 863–988 (run_event_loop) | `events.rs` | `pub(super)` (called from `run_chat` in `mod.rs`) |

Imports needed:
```rust
use std::io::Stdout;
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use super::app::{App, SlashCommandResult};
use super::render::render_ui;
```

## Public-API view from outside the chat module

After the split, the only crate-visible item is `tui::chat::run_chat` (and the re-exported `MessageRole`, `ChatMessage`, `AgentInfo` types which remain `pub` per the visibility-preservation goal of Approach A). `main.rs` references exactly one of those:

```rust
// crates/coven-cli/src/main.rs line 150 (before):
Some(Command::Chat) => chat::run_chat(),

// after:
Some(Command::Chat) => tui::chat::run_chat(),
```

And the `mod` declaration at line 23 (post–Phase 1):

```rust
// before:
mod chat;

// after:
mod tui;
```

The `mod` declaration's alphabetical position shifts from `mod chat;` (between `mod api;` and `mod control_plane;`) to `mod tui;` (between `mod theme;` and `mod verification;`).

## Tests

### Migration

All five existing tests/helpers (`app_with_agents`, `agent`, plus 4 behavioral tests targeting `App` methods) move into `app.rs`'s `#[cfg(test)] mod tests` block intact.

The guardrail test `chat_module_stays_single_file_to_avoid_rust_module_ambiguity` (chat.rs:1036) is **deleted**. Its purpose was to prevent exactly the split this spec implements. The `use std::path::Path;` import it required is removed with it.

### No replacement guardrail

The Rust compiler itself rejects the only truly-ambiguous case (both `src/tui/chat.rs` and `src/tui/chat/mod.rs` existing simultaneously). A test asserting "these specific files exist with this layout" would be a constraint maintained across every future restructure for no functional benefit.

## Acceptance criteria

Phase 2 is complete when:

1. `crates/coven-cli/src/chat.rs` no longer exists (`git ls-files` returns nothing for it; the working tree has no such file).
2. `crates/coven-cli/src/tui/mod.rs` exists with the single content `pub mod chat;` (plus the module-level doc comment).
3. `crates/coven-cli/src/tui/chat/` contains exactly four files: `mod.rs`, `app.rs`, `render.rs`, `events.rs`. No others.
4. `crates/coven-cli/src/main.rs` has `mod tui;` (alphabetical position adjusted) and `tui::chat::run_chat()` at line 150.
5. `cargo build -p coven-cli` succeeds with zero warnings.
6. `cargo test -p coven-cli` passes; the unit-test count drops by exactly one (the deleted guardrail test). Smoke tests pass at 4.
7. `cargo clippy -p coven-cli --no-deps` produces zero warnings.
8. No item from the chat module is newly exposed beyond `tui::chat::run_chat`, `tui::chat::ChatMessage`, `tui::chat::AgentInfo`, `tui::chat::MessageRole` (the three re-exported types from today's surface).
9. Manual smoke check: launching `coven chat` opens the TUI and renders without visible regressions (same colors, same layout, same key bindings).

## Estimated diff scale

| File | Action | Lines |
|---|---|---|
| `crates/coven-cli/src/chat.rs` | Delete | -1111 |
| `crates/coven-cli/src/tui/mod.rs` | Create | ~10 |
| `crates/coven-cli/src/tui/chat/mod.rs` | Create | ~40 |
| `crates/coven-cli/src/tui/chat/app.rs` | Create | ~530 |
| `crates/coven-cli/src/tui/chat/render.rs` | Create | ~380 |
| `crates/coven-cli/src/tui/chat/events.rs` | Create | ~150 |
| `crates/coven-cli/src/main.rs` | 1-line edit + 1 mod swap | ±2 |

Net: ~0 lines (file is reorganized, not reduced). The deleted guardrail test removes ~25 lines from the running total.
