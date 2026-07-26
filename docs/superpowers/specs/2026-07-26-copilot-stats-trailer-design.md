# Copilot stats trailer filtering — design

- **Date:** 2026-07-26
- **Issue:** [#493](https://github.com/OpenCoven/coven/issues/493)
- **Status:** approved for implementation

## Problem

`coven chat` cleans PTY output and removes harness-owned transcript metadata
before displaying assistant prose. Copilot's one-shot prompt mode ends with a
four-line statistics trailer:

```text
Changes    +1 -1
Requests   1 Premium (8s)
Tokens     ↑ 28.0k (20.4k cached) • ↓ 32
Resume     copilot --resume=<session-id>
```

The current filter recognizes each of those line shapes independently and
sets the shared `AgentOutputMode` to `Hidden`. That mode persists across PTY
chunks and is reset only by a later assistant marker or turn teardown. Copilot
does not emit Codex's `assistant`/`codex` markers, so one stats-shaped line in
ordinary prose can hide every later line in the turn. A realistic example is
an assistant explaining a resume command:

```text
Resume     copilot --resume=<session-id>
```

This is not only a formatting defect. It is silent assistant-output loss.

## Evidence and root cause

The data path is:

1. daemon `output` events deliver arbitrary PTY chunks;
2. `dispatch_pty_output` retains incomplete raw lines and cleans terminal
   control sequences;
3. complete lines enter `human_facing_plain_output`;
4. `is_copilot_stats_line` classifies a single line;
5. the classifier mutates the role-visibility mode to `Hidden`;
6. later Copilot prose is discarded because no marker can restore visibility.

Recent fixes already establish the adjacent invariants:

- classification happens only on complete PTY lines;
- Codex role markers are harness-gated;
- raw line state survives chunk boundaries for CR, backspace, ANSI, and
  whitespace fidelity;
- live and batched modes share the same visible-text sink.

The remaining architectural mistake is treating a Copilot terminal trailer as
a role transition. Trailer recognition needs its own bounded, fail-open state.

## Goals

1. Never let one marker-shaped assistant line hide later prose.
2. Preserve the prose-only chat transcript by removing a genuine Copilot
   statistics trailer.
3. Apply Copilot trailer rules only to Copilot PTY sessions.
4. Preserve exact visible text when a trailer candidate is incomplete,
   out-of-order, followed by prose, or interrupted.
5. Keep behavior identical in live and batched streaming modes.
6. Bound all new per-session state and clean it up with the existing PTY
   lifecycle.
7. Preserve Codex transcript filtering and terminal-control handling.

## Non-goals

- Do not change daemon event formats, session persistence, or harness launch.
- Do not change stream-JSON rendering; the defect is in merged PTY output.
- Do not expose Copilot statistics as a new transcript message or status UI.
- Do not broaden or loosen Codex role-marker recognition.
- Do not refactor the rest of the large chat app while fixing this seam.
- Do not audit unrelated human-facing CLI commands in this PR.

## Design

### D1 — Separate role filtering from trailer filtering

`AgentOutputMode` remains the Codex transcript role state. Copilot statistics
must never mutate it.

- Codex sessions continue through `human_facing_agent_output`, without
  Copilot stats classification.
- Copilot PTY sessions pass complete cleaned lines through a dedicated trailer
  recognizer.
- Claude fallback, Grok, and other non-Codex PTY harnesses treat
  Copilot-shaped lines as ordinary prose.
- An unknown harness does not apply the Copilot recognizer. Fail-open output is
  safer than deleting content based on an unproven wire format.

### D2 — Recognize one exact ordered terminal suffix

The recognizer accepts only this ordered sequence:

1. `Changes` with the existing `+…` value shape;
2. `Requests` with the existing leading-digit value shape;
3. `Tokens` with the existing `↑…` value shape;
4. `Resume` with the existing `copilot --resume=…` value shape.

A candidate starts only on `Changes`. A lone `Requests`, `Tokens`, or `Resume`
line is emitted immediately as prose.

Candidate lines are held until the session reaches its normal `exit` event.
At that point:

- an exact completed sequence at the end of output is discarded as Copilot's
  statistics trailer;
- an incomplete sequence is emitted verbatim;
- a candidate followed by any non-trailer prose is emitted verbatim before
  that prose;
- an out-of-order line flushes the candidate, then is reprocessed so a new
  `Changes` line can start a fresh candidate.

Optional blank lines after a complete candidate may remain held until exit.
The state records at most four trailer lines plus whether trailing blank
spacing occurred; it never accumulates arbitrary output.

### D3 — Keep state with the PTY line buffer

The existing per-session `PtyLineBuffer` is the ownership boundary for:

- the raw incomplete line;
- the number of already-rendered cleaned characters;
- the bounded Copilot trailer candidate.

Keeping these together makes line assembly happen before trailer
classification and reuses the established session cleanup paths. The map entry
remains alive while any of those three states is non-empty.

Pure helpers classify a complete line by trailer field and advance or flush
the candidate. They return visible text to the existing `emit_agent_text`
sink; they do not mutate transcript messages directly.

### D4 — Fail-open lifecycle behavior

Normal session completion finalizes a candidate:

- complete terminal trailer → discard;
- partial candidate → emit.

Other lifecycle paths preserve existing transcript semantics:

- **kill/cancel:** emit complete candidate lines before dropping an unfinished
  raw line;
- **`/clear`:** drop all buffered state because the transcript was explicitly
  erased; nothing may resurface later;
- **`/new`:** preserve state for a still-running session, matching the current
  PTY-fragment behavior;
- **suppressed stale-session retry:** drop state with the suppressed session,
  because that session's output is intentionally replaced by recovery copy;
- **attach/replay:** start from empty state and reconstruct deterministically
  from recorded events.

No lifecycle path may carry a candidate into a different session.

### D5 — Formatting and ordering

When a false candidate is flushed, its raw cleaned lines retain their original
newlines and ordering. The current live/batched sink remains authoritative:

- live mode appends recovered prose immediately;
- batched mode appends it to the pending buffer;
- neither path injects stream-JSON paragraph separators into PTY data.

Terminal cleanup still runs before classification, so split ANSI sequences,
carriage-return frames, backspaces, whitespace-only continuations, and UTF-8
retractions retain the behavior covered by the existing tests.

## Testing

Implementation proceeds test-first and covers:

1. a lone `Resume     copilot --resume=…` line and all following prose remain
   visible;
2. false-positive lines split across PTY chunks remain visible after their
   newline arrives;
3. a complete ordered Copilot trailer is hidden only when it is the terminal
   suffix;
4. the complete trailer remains hidden when its lines and labels span
   arbitrary PTY chunks;
5. partial and out-of-order candidates flush verbatim at EOF;
6. a complete candidate followed by prose flushes the candidate and prose in
   order;
7. stats-shaped lines stay visible for Codex, Claude fallback, Grok, and an
   unknown harness unless their own established filter says otherwise;
8. live and batched modes produce equivalent assistant text;
9. kill flushes complete candidate lines, `/clear` prevents resurfacing,
   `/new` preserves the active session candidate, and suppressed-session
   cleanup drops it;
10. candidate storage remains bounded;
11. existing Codex role-marker, chunk-split, CR/backspace, ANSI, whitespace,
    UTF-8, and stream-JSON tests remain green.

Required verification:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

## Rollout

One issue-scoped PR from `fix/493-copilot-stats-trailer`. No migration,
configuration change, API change, or user documentation update is required.
The PR should close #493 and describe the fail-open invariant explicitly:
cosmetic trailer filtering may show extra harness metadata, but it must never
silently remove assistant prose.
