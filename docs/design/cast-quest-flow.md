---
summary: "Phase 5 design contract for Cast's sequential goal flow: deterministic sub-prompts, visible handoffs, and a structured advance step between phases."
title: "Cast — Sequential Goal Flow (Phase 5)"
description: "How Cast turns a high-level user goal into an ordered Quest of phases, each producing a concrete sub-prompt and a visible handoff to the harness."
---

# Cast — Sequential Goal Flow (Phase 5)

This document is the design target for the Cast quest flow added in Phase 5. The implementation lives in `crates/coven-cli/src/tui/cast/quest.rs` (pure logic) and `crates/coven-cli/src/tui/cast/render.rs` (`render_quest_handoff`). The Cast shell wires the quest into its existing gate / follow / outcome surfaces in a follow-up phase; the module is intentionally callable on its own first so the deterministic core can be exercised by tests without a daemon.

## 1. Why a quest

Phase 4 and earlier treat every spell as a single launch: parse → plan → gate → dispatch → outcome. A real piece of repository work is rarely one launch — it is design → implement → verify, with the next step shaped by what just happened. Phase 5 makes that loop a first-class Cast surface so the user can:

1. State a high-level goal once.
2. Read the concrete sub-prompt Cast would hand to a harness for the *first* phase.
3. Approve, edit, or skip that sub-prompt.
4. Inspect the result.
5. Read the recomposed sub-prompt for the next phase, with a visible note about *why* it changed.

No LLM planner is introduced inside Cast. Sub-prompts are assembled from structured templates plus the prior phase's recorded outcome, so every handoff is reproducible, inspectable, and overridable.

## 2. Data model (`cast::quest`)

```text
Quest
 ├ title         derived from the user's goal (truncated to 60 chars)
 ├ goal          the original free-text request
 ├ phases        Vec<QuestPhase>  (default rhythm: design → implement → verify)
 └ cursor        index of the next non-complete phase

QuestPhase
 ├ name          short identifier: "design" | "implement" | "verify"
 ├ goal          noun-phrase role description for this phase
 ├ harness       Option<CastHarness>  (defaults to the quest's harness)
 ├ template      base sub-prompt template (with `{goal}` substitution)
 ├ sub_prompt    currently-resolved text Cast would send right now
 ├ status        Pending | Running { session_id } | Complete(summary) | Skipped { reason }
 ├ handoff       Option<QuestHandoff>  (attached by `advance` from the prior phase)
 └ edited_by_user  true once a user override lands; prevents silent regeneration

QuestPhaseSummary
 ├ session_id        daemon session that ran this phase (if any)
 ├ exit_status       e.g. "completed", "failed", "interrupted"
 ├ exit_code         Option<i32>
 └ carried_context   bulletable facts to surface in the next sub-prompt

QuestHandoff
 ├ from_phase        the prior phase's `name`
 ├ prior_status      human-readable label (e.g. "completed (exit 0)")
 ├ reason            *why* the next sub-prompt was updated
 └ carried_context   verbatim from the prior summary
```

## 3. Composer (`compose_sub_prompt`)

Pure function. Returns:

```text
<template with {goal} substituted>

Handoff from phase `<from_phase>` (status `<prior_status>`):
- <reason>
- <carried_context bullet 1>
- <carried_context bullet 2>
…
```

The handoff block is omitted on the first phase. This is the *exact* text the harness receives.

## 4. Advance step (`advance`)

```text
advance(quest, summary) →
  1. Snapshot the current phase's name + status.
  2. Mark current phase Complete(summary).
  3. Move cursor forward by one.
  4. If a next pending phase exists:
       attach QuestHandoff { from_phase, prior_status, reason, carried_context }
       if !next.edited_by_user: recompose next.sub_prompt
  5. Return Some(next_index) or None.
```

Failure-flavoured reasons (`"failed"`, `"error"`, `"exit 1"`, `"interrupted"`) produce a different handoff sentence (`"incorporate the failure context before continuing"`) than success (`"carry its result into the next sub-prompt"`). Tests pin this distinction so the user always sees the right framing.

User edits via `set_phase_sub_prompt(quest, index, text)` are *sticky*: subsequent advances still attach a handoff (so the user can read why Cast wanted to update the prompt), but the `sub_prompt` text itself is preserved verbatim.

## 5. Visible handoff card (`render_quest_handoff`)

Rendered between phases. Follows the §2.5 hierarchy from `cast-tui-contract.md`:

```text
Cast handoff
quest         Ship phase 5 sub-prompting
phase         2/3 · implement
from          design
prior         completed (exit 0)
              Phase `design` finished with `completed (exit 0)` — carry its result into the next sub-prompt.
delegate to   Codex

Carried context
  ·  added `cast::quest` module
  ·  drafted handoff card

Sub-prompt
  Implement the change agreed in the prior design phase. …
  …
  Handoff from phase `design` (status `completed (exit 0)`):
  - Phase `design` finished with `completed (exit 0)` — …
  - added `cast::quest` module
  - drafted handoff card

enter approves the sub-prompt · type to edit · esc cancels
```

