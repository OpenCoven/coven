---
summary: "Phase 6 inspection notes: captured launcher and Cast non-interactive frames at typical widths, with what was tested and what remains a manual check before merge."
title: "Cast — Phase 6 Verification Notes"
description: "Verification artifact for the Cast redesign branch (cast/phase-6-readiness). Captures rendered frames in NoColor mode, lists what passed automatically, and flags what still needs a human terminal pass before merge."
---

# Cast — Phase 6 Verification Notes

This document is the verification artifact for the Cast redesign branch. It exists because TUI work cannot be fully verified by `cargo test` alone — colour, cursor behaviour, raw-mode key handling, and `SIGWINCH` reflow only show up in a real terminal. Tests pin the byte-level output of the renderer in `NoColor` mode; this file pins what a reviewer should see when they actually open it.

The frames below were captured from `render_magical_tui_frame_plain_with_width` (the launcher renderer in `crates/coven-cli/src/tui/shell.rs`) and the binary's non-interactive Cast frame. They are NoColor — no ANSI escapes — so the structure is the only signal. In a real terminal the identity row, selected slash row, and field labels carry brand colour per [`cast-tui-contract.md`](./cast-tui-contract.md).

## Gate results

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | ✓ clean (after running `cargo fmt` once at start of phase) |
| `cargo clippy -p coven-cli --tests --no-deps -- -D warnings` | ✓ clean |
| `cargo test -p coven-cli` (313 unit + 4 smoke) | ✓ 0 failures |

Notes:

- The launcher tests in `crates/coven-cli/src/main.rs::tests::magical_tui_frame_*` (10 cases) failed at the start of Phase 6 because the Phase 1 design contract was never implemented in `tui/shell.rs::render_magical_tui_frame_with_mode_and_width` — only the assertions had landed. Phase 6 rewrote that renderer against the contract; all 10 now pass.
- Two pre-existing `dead_code` warnings remain on `theme::BORDER_SUBTLE` / `theme::BORDER_STRONG` (added by the Phase 1 contract for future single-rule borders that are not yet plumbed through callsites). They do not fail clippy (which is run against `--tests`) because the constants are in the non-test target; flagged here so a follow-up phase wires them in.

## Launcher frame — selection=0, empty input, width=76

```text
Cast

────────────────────────────────────────────────────────────────────────────
> type a task or /run codex
────────────────────────────────────────────────────────────────────────────

Commands                               Snapshot
› /start     Start here                project         (unset)
  /help      Help                      harness         (unset)
  /tui       Open TUI                  daemon          unknown
  /doctor    Doctor
  /daemon    Daemon status
  /run       Run an agent
1 of 14

spell           /start
detail          Setup check and a safe first command

enter run · ↑↓ select · esc quit · ctrl+u clear
```

What to verify in a real terminal:

- `Cast` renders in `PRIMARY_STRONG` (the brand purple, `#9A8ECD`).
- The two `─` rules above and below the prompt are subtle (`FIELD_LABEL`), not bright.
- `› /start` is the only row in `PRIMARY_STRONG`; the other five command rows are in `TEXT`.
- The Snapshot values (`(unset)`, `unknown` here) are replaced with the resolved project name, default harness id, and daemon status (`running` / `stopped` / `stale`).
- `enter run · ↑↓ select · esc quit · ctrl+u clear` is rendered in `DIM`.

## Launcher frame — selection=0, typed input, width=76

```text
Cast

────────────────────────────────────────────────────────────────────────────
> polish the README
────────────────────────────────────────────────────────────────────────────

Commands                               Snapshot
› /start     Start here                project         (unset)
  /help      Help                      harness         (unset)
  /tui       Open TUI                  daemon          unknown
  /doctor    Doctor
  /daemon    Daemon status
  /run       Run an agent
1 of 14

spell           /start
detail          Setup check and a safe first command

enter run · ↑↓ select · esc quit · ctrl+u clear
```

What to verify in a real terminal:

- Empty-prompt placeholder (`> type a task or /run codex`) renders in `DIM`; typed input (`> polish the README`) renders in `TEXT`. The shift from dim to bright as the user types is the focus signal.
- The terminal cursor is at the end of the input line. There is no synthetic `█` block.

## Launcher frame — selection=12 (`/sacrifice`), width=76

```text
Cast

────────────────────────────────────────────────────────────────────────────
> type a task or /run codex
────────────────────────────────────────────────────────────────────────────

Commands                               Snapshot
  /sessions  Active sessions           project         (unset)
  /all       All sessions              harness         (unset)
  /attach    Attach session            daemon          unknown
  /summon    Summon session
  /archive   Archive session
› /sacrifice Sacrifice session
13 of 14

spell           /sacrifice
detail          Permanently delete a non-running session

enter run · ↑↓ select · esc quit · ctrl+u clear
```

