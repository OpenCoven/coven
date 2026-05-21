---
title: "Coven Chat-First Terminal UI"
description: "Redesign Coven's default terminal experience as a chat-first coding-agent TUI while preserving Cast planning, safety gates, and daemon-backed sessions."
---

# Coven Chat-First Terminal UI — Design Plan

Status: Approved direction, ready for implementation
Date: 2026-05-20
Scope: Replace the default `coven` / `coven tui` launcher-first experience with a chat-first terminal UI. Make `coven chat` use the same interaction model.

## Summary

Coven should open like a working coding-agent terminal, not like a command launcher. The default interactive experience should be a full-screen chat UI with a compact status row, transcript, composer, active session state, and inline Cast planning.

## Problem

The current default Coven terminal flow opens a raw-terminal Cast launcher with example spells and action rows. It makes Coven feel like a command menu instead of a coding-agent terminal. A chat-first terminal should let the user land directly in a working loop: see project/runtime/session context, type a natural task immediately, stream agent output into a transcript, approve risky actions inline, and discover commands progressively.

Coven already has most of the required backend pieces:

- `tui::shell` — current default launcher and Cast dispatch
- `tui::chat` — ratatui chat surface, transcript, composer, daemon session launch, attach, input forwarding, polling, history, help, session overlay
- `tui::cast` — spell parsing, planning, safety decisions, plan cards, quest flow, outcomes, daemon-backed follow behavior

## Goals

1. Make `coven` and `coven tui` open a chat-first full-screen TUI in interactive terminals.
2. Make `coven chat` use the same experience.
3. Preserve Cast as the planning, safety, and dispatch layer.
4. Preserve daemon-backed launch, attach, replay, follow, kill, export, and event polling.
5. Move command discovery into `/help`, `/commands`, and `Ctrl+K`.
6. Keep non-interactive output script-friendly by retaining the existing plain Cast frame.
7. Keep implementation testable with snapshots, focused state tests, and daemon/client fakes.

## Non-Goals

- Do not clone Claude Code branding, exact text, or private implementation behavior.
- Do not change harness auth, provider/model ownership, daemon APIs, or store schema unless a narrow need appears.
- Do not remove existing commands like `/sessions`, `/attach`, `/doctor`, `/daemon`, `/patch`, `/quest`, or session ritual verbs.
- Do not build a graphical or web UI.
- Do not require live Codex, Claude, or provider credentials in tests.

## Architecture

### 1. One Shared Chat Shell

Introduce one shared chat-first shell used by `Command::Chat`, `Command::Tui`, and bare `coven` in an interactive terminal. Reuse the existing `tui::chat` ratatui lifecycle. Do not create a second full-screen stack. The existing launcher stops being the first screen; keep its plumbing for non-interactive Cast output and compatibility paths only.

### 2. Cast-Aware Composer

All composer submissions pass through Cast before side effects:

1. User types into the composer.
2. Chat app builds a Cast plan from the raw spell.
3. Safe plans proceed directly.
4. Risky/ambiguous plans render inline confirmation cards.
5. Accept/reject resolves the pending confirmation.
6. Launch, attach, quest, patch, and session actions reuse existing Cast and daemon paths.
7. Outcomes append to the transcript.

The composer supports both natural language and slash commands:
`fix the failing tests` / `/sessions` / `/claude review the diff` / `/quest continue`

### 3. Command Palette Overlay

The current menu becomes an overlay, not the default first screen.

- `/help` — readable command help in the transcript or overlay
- `Ctrl+K` — compact command palette overlay
- `/commands` — alias for the same overlay

### 4. Visual Layout

```
Coven codex · /path/to/project · daemon: running

✦ coven
  Ready. Type a task or /help.

> fix the failing tests

✦ Cast plan
  harness Codex · Cast default
  risk [ SAFE ]
  steps launch project-scoped session

✦ Codex
  ...streamed output...

────────────────────────────────────────────────────────
> Try "review this branch" or /help
```

Visual rules:
- transcript is the primary surface
- composer is always at the bottom
- status is compact and persistent
- no decorative launch menu on first paint
- hints are short and contextual
- layout must work without heavy color dependence

### 5. Session Model

Shell tracks one active session by default. When an active session exits, the next plain prompt launches a new session cleanly.

