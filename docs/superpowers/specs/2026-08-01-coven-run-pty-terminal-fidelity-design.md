# Coven Run PTY Terminal Fidelity Design

## Summary

Interactive `coven run` sessions should render the selected harness as though it
were launched directly in the caller's terminal. Coven will initialize the child
PTY from the connected terminal's real geometry and keep that geometry synchronized
for the lifetime of the attached session. Coven will not add competing invocation
chrome or replace the harness's native interface.

This design addresses [issue #540](https://github.com/OpenCoven/coven/issues/540).

## Problem

`crates/coven-cli/src/pty_runner.rs::terminal_size` currently reads only the
`LINES` and `COLUMNS` environment variables, then falls back to 24 rows by 80
columns. Those variables are commonly absent, are not automatically updated when a
terminal pane changes size, and do not include pixel geometry. The attached runner
passes that value to `openpty` once and never calls `MasterPty::resize`.

The reported invocation demonstrates the user-visible consequences:

- Codex renders an approximately 80-column frame inside a pane that is nearly twice
  as wide.
- Output wraps early and leaves a large unused area to the right.
- The composer redraws against incorrect row and column boundaries, leaving a stale
  duplicate composer in scrollback.
- Resizing the host pane cannot repair the child because the child PTY never receives
  the new geometry.

The native Codex interface is not the source of the defect. The incorrect geometry
originates at the Coven PTY boundary.

## Goals

- Give an interactive child PTY the caller's actual columns, rows, and available
  pixel dimensions before the harness starts.
- Forward later terminal size changes to the child promptly and without redundant
  resize operations.
- Preserve the native harness interface, raw byte transport, input behavior, color,
  cursor control, alternate-screen behavior, and keyboard shortcuts.
- Shut down resize work deterministically when the child exits or the attached run
  fails.
- Keep safe behavior when no terminal geometry is available.
- Add regression coverage for source selection, invalid geometry, resize propagation,
  deduplication, and shutdown.

## Non-goals

- Replacing interactive Codex with `codex exec` or another one-shot interface.
- Building a Coven-owned transcript, composer, header, footer, or status bar around
  the harness.
- Reformatting or filtering harness output.
- Changing detached sessions, daemon-observed sessions, captured stream-JSON output,
  or harness command construction.
- Fixing unrelated test timing behavior discovered during baseline verification.

## Considered approaches

### 1. Transparent PTY fidelity

Keep the native harness TUI and repair the PTY boundary. This is the selected
approach because it fixes the owning defect, preserves every supported harness's
native behavior, and remains confined to the shared PTY runner.

### 2. Default prompted runs to a headless harness mode

Launching `codex exec` would avoid interactive repaint behavior and produce a clean
one-shot transcript. It would also remove the follow-up composer and change the
meaning of an interactive `coven run`, so it does not satisfy the invocation shown
in the report.

### 3. Wrap harnesses in Coven-owned invocation chrome

A Coven frame could display session metadata and control resize itself. It would
compete with alternate-screen applications, duplicate native status information,
and substantially broaden the authority and rendering surface. It is unnecessary
for terminal fidelity.

## Design

### Terminal geometry resolution

The PTY runner will resolve startup geometry in this order:

1. When the attached path has a real terminal, read
   `crossterm::terminal::window_size()`. Accept the result only when both rows and
   columns are nonzero. Preserve its cell dimensions and pixel width and height.
2. If pixel-aware window inspection is unavailable or invalid, read
   `crossterm::terminal::size()`. Accept positive cell dimensions and set pixel
   width and height to zero.
3. If terminal inspection is unavailable or invalid, read positive `LINES` and
   `COLUMNS` values from the environment. A missing or invalid dimension uses its
   conventional default rather than invalidating the other dimension.
4. Use 24 rows by 80 columns with zero pixel dimensions as the final fallback.

The resolver will be a small pure helper around explicit terminal and environment
inputs so precedence and invalid-value behavior can be tested without depending on
the test runner's terminal.

### Live resize relay

Only a genuinely interactive attached session needs live resize forwarding. After
the PTY reader and writer are taken, the PTY master will move into a scoped resize
watcher. The watcher will sample the connected terminal geometry at a short bounded
interval and call `MasterPty::resize` only when the complete `PtySize` differs from
the last applied size.

Polling is deliberate here:

- it uses the same cross-platform crossterm geometry source on Unix and Windows;
- it does not compete with the raw stdin forwarding thread for terminal events;
- it avoids process-global signal handlers and their cross-test interactions; and
- a 100 ms interval is responsive for pane dragging while remaining negligible next
  to an interactive harness process.

Transient host-geometry read failures retain the last valid size and allow the next
poll to retry. A child-PTY resize failure ends the watcher quietly because it normally
means the PTY is closing; writing diagnostics into stdout would corrupt the harness
display.

### Lifetime and cleanup

The resize watcher will own the PTY master and an explicit stop signal. The attached
runner will stop and join the watcher after the child exits and before it waits for
the output thread to finish. Joining drops the final master handle, allowing the PTY
reader to receive EOF without leaking a background thread or delaying process exit.

The watcher will also be RAII-safe: early returns after it starts must signal and
join it during drop. Existing `RawModeGuard` cleanup remains responsible for restoring
the caller's terminal mode.

Non-interactive attached runs will retain the master on the current thread and will
not spawn a watcher. Their initial geometry still uses the same safe resolver.

### Data flow

```text
host terminal geometry
        |
        v
startup geometry resolver ---> openpty(real size) ---> harness starts at full width
        |
        v
scoped resize watcher -------> MasterPty::resize -------> child receives resize
        |
child exit / runner error
        |
        v
stop + join watcher ---> drop master ---> output EOF ---> restore raw mode
```

The existing stdin-to-PTY and PTY-to-stdout byte streams remain unchanged.

## Error handling

- Failure to inspect the host terminal is not fatal; environment and conventional
  defaults preserve current headless behavior.
- Zero rows or columns are invalid and never reach `openpty` or `resize`.
- A transient geometry read failure during an attached run keeps the last valid
  child size.
- A resize failure stops only the resize watcher. Child status and output remain
  authoritative for the run result.
- A watcher panic is contained when joined and must not prevent raw-mode restoration
  or child-result persistence.

## Test strategy

Focused tests in `crates/coven-cli/src/pty_runner.rs` will prove:

- real terminal geometry wins over conflicting environment values;
- pixel width and height survive conversion to `PtySize`;
- invalid or unavailable terminal geometry falls back to positive environment values;
- missing, zero, and malformed environment dimensions fall back safely;
- unchanged samples do not call `MasterPty::resize`;
- changed rows, columns, or pixel dimensions produce exactly one resize call;
- transient source failures do not discard the last applied size;
- resize failure and explicit shutdown terminate the watcher;
- the watcher drops the master so the output path can finish.

A Unix PTY regression will launch a small shell fixture that reports `stty size`,
change the controlling terminal geometry, and assert that the child observes the new
rows and columns. Platform-independent unit tests will cover the watcher contract;
Windows CI will compile the same production path.

Manual verification will run the built `coven` from a wide terminal, launch Codex
with a bounded prompt, and inspect that:

- the native frame uses the full pane width instead of approximately 80 columns;
- long output wraps at the visible pane boundary;
- resizing narrower and wider causes one coherent redraw; and
- the composer remains singular and aligned after the response and resize.

## Verification gates

Before committing implementation changes:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

The clean baseline build passed on 2026-08-01. The baseline workspace suite had one
unrelated load-sensitive failure:
`codex_json_sigterm_reaps_descendants_and_marks_ledger_failed` uses a fixed
three-second fixture-start deadline. The compiled test passed in 0.45 seconds when
run alone, while overlapping Cargo jobs starved identical runs past the deadline and
they failed after 7 to 10 seconds. No #540 files had been modified, and temporary
diagnostic instrumentation was reverted. Final verification must report this baseline
separately if it recurs and must still run the focused PTY tests independently.

## Acceptance criteria

- The screenshot's wide pane produces a correspondingly wide native Codex layout.
- Attached harnesses receive startup and live terminal geometry.
- Resizing does not create duplicate composers, stale frames, or avoidable redraws.
- Native harness interaction remains visually and behaviorally transparent.
- Every new automated regression passes on the supported CI platforms.
- Repository formatting, lint, test, secret, and privacy gates are run with fresh
  evidence before the implementation is considered complete.