- Sub-prompts longer than 8 lines are clipped with a `… N more lines` indicator; the full text still goes to the harness, but the card stays under one screen.
- User-edited sub-prompts get a `· user-edited` tag on the `delegate to` row so the user can tell Cast did not author the current text.
- The card never executes anything — it is a visible *announcement* of the delegation Cast is about to make.

## 6. Shell wiring (next phase)

Two seams will need to land in `crates/coven-cli/src/tui/shell.rs`:

1. A new `CastIntent::Quest { goal }` variant (parsed from `/quest <goal>` or natural-language patterns like `start a quest to …`). The planner builds a `Quest` via `quest_from_goal` and surfaces it through the existing plan/outcome pipeline.
2. Between phases, the shell:
   - prints `render_quest_handoff(&quest, next_index)`,
   - reuses `evaluate_gate` against the next phase's sub-prompt (so the safety classifier still vets per-phase content),
   - dispatches the underlying launch through `dispatch_cast_launch` with the phase's sub-prompt as the harness task,
   - on exit, builds a `QuestPhaseSummary` from the `CastSessionExit` plus any author-supplied carried context, calls `advance`, and loops.

This module deliberately stops short of the shell wiring so Phase 5's contract — sub-prompts, handoffs, advance — is verifiable on its own.

## 7. Done-when (this phase)

- [x] `cast::quest` module compiles and lives next to the existing Cast surface.
- [x] `quest_from_goal` produces a concrete sub-prompt for every phase up front.
- [x] `advance` attaches a structured handoff and recomposes the next sub-prompt deterministically.
- [x] `set_phase_sub_prompt` lets the user override the next sub-prompt and survives the next advance.
- [x] `skip_phase` rolls the cursor past a phase the user judged unnecessary.
- [x] `render_quest_handoff` shows the source phase, prior status, carried context, target harness, and the sub-prompt text the harness will see.
- [x] 15 new unit tests cover the composer, advancer, edits, skip, failure framing, and the render card (incl. long sub-prompts and quest exhaustion).
- [x] Shell wiring for `/quest <goal>` — landed in Phase 7. `dispatch_cast_quest` in `tui/shell.rs` loops phases, gates each sub-prompt, dispatches via `dispatch_cast_launch`, and advances on each `cast.summary`-derived `QuestPhaseSummary`.
- [x] Quest event ledger entries (`cast.quest.*`) — landed in Phase 7. `cast.quest.{started, phase_started, phase_completed, advanced, completed}` are written to the first phase's session (the "anchor"). `cast::find_cast_quest_info` decodes them so `/attach <anchor>` surfaces a one-line quest-anchor note alongside the cast.summary.

## 8. Edit / skip UX per phase (Phase 8)

After Phase 7 auto-advance landed, Phase 8 wired the §5 approve / edit / skip / cancel loop into `dispatch_cast_quest`:

- **`PhaseInteraction` + `parse_phase_action`** (in `tui/cast/quest.rs`) — pure parser, four variants:
  - Empty input → `Approve` (run the phase as Cast composed it).
  - `/skip [reason]` → `Skip { reason }` with a default reason when omitted.
  - `/cancel [reason]` → `Cancel { reason }` with a default reason when omitted.
  - Anything else → `Edit { sub_prompt }` (single-line replacement; the shell loop applies it via `set_phase_sub_prompt`, re-renders the handoff card, and re-prompts so the user can refine, approve, skip, or cancel).
- **`run_phase_interaction`** (in `tui/shell.rs`) — drives the prompt loop with an injectable reader so it is testable without stdin. `Edit` is consumed inside the loop; callers only see one of the three terminal actions.
- **`cast.quest.phase_skipped`** event is written to the anchor when the user skips, alongside the other `cast.quest.*` reconstruction aids.
- **Footer hint** on the Pending handoff card now reads `enter approve · type to replace · /skip [reason] · /cancel [reason]`, matching the parser exactly.

The Edit flow is intentionally a **single-line full replacement** because the per-phase prompt is one line of cooked stdin. Pasting a multi-line block typically arrives as one line on the prompt; for richer multi-line editing the user can use `$EDITOR` outside Cast and paste back. A future phase could swap in a multi-line editor surface.

## 9. Deferred to a later phase

Three §7 deferrals from Phase 7 are still open:

- **Full re-attach state rebuild.** `/attach <anchor>` currently prints a one-line note ("phase N/M, in progress / complete"). Replaying `cast.quest.*` events to reconstruct a `Quest` and re-render the next handoff card is the long pole and a separate phase.
- **Local-PTY fallback ledger.** Quest event writes are best-effort and skipped silently when `dispatch_cast_launch` falls back to the synchronous local PTY path (no daemon, no session id, no anchor). Re-attach will not find a quest there.
- **`QuestPhaseStatus::Running` transition.** The shell loop transitions Pending → Complete directly (synchronous dispatch). A future async / detached-quest UX would set Running between the two.
