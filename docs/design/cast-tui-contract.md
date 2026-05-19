---
summary: "Phase 1 visual contract for the Cast/Coven TUI: sleek minimalist Cast Codes target. Defines surfaces, spacing, typography, color roles, panel hierarchy, copy tone, and explicit anti-patterns."
title: "Cast TUI — Visual Contract (Phase 1)"
description: "Design contract the Cast TUI must implement against. Not for end users; the audience is OpenCoven contributors working on the Coven CLI."
---

# Cast TUI — Visual Contract (Phase 1)

This document is the design target for the Coven CLI's interactive surfaces: the launcher ("magical TUI"), the Cast plan/outcome cards, and the non‑interactive Cast frame. It is intentionally short. Phase 2+ implements against it; do not deviate without amending this file.

Anchors: [DESIGN.md](../../DESIGN.md), [BRAND.md](../BRAND.md), [`brand/ui/color-tokens.css`](../../brand/ui/color-tokens.css), [`brand/ui/typography.css`](../../brand/ui/typography.css).

## 1. Surfaces in scope

| Surface | Source | Notes |
| --- | --- | --- |
| Launcher frame ("Coven home") | `crates/coven-cli/src/tui/shell.rs::render_magical_tui_frame_with_mode_and_width` | The screen `coven` opens to. Today contains workspace map, status block, task inbox, input box, slash list, selected‑command panel, store footer. |
| Cast non‑interactive frame | `crates/coven-cli/src/tui/cast/render.rs::render_cast_frame_with_mode` | Printed when stdout is piped or stdin is not a TTY. |
| Cast plan intro card | `crates/coven-cli/src/tui/cast/render.rs::render_plan_intro_with_mode` | Shown before any side effect; describes the resolved intent and safety decision. |
| Cast outcome card | `crates/coven-cli/src/tui/cast/render.rs::render_outcome_with_mode` | Shown after the spell finishes; describes what landed and the next step. |
| Cast transcript banner | `crates/coven-cli/src/tui/shell.rs::dispatch_via_daemon` (`println!("Cast transcript — session …")`) | The "Press Enter at any time to send input." line printed before live output streams. |
| Cast exit summary line | `crates/coven-cli/src/tui/shell.rs::TranscriptObserver::on_exit` | `[Cast: session <status> (exit code N)]` line that closes a transcript. |

Out of scope (Phase 1):
- The ratatui‑based chat TUI surfaces (`crates/coven-cli/src/tui/chat/`) — separate visual track.
- Session browser (`tui/sessions.rs`) — touched in a later phase, not now.
- Doctor, patch, daemon‑status output — these are CLI prose, not surfaces.

## 2. Design contract

The whole product is one frame: black surface, monospace, sparse. Cast Codes should feel like reading a quiet status board, not a dashboard.

### 2.1 Surfaces and panels

- One surface tone: `--oc-surface-0` (`#000000`). No nested filled boxes. If a panel needs separation, separate it with a single line of `--oc-border-subtle` (`rgba(255,255,255,0.08)`) drawn as a single thin rule, never with `+---+` corner art.
- At most **two** visual panels per frame:
  1. A header band (3–4 lines max: brand name, one‑line context, one‑line status).
  2. A body region (plan/outcome fields *or* slash list, never both stacked deep).
- Replace the current `+--- Workspace map ---+`, `+-- Ask anything --+`, and `Selected command` blocks with a single bordered prompt area + an unbordered list. Heavy ASCII boxes are removed; if a border is needed at all, use a single horizontal rule above and below the content (`────`), no corners.

### 2.2 Spacing and density

- One blank line between sections. Never two. Never zero.
- Field rows: `label    value` with a fixed label column of 14 chars (the longest field label, e.g. `Default harness`). All callsites use the same column so the eye locks onto value text.
- Wrap at the smaller of terminal width − 2 and 96 columns (`MAGICAL_TUI_MAX_INNER_WIDTH`). Truncate with `…` only at the end of a logical row; never mid‑label.
- Maximum 6 visible items in the slash list before scrolling indicator. The current frame shows 13 items always — that violates the "generous negative space" rule.

### 2.3 Typography (within a monospace TUI)

The DESIGN.md type stacks (Inter / Satoshi) do not apply inside a terminal — everything is `--oc-font-mono`. What we adapt:

- **Headers**: rendered as a single line in `PRIMARY_STRONG` (`#9A8ECD`), Title Case, no decoration. *No* leading sigils, em‑dashes, or `>` glyphs.
- **Labels**: rendered in `FIELD_LABEL` (`TEXT_MUTED`), lowercase, no colon at the end of the rendered string — the column gap is the separator. Allowed exception: ALL‑CAPS short label chips like `SAFE`, `CONFIRM`, `REJECT`, `LIVE` (semantic only, two‑word max).
- **Body values**: `TEXT` (default white at 94% on black).
- **Hints / footer**: `DIM` (`TEXT_FAINT`), one line, never more than 80 chars.
- No mixed‑case slash commands; commands always render as `/<lowercase>`.
- Headlines never end in punctuation. Body sentences do.

### 2.4 Color roles

Honour the 90/10 rule. The renderer must use **only** these semantic tokens; raw `Rgb` literals or new hex strings are forbidden outside `theme.rs`.

| Role | Token | When |
| --- | --- | --- |
| Default text | `TEXT` (`brand::TEXT`) | All body content and values. |
| Muted label | `FIELD_LABEL` (= `TEXT_MUTED`) | Field labels, hints inside a panel. |
| Faint hint | `DIM` (= `TEXT_FAINT`) | Footer hints, keyboard shortcuts, "Esc quits". |
| Strong accent | `PRIMARY_STRONG` (`#9A8ECD`) | Headlines, the selected slash row, the active risk chip if `safe`. |
| Soft accent | `PRIMARY` (`#C5BDED`) | Hover/secondary highlight; cursor underline; the `>` selection arrow. |
| Danger | `DANGER` (`#FF3B30`) | `REJECT` chip; destructive confirmation prompts. |
| Success | `SUCCESS` (`#30D158`) | `LIVE`, `OK`, `exit 0` chip. |
| Info | `ACCENT_BLUE` (`#0A84FF`) | Reserved for actionable links/keys only (e.g. inline `coven attach <id>` references); never used as a generic UI accent. |

Surfaces stay `--oc-surface-0` (pure black). `SURFACE_1`/`SURFACE_2` are not used by the TUI in Phase 2; they exist for future ratatui panels.

`USER_LABEL` (`#7A6DAA`) loses its current launcher role (it is currently used for everything from welcome to input‑box rows) — in the new contract it is reserved for differentiating the *user prompt line* from agent output inside the transcript only.

### 2.5 Hierarchy

Every Cast frame reads top‑to‑bottom in this order; sections may be omitted but never reordered:

1. **Identity** — one line. `Cast` in `PRIMARY_STRONG`, nothing else.
2. **Context** — at most three field rows: `project`, `harness`, `daemon`. Single column.
3. **Body** — one of:
   - Plan: `spell`, `harness`, `risk` (+ optional reason line), then a numbered step list (max 4 visible).
   - Outcome: `spell`, `launched`, `session`, then optional `notes` (max 3), then `next`.
   - Launcher: prompt input, then the slash list (max 6 visible).
4. **Footer hint** — one `DIM` line. Never two.

The current `Welcome back to the Coven.` / `OpenCoven terminal home for local agent work.` / `Workspace map` / `Status` / `Task inbox` / `Slash commands` / `Selected command` / `Store: ~/.coven` stack collapses to: identity → context (3 rows) → prompt → slash list → footer.

### 2.6 Copy tone

- Confident, restrained, direct. Lifted from BRAND.md: "Arcane but precise, technical not gimmicky."
- One sentence per surface, max. Plan and outcome fields are noun phrases, not sentences.
- No second‑person address ("Welcome back"), no first‑person Cast ("I will…").
- Forbidden tropes: "circle fades", "magical mode", "magical terminal mode", "the circle", "your Coven familiar is ready" repeated. The familiar's name does the work; the prose does not need to lampshade it.
- Risk reasons are noun‑first: `push to remote — pre‑flight required`, not `! push to remote — please confirm`.

### 2.7 Selection and interaction

- Slash list selection: the selected row is rendered in `PRIMARY_STRONG`; unselected rows in `TEXT_MUTED`. The leading marker is a single space for unselected and a thin `›` (U+203A) for selected — not `>`.
- Input area: single thin horizontal rule above and below (`────`), no corners, no label inside the rule. The placeholder is rendered in `DIM`. The cursor is implicit (terminal cursor); no synthetic `█` block.
- Focus glow (DESIGN.md §9) translates to: when the prompt has focus the underline rule below the input is `PRIMARY_STRONG`; otherwise it is `--oc-border-subtle`.
- Keys hint: a single `DIM` line at the bottom — `enter run · ↑↓ select · esc quit · ctrl+u clear`. Centered‑dot separator, no `|` pipes.

### 2.8 Risk chips

`safe` / `confirm` / `reject` render as fixed‑width 8‑char ALL‑CAPS chips:

```
[ SAFE  ]   PRIMARY_STRONG
[ CONFIRM]  DANGER (muted via PRIMARY when only a soft warning)
[ REJECT ]  DANGER
```

The current `! reason — suggestion` and `X reason — alternative` lines drop the leading glyph entirely; the chip carries the semantic, the reason line is plain prose under it.

## 3. Anti‑patterns — what NOT to build

These exist in the current code and must not survive Phase 2.

1. **ASCII chrome boxes**: `+---+ | +---+` corner art for `Workspace map`, `Ask anything`, `Input box`. Remove. Use single‑rule `────` only when separation is necessary.
2. **Decorative graphs**: the `[nova] — [coven] — [cody]` workspace map. Cast does not have nova/cody nodes as first‑class concepts at the launcher level; this is gimmicky flourish DESIGN.md §10 forbids.
3. **Fake task inboxes**: `[ ] inspect repo  [ ] launch harness` — these are not real tasks and they imply state we do not track. Delete.
4. **Multiple stacked section headers** ("Status", "Task inbox", "Slash commands", "Selected command") on the same frame. Collapse to the §2.5 hierarchy.
5. **Repeated brand voice**: `Cast — your Coven familiar` header *and* `Cast, your Coven familiar, is ready…` salute on the same screen. Choose one identity line.
6. **Emojis or pictographs in UI text**: per memory rule (2026‑05‑17). The current code does not use any, but Phase 2 must not introduce sigil glyphs, sparkles, or check‑mark glyphs.
7. **Glyph‑prefixed risk lines** (`!`, `X`): semantics belong in a typed chip, not a punctuation mark.
8. **`=>` and `|` separators** in copy: `/start => coven doctor`, `… | …`. Replace with column gap or a thin middle dot (`·`).
9. **Two visual languages in one product**: heavy ASCII boxes in the launcher next to plain key:value cards from Cast plan/outcome. They must share one chrome system.
10. **Filled surface tones inside the TUI** (`SURFACE_1`, `SURFACE_2`): unused in Phase 2; keep the canvas pure black.
11. **Saturated purple**: `PURPLE_3` (`#C5BDED`) currently maps to `PRIMARY` (the most common accent). Swap so `PRIMARY_STRONG` (`#9A8ECD`) carries weight and `PRIMARY` is hover/secondary only.
12. **`Store: ~/.coven` footer line**: implementation detail, not a user signal. Remove.

## 4. Files Phase 2 will edit

This list is the seam — Phase 2 should touch these and only these.

- `crates/coven-cli/src/tui/shell.rs` — rewrites `render_magical_tui_frame_with_mode_and_width`, removes `magical_tui_graph_lines`, `magical_tui_status_lines`, `magical_tui_task_inbox_lines`, the `magical_tui_input_box_*` helpers, and the `Selected command` block.
- `crates/coven-cli/src/tui/cast/render.rs` — rewrites `render_cast_frame_with_mode`, `render_plan_intro_with_mode`, `render_outcome_with_mode` against the new field‑column rule; introduces a single `chip(…)` helper for risk badges.
- `crates/coven-cli/src/theme.rs` — adds `BORDER_SUBTLE` and `BORDER_STRONG` semantic tokens (mirroring `--oc-border-subtle` / `--oc-border-strong`). No existing tokens are renamed; `PRIMARY`/`PRIMARY_STRONG` *roles* shift via callsite changes, not by remapping the constants.
- Tests in `crates/coven-cli/src/tui/cast/render.rs` and `crates/coven-cli/src/tui/shell.rs` (the `render_magical_tui_frame_plain*` tests and the existing Cast render tests) — assertions update to match the new copy. Any test that asserts the presence of `+----+`, `[ ] inspect repo`, `Workspace map`, `Store:`, or `Cast — your Coven familiar` will be rewritten.

No other crate is touched in Phase 2. `brand/ui/*.css` is canonical and stays as‑is; we adapt the TUI to it, not the other way around.

## 5. Done‑when checklist (for the implementer in Phase 2)

- [ ] Launcher renders ≤ 22 lines on an 80‑column terminal with default content.
- [ ] No `+`, `|`, `\`, `/` ASCII corner art remains in any rendered frame.
- [ ] Every accent color comes from a semantic theme token; `rg "Rgb \{" crates/coven-cli/src/tui` returns nothing.
- [ ] Cast plan, outcome, and launcher frames all use the 14‑char label column.
- [ ] Tests in `tui/cast/render.rs` and `tui/shell.rs` pass with assertions updated to the new copy.
- [ ] `coven` (interactive) and `coven | cat` (piped) both render frames matching §2.5 hierarchy; no surface‑specific divergence.