What to verify in a real terminal:

- The command rail is windowed: items 7–12 are visible (six rows) with `/sacrifice` highlighted at the bottom.
- The scroll hint `13 of 14` renders in `DIM` below the rail.
- The action preview still resolves correctly to the new selection (`/sacrifice` + its description).
- When the user picks `/sacrifice` and presses Enter with an empty prompt, the Cast safety gate prompts for the typed `sacrifice` confirmation — that flow is covered by `cast_plan_for_sacrifice_describes_typed_confirm_in_copy` and the smoke `sacrifice` test, not by re-rendering here.

## Cast non-interactive frame (piped stdout)

This frame is printed when `coven` is run without a TTY (e.g., piped, in CI, in a non-interactive script). Captured by running `./target/debug/coven </dev/null`:

```text
Cast — your Coven familiar
Cast, your Coven familiar, is ready. Type a spell, or use a slash command.

Context
Project        /path/to/opencoven/coven
Default harness codex

Example spells
  fix the failing tests
  explain this repo in 5 bullets
  run claude polish the README
  use codex draft a release note
  review this branch
  open the last Claude session
  sessions
  doctor

Slash spells
  /run codex fix the failing tests
  /claude review the latest diff
  /sessions     /all     /attach <id>     /summon <id>
  /archive <id>     /sacrifice <id>
  /doctor     /daemon     /patch     /help     /quit

Tip: in a terminal, `coven` opens the Cast launcher. Empty input opens the slash palette.

Tip: run `coven` in a real terminal to open the Cast launcher and type a spell.
```

This surface predates the Phase 1 launcher contract and is intentionally a Phase 1+ copy variant — see `tests::cast_non_interactive_frame_introduces_cast_and_shows_examples`. The contract drift items (em-dash in the headline, capitalised `Project`, second-person address) are deliberately out of scope for Phase 6 because they are pinned by Phase 1 tests; tightening that surface to fully match §2.6 is a follow-up.

## What this branch does NOT cover

These were considered for Phase 6 but deliberately deferred:

- **Live raw-mode keybinding sweep.** The launcher's raw-mode loop (Arrow keys, Backspace, Ctrl+U, Ctrl+C, Esc, Enter) is exercised manually only; there is no end-to-end test that drives keys into the binary. A future phase could add a PTY-driven integration test.
- **`SIGWINCH` reflow.** The renderer reads terminal columns on every redraw and produces a fitted frame, but no automated test resizes the terminal mid-run. A reviewer should resize a real terminal while the launcher is open and confirm the rule lengths and two-lane widths track the new width.
- **TrueColor vs Indexed256 visual parity.** `theme::ratatui_color_with_mode` is unit-tested, but the launcher's colour rendering in a 256-colour-only terminal (e.g. older SSH sessions) needs a human eye to confirm the `PRIMARY_STRONG` purple is still legible.
- **`docs/design/cast-tui-contract.md` §3.11 — saturated purple swap.** The contract calls for `PRIMARY_STRONG` (the more saturated purple, `#9A8ECD`) to carry weight and `PRIMARY` (lighter, `#C5BDED`) to be a hover/secondary accent. The new launcher renderer uses `PRIMARY_STRONG` for identity, selected row, and headers; `PRIMARY` is unused at the launcher level. ✓
- **`BORDER_SUBTLE` / `BORDER_STRONG` use.** The contract adds these tokens (Phase 1, §4) but Phase 2-5 never plumbed them into a renderer. The launcher uses `FIELD_LABEL` for the prompt rules instead. Phase 7 should either wire them in or remove the unused constants.

## Risks still open before merge

- `theme::BORDER_SUBTLE` and `theme::BORDER_STRONG` are unused dead code (4 warnings during `cargo build`). They do not fail any gate but they signal an incomplete Phase 1 → 2 handoff.
- The Cast non-interactive frame (printed when stdin/stdout is not a TTY) still uses pre-contract copy ("Cast — your Coven familiar", capitalised labels, second-person greeting). It is pinned by tests and not in scope for Phase 6.
- `docs/ROADMAP.md` is last-updated 2026-05-09 and does not mention the Cast redesign. If the redesign is meant to ship in the next public update, the roadmap will need a fresh entry under Coven > Now.
- `docs/PRODUCT-SPEC.md` "visible work" thesis (line 14) is intact; no change needed.
- `npm/coven/README.md` install copy is unchanged and accurate; no change needed.
