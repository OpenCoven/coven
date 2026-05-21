# Long-Running Goal: Coven Chat-First TUI

**Repo:** OpenCoven/coven | **Branch:** feat/chat-first-tui | **Full design:** docs/design/chat-first-tui.md

## Goal

Redesign the default Coven terminal experience so that `coven`, `coven tui`, and `coven chat` all open the same chat-first full-screen TUI — similar in feel to Claude Code or Claurst — while preserving Cast planning, safety gates, and daemon-backed sessions.

## What to build

**One shared chat shell.** Make `Command::Tui`, `Command::Chat`, and bare `coven` (interactive TTY) all call a single shared shell backed by the existing `tui::chat` ratatui lifecycle. The old launcher (`tui::shell`) stops being the default first screen; keep it as a non-interactive Cast output path only.

**Cast-aware composer.** Route all composer input through Cast planning before side effects. Safe plans proceed immediately. Risky plans show an inline confirmation card. Accept/reject/Esc resolves pending confirmations. Outcomes append to the transcript. Support both natural language and slash commands from the same composer.

**Command palette overlay.** Move the launcher-style command menu behind `Ctrl+K` and `/commands`. `/help` should also surface command discovery. Remove them from the first-paint frame.

**Default visual layout:**

```
Coven codex · /path/to/project · daemon: running

✦ coven
  Ready. Type a task or /help.

> fix the failing tests

✦ Cast plan
  harness Codex · Cast default  risk [ SAFE ]
  steps  launch project-scoped session
✦ Codex
  ...streamed output...

───────────────────────────────────
> Try "review this branch" or /help
```

Rules: transcript is primary surface, composer always at the bottom, compact persistent status row, no launch menu on first paint.

**Session state.** Track one active session by default. Forward follow-up input to a running session. Clear completed sessions cleanly before the next message. Keep `/attach <id>`, `/sessions`, and `/kill [id]` working.

**Non-interactive output.** When stdout is not a TTY, keep the existing plain Cast frame. `coven "fix tests"` and `coven tui | head` must remain script-friendly.

**Error handling.** Daemon unavailable → inline `coven daemon start` guidance. Harness unavailable → `coven doctor` guidance. API mismatch → stop polling. Small terminal → minimal fallback. Unknown command → inline error + suggest `/help`.

## Do NOT

- Do not clone Claude Code branding, exact wording, or private behavior.
- Do not change harness auth, daemon APIs, or store schema unless a narrow need is proven.
- Do not remove existing commands (`/sessions`, `/attach`, `/doctor`, `/daemon`, `/patch`, `/quest`, ritual verbs).
- Do not require live Codex, Claude, or provider credentials in tests.

## Migration stages

1. Add chat frame snapshot helpers and tests for the target first screen.
2. Route `Command::Tui` and default `None` to the shared chat shell for interactive terminals.
3. Route cast input through Cast planning before daemon launch.
4. Move command-palette into `/help`, `/commands`, `Ctrl+K` overlays.
5. Preserve and re-test non-interactive Cast plain output.
6. Run full verification, open a PR.

## Automated gates (must pass before PR)

```bash
cargo test -p coven-cli
cargo clippy -p coven-cli --no-deps
cargo fmt --check
git diff --check
cargo build -p coven-cli
```

## Acceptance criteria

- `coven`, `coven tui`, `coven chat` all open the same chat-first TUI.
- First frame is transcript/composer/status — not a launcher menu.
- Plain language and slash commands work from one composer.
- Cast plan/safety/outcome behavior preserved inside transcript flow.
- Existing daemon session launch, attach, follow, kill, and export still work.
- Non-interactive Cast output still works.
- Focused snapshot/state tests cover new visual and interaction contracts.
- Full repo gates pass.
- Raw/duplicated Codex output does not appear in chat.
