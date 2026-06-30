---
name: board
description: >
  Read and write the workspace BOARD.md (shared task board for agents + Val).
  Wraps `bin/board`, a small Python CLI that preserves the markdown schema
  and appends to the activity log automatically. Use this whenever an agent
  proposes, claims, or finishes work that should be visible to the rest of
  the roster.
tags: [board, tasks, workspace, multi-agent, activity-log]
---

# BOARD — shared task board

`BOARD.md` lives at the workspace root. Every active agent (`nova`, `kitty`, `cody`, `pi`, `codex`) and Val read and write to it. The activity log at the bottom is **append-only** — never rewrite it.

Use the `bin/board` CLI instead of editing the file directly. It preserves the schema, restamps timestamps, and writes a matching activity-log row for every mutation.

## When to use

- Proposing a new task → `board add`
- Picking up someone else's task → `board claim`
- Finishing work that's on the board → `board done`
- Checking what's active before starting work → `board list`
- Auditing recent agent activity → `board log`

## When NOT to use

- One-off TODOs that only matter to your current session → use the conversation's todo tool, not BOARD.md
- Long-form reflection / dream entries → `DREAMS.md`
- Repo-specific implementation plans → `plans/{org}/{repo}/<file>.md`

## Commands

```bash
# show one section (or all)
board list
board list today

# add a task to a section
board add today "Move demo script to plans/" --agent nova --ctx "It's at workspace root after demo."

# reassign a task's "by" agent
board claim "demo script" --agent kitty

# flip checkbox to done, move to "Done this week", log it
board done "demo script" --agent kitty

# tail the activity log (default 10 rows, newest first)
board log
board log -n 30
```

## Agent identity

The `--agent` flag wins. If absent, the CLI reads `$OPENCLAW_AGENT`. If that's also unset, it defaults to `nova`. Anything outside the roster (`nova`, `kitty`, `cody`, `pi`, `codex`, `val`) is rejected. Fishy retired on 2026-04-28 and should not be used for new board entries.

Set the env var once per agent session so every command stamps correctly:

```bash
export OPENCLAW_AGENT=cody
board add today "Run a competitive scan" --ctx "5 sources, summarize in DREAMS"
```

## Title matching for `claim` / `done`

Pass any case-insensitive substring of the task title. If the substring matches more than one task, the CLI lists candidates and exits non-zero — narrow the query and retry.

```bash
board done "skills"
# error: 'skills' matches 2 tasks:
#   - Commit new skills under `skills/` (pr-agent, prompt-engineer, …)
#   - TinyFish skills scaffolded — search / fetch / browser / agent-run

board done "TinyFish skills"   # unique
```

## Concurrency

Each write takes an `flock` exclusive lock on `BOARD.md`. Two agents running `board add` simultaneously serialize cleanly; humans editing in their text editor bypass the lock and we'll eat the merge conflict if it happens.

## File location override

The CLI walks up from `$PWD` looking for `BOARD.md`. To target a different file (e.g. for testing), set `BOARD_PATH`:

```bash
BOARD_PATH=/tmp/BOARD-fixture.md board list
```

## See also

- `BOARD.md` — the live workspace board (with embedded schema docstring)
- `mockups/board.html` — four-view design prototype (Notebook / Console / Whiteboard / Diary)
- `plans/openclaw/openclaw/knotch-board-plan.md` — feature plan, schema spec, open questions