Required interactions: start from natural language, stream output to transcript, forward follow-up input, clear on exit/kill, `/attach <id>` replays/follows, `/sessions` opens overlay, `/kill [id]` stops active/named session.

### 6. Non-Interactive Behavior

If stdin or stdout is not a TTY, `coven` and `coven tui` keep printing the existing plain Cast frame. These paths must remain readable, deterministic, and CI-friendly.

## Components

**`tui::chat::app`** — transcript messages, composer content/cursor, active harness/agent, active session id/event cursor, overlays, pending Cast confirmation. New Cast state should stay narrow.

**`tui::chat::render`** — compact status row, transcript, bottom composer, overlays, small-terminal fallback. Add plain snapshot helpers for tests.

**`tui::chat::events`** — text entry, Enter submit, Shift+Enter newline, Ctrl+C/D exit, Ctrl+K palette, Esc clears overlay/confirmation/composer, Up/Down history, overlay navigation.

**`tui::chat::client`** — daemon-backed transport boundary only. No socket code in render or events.

**`tui::cast`** — spell parsing, plan rendering, safety decisions, quests, outcomes, attach summaries. May add adapter helpers so chat can render Cast content as transcript messages.

## Data Flow

**Safe prompt:** compose → Cast plan → plan transcript message → daemon launch → output events → completion message → clear session.

**Confirmation prompt:** compose → Cast returns Confirm → inline plan card → pending-confirmation state → yes/no/Esc → proceed or cancelled outcome.

**Slash command:** compose → Cast intent or chat-local command → overlay or transcript result → no daemon launch unless required.

**Daemon events:** poll/stream → normalize into transcript entries → update status row → completion/error/kill clears running state. Terminal output is transcript content, never raw nested TUI output.

## Error Handling

- Daemon unavailable: inline `coven daemon start` / restart guidance
- Harness unavailable: `coven doctor` guidance, keep composer usable
- API mismatch: show mismatch, stop polling until next user action
- Event polling failure: keep existing backoff and coalescing behavior
- Small terminal: minimal "Terminal too small" frame
- Unknown command: concise inline error, suggest `/help`

## Testing Contracts

1. `coven`, `coven tui`, and `coven chat` route to same interactive shell entry point when attached to a TTY.
2. Non-interactive `coven tui` still prints the plain Cast frame.
3. Default chat frame has compact status row, welcome transcript, and bottom composer.
4. Default chat frame does not contain the old launcher-first command list.
5. `/help`, `/commands`, or `Ctrl+K` exposes the command surface that was previously first-screen.
6. Plain input appends a Cast plan transcript entry before launching.
7. Safe prompts launch without confirmation.
8. Confirmation prompts pause for explicit approval.
9. Session exit and kill clear active session.
10. Existing daemon-backed attach/replay/follow/input-forwarding/kill/export behavior still works.
11. Raw nested Codex TUI output does not appear inside Coven chat.

Minimum automated gates:

```bash
cargo test -p coven-cli
cargo clippy -p coven-cli --no-deps
cargo fmt --check
git diff --check
```

Manual verification:

```bash
cargo build -p coven-cli
./target/debug/coven          # confirm chat-first first frame
./target/debug/coven tui | head  # confirm plain Cast output
```

## Migration Plan

1. Add chat frame snapshot helpers and tests for the target first screen.
2. Make `Command::Tui` and default `None` call the shared chat shell for interactive terminals.
3. Adapt chat input to route through Cast planning before daemon launch.
4. Move command-palette behavior into `/help`, `/commands`, and `Ctrl+K` overlays.
5. Preserve and re-test non-interactive Cast plain output.
6. Run full verification and open a PR separate from prior chat-state fixes.

## Acceptance Criteria

Complete when evidence proves:

- `coven`, `coven tui`, and `coven chat` open the same chat-first interactive TUI.
- First interactive frame is transcript/composer/status oriented, not launcher/menu oriented.
- Plain language and slash commands work from one composer.
- Cast plan/safety/outcome behavior is preserved inside the transcript flow.
- Existing daemon-backed session launch, attach, event follow, input forwarding, kill, and export behavior still works.
- Non-interactive output remains script-friendly.
- Focused snapshot/state tests cover the new visual and interaction contract.
- Full repo gates pass.
