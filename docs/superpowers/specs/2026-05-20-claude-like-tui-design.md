---
title: "Claude Code-like Coven TUI design"
description: "Recreate the default Coven TUI as a chat-first terminal experience while preserving Cast planning, safety gates, and daemon-backed sessions."
---

# Claude Code-like Coven TUI — Design

**Status:** Approved direction — ready for implementation planning after review  
**Date:** 2026-05-20  
**Scope:** Replace the default `coven` / `coven tui` launcher-first experience with a chat-first terminal UI, and bring `coven chat` into the same interaction model.

## Problem

The current default Coven terminal flow opens a raw-terminal Cast launcher: a command palette with example spells, slash spells, and direct action rows. That makes sense as an onboarding menu, but it does not feel like Claude Code. Claude Code's terminal experience is conversation-first: the user lands in a working chat loop, sees project/runtime context, types natural language immediately, and discovers commands through slash help rather than starting from a menu.

Coven already has most of the required backend behavior:

- `tui::shell` owns the default `coven` / `coven tui` raw-terminal launcher and Cast dispatch.
- `tui::chat` owns a ratatui transcript/composer surface with daemon session launch, attach, input forwarding, event polling, history, agent selection, help, and session overlay.
- `tui::cast` owns spell parsing, safety decisions, plan cards, outcomes, quest flow, and daemon-backed follow behavior.

The redesign should change the product shape without discarding these proven pieces.

## Goals

1. Make `coven` and `coven tui` open a chat-first full-screen TUI.
2. Make `coven chat` use the same experience so users do not learn two terminal products.
3. Preserve Cast as the planner, safety gate, and dispatcher for typed spells.
4. Preserve daemon-backed session launch, event polling, attach/replay, kill, and export behavior.
5. Make slash commands discoverable through `/help` and `Ctrl+K`, not through a launch menu as the first screen.
6. Keep non-interactive output friendly for pipes and CI by retaining the existing plain Cast frame.
7. Keep the implementation testable through text snapshots and focused state tests.

## Non-goals

- Do not clone Claude Code branding, exact copy, or private behavior. The target is familiar interaction grammar: chat-first, compact status, natural prompt, slash-command discovery.
- Do not change harness auth, model/provider ownership, daemon APIs, or store schema unless implementation proves a narrow need.
- Do not remove existing commands such as `/sessions`, `/attach`, `/doctor`, `/daemon`, `/patch`, `/quest`, or session ritual verbs.
- Do not implement a graphical or web UI.
- Do not make the first implementation depend on live Codex or Claude credentials for tests.

## Recommended Architecture

### 1. A single chat-first shell

Introduce a shared chat shell entry point used by both:

- `Command::Chat`
- `Command::Tui`
- `None` (plain `coven` in an interactive terminal)

The shell should reuse the ratatui lifecycle from `tui::chat` rather than adding a second full-screen stack. The existing raw-terminal launcher in `tui::shell` becomes legacy plumbing for non-interactive Cast output and for command dispatch helpers that still make sense.

### 2. Cast-aware chat input

Plain input in the new shell should route through Cast's parser/planner path before side effects:

1. User enters text in the composer.
2. The app builds a Cast plan from the raw spell.
3. Safe plans proceed directly.
4. Confirm/reject plans surface inline in the transcript with an explicit response path.
5. Launch/attach/session actions reuse existing Cast and daemon machinery.
6. Outcome summaries append to the transcript.

The user should be able to type either natural language (`fix the failing tests`) or slash commands (`/sessions`, `/claude review the diff`) in the same composer.

### 3. Command palette as overlay

The current menu should not be the first screen. It should become:

- `/help`: readable command help in the transcript or overlay.
- `Ctrl+K`: compact command palette overlay.
- `/commands`: alias for the same overlay.

The palette lists the existing command surface, but it is secondary to the composer.

### 4. Visual layout

The default frame should be quiet and dense:

```text
Coven                  codex · /path/to/project                 daemon: running

✦ coven
  Ready. Type a task or /help.

> fix the failing tests

✦ Cast plan
  harness      Codex · Cast default
  risk         [ SAFE ]
  steps        launch project-scoped session

✦ Codex
  ...streamed output...

────────────────────────────────────────────────────────────────────────────
> Try "review this branch" or /help
```

Important visual rules:

- The transcript is the primary surface.
- The composer is always at the bottom.
- Status is compact and persistent.
- There is no decorative launch menu on first paint.
- Hints are short and contextual.
- Existing brand color tokens remain available, but the layout should not depend on heavy color to be understandable.

### 5. Session model

The shell should track one active session at a time by default, matching the current chat TUI. When an active session exits, the next plain prompt launches a new session. Attach/replay uses the session overlay and existing daemon APIs.

Required session interactions:

- Start a session from natural language.
- Stream output into transcript.
- Forward follow-up input to a running session.
- Clear active session on exit or kill.
- `/attach <id>` replays or follows.
- `/sessions` opens session overlay.
- `/kill [id]` stops active or named live session.

