---
name: memory-timeline-manager
description: Manage short-term timeline memory and long-term MEMORY.md curation safely (capture daily notes, label retention, promote snippets, and run review sweeps). Use when asked to add to timeline memory, promote notes to long-term memory, or organize/prune memory files.
---

# Memory Timeline Manager

Use this skill for the memory workflow:
1) Capture quickly in `memory/YYYY-MM-DD.md`
2) Curate durable facts in `MEMORY.md`
3) Promote important snippets from timeline to long-term memory
4) Keep changes safe and reversible

## Safety rules (always)

- Allowed write targets:
  - `MEMORY.md`
  - `memory/*.md`
- Never write outside those paths unless user explicitly asks.
- Prefer additive edits over destructive rewrites.
- Before deleting or bulk-pruning memory files, ask for confirmation.
- For uncertain/large edits, create a dated backup file in `memory/` first.

## Fast routing

- "remember this" / daily logs / quick note → append to `memory/YYYY-MM-DD.md`
- stable preferences / decisions / recurring schedule / project truths → `MEMORY.md`
- "promote this" → append snippet to `MEMORY.md` with timestamp heading
- cleanup/review requests → follow retention sweep in `references/retention.md`

## Standard operations

### 1) Capture timeline note

- Target file: `memory/<today>.md`
- If file missing, create it with a date heading.
- Append concise bullet(s), preserving chronology.

### 2) Promote snippet to long-term memory

Use `scripts/promote_snippet.py` when available for deterministic formatting.

- Add heading:
  `## Promoted from Timeline (YYYY-MM-DD HH:MM:SS)`
- Add cleaned snippet text.
- Keep entries short and factual.

### 3) Retention labeling (lightweight)

Use labels in note text or sidecar metadata when requested:
- `keep`
- `review`
- `archive`
- `delete-candidate`

When labels are in text, use `[#label]` suffix on bullets for compatibility.

### 4) Weekly review sweep

- Read recent `memory/YYYY-MM-DD.md` files
- Identify durable items worth keeping
- Promote to `MEMORY.md`
- Optionally mark stale notes as `archive` / `delete-candidate`

See `references/retention.md` for checklist.

## File conventions

- Timeline files: `memory/YYYY-MM-DD.md`
- Long-term memory: `MEMORY.md`
- Avoid creating extra docs unless user asks.

## References

- Retention & review checklist: `references/retention.md`
- Section taxonomy guidance: `references/memory-taxonomy.md`