### 6. Non-interactive behavior

If stdin or stdout is not a TTY, `coven` and `coven tui` should keep printing the existing plain Cast frame. This preserves scripts, docs snapshots, and current CI-friendly behavior.

## Components

### `tui::chat::app`

Owns durable app state:

- transcript messages
- composer content and cursor
- active harness/agent
- active session id and event cursor
- command palette/help/session overlays
- pending Cast confirmation, if any

New state should be narrowly added for Cast plans and confirmation prompts rather than widening daemon or store types.

### `tui::chat::render`

Owns the full-screen frame:

- compact status row
- transcript
- bottom composer
- help/command palette overlay
- session overlay
- small-terminal fallback

Add plain snapshot helpers so tests can assert the new frame without a real terminal.

### `tui::chat::events`

Owns key handling:

- text entry
- Enter submit
- Shift+Enter newline
- Ctrl+C / Ctrl+D exit
- Ctrl+K palette
- Esc clears overlay or composer
- Up/Down history when composer is focused
- overlay navigation where relevant

### `tui::chat::client`

Remains the daemon-backed transport boundary. The redesign should not move direct socket code into render or event code.

### `tui::cast`

Remains the semantic layer for spell parsing, plan rendering, safety, quests, outcomes, and attach summaries. Implementation may add adapter helpers so chat can render Cast plan/outcome content as transcript messages instead of printing to stdout.

## Data Flow

### Safe natural prompt

1. Composer receives `fix the failing tests`.
2. Chat app calls Cast planning with the raw spell.
3. Plan is appended as a system/Cast transcript message.
4. Daemon session launches with the resolved harness and prompt.
5. Output events append as assistant/agent transcript messages.
6. Exit event appends a completion message and clears active session.

### Confirmation prompt

1. Composer receives a risky spell such as `publish the crate`.
2. Cast returns `Confirm`.
3. Transcript shows the plan and confirmation reason.
4. App enters pending-confirmation state.
5. `yes` proceeds; empty input, `no`, or Esc cancels and appends a cancelled outcome.

### Slash command

1. Composer receives `/sessions`.
2. Cast intent or chat-local command maps to the session overlay.
3. Overlay uses existing session list APIs.
4. Selecting or typing `/attach <id>` enters attach/replay/follow flow.

## Error Handling

- Daemon unavailable: show an inline message with `coven daemon start` / `restart` guidance.
- Harness unavailable: show `coven doctor` guidance and keep the composer usable.
- API mismatch: show the daemon API mismatch and stop polling until the next user action.
- Event polling failure: keep existing backoff and coalescing behavior.
- Small terminal: show a minimal "Terminal too small" frame.
- Unknown command: show a concise inline error and suggest `/help`.

## Testing

Implementation should use test-first changes around these contracts:

1. `coven` / `coven tui` / `coven chat` route to the same interactive shell entry point when attached to a TTY.
2. Non-interactive `coven tui` still prints the Cast plain frame.
3. The default chat frame contains a compact status row, welcome transcript, and bottom composer, and does not contain the old launcher-first command list.
4. `/help` or `Ctrl+K` exposes the command surface that used to be first-screen.
5. Plain input appends a Cast plan transcript entry before launching.
6. Safe prompts launch without confirmation; confirm prompts pause for explicit approval.
7. Session exit and kill clear active session.
8. Existing `cargo test -p coven-cli`, `cargo clippy -p coven-cli --no-deps`, and secret guard pass.

Manual verification should include:

- Launch `target/debug/coven` in a real TTY and confirm the first frame is chat-first.
- Type a harmless prompt with a fake or available harness and confirm transcript flow.
- Run `/help`, `/sessions`, and `/quit`.
- Pipe `target/debug/coven tui | head` and confirm plain Cast output still works.

## Migration Plan

The implementation plan should be split into small reviewable stages:

1. Add chat frame snapshot helpers and tests for the target first screen.
2. Make `Command::Tui` and default `None` call the shared chat shell for interactive terminals.
3. Adapt chat input to route through Cast planning before daemon launch.
4. Move command-palette behavior into `/help` and `Ctrl+K` overlays.
5. Preserve and re-test non-interactive Cast plain output.
6. Run full verification and open a PR separate from prior chat-state fixes.

## Acceptance Criteria

The goal is complete when current evidence proves:

- `coven`, `coven tui`, and `coven chat` open the same chat-first interactive TUI.
- The first interactive frame is transcript/composer/status oriented, not launcher/menu oriented.
- Plain language and slash commands work from one composer.
- Cast plan/safety/outcome behavior is preserved inside the transcript flow.
- Existing daemon-backed session launch, attach, event follow, input forwarding, kill, and export behavior still work.
- Non-interactive output remains script-friendly.
- Focused snapshot/state tests cover the new visual and interaction contract.
- Full repo gates pass.
