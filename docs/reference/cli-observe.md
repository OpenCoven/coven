---
summary: "Read-path observability commands: status, familiars, skills, memory, research, calls, hub, and session inspection."
read_when:
  - Checking what your coven is doing from a terminal
  - Scripting against Cave-parity read views
title: "coven observability commands"
description: "Reference for coven status, familiars, skills, memory, research, calls, hub, and sessions show/events/log: terminal parity with the CovenCave dashboard and the daemon API."
---

# Observability commands

Everything the CovenCave dashboard reads is also visible from the terminal.
These commands are **read-only**, work **without a running daemon** (they read
the same `~/.coven` files and SQLite store the daemon serves), and each takes
`--json` for machine-readable output that carries the same body as the
corresponding daemon API route.

| Command | Human view | `--json` body |
|---|---|---|
| `coven status` | Ecosystem overview | `{ "health": …, "overview": … }` composition of `GET /api/v1/health` and `GET /api/v1/overview` |
| `coven familiars` | Roster table | `GET /api/v1/familiars` |
| `coven skills` | Skill inventory | `GET /api/v1/skills` |
| `coven memory` | Memory file table | `GET /api/v1/memory` |
| `coven research` | Research loop log | `GET /api/v1/research` |
| `coven calls [<id>]` | Delegation ledger (list or detail) | `GET /api/v1/coven-calls[/:id]` |
| `coven hub status` | Hub role, nodes, queue depth | `GET /api/v1/hub/status` |
| `coven hub nodes` | Executor node table | `GET /api/v1/hub/nodes` |
| `coven hub jobs [--state <s>]` | Job table | `GET /api/v1/hub/jobs[?state=s]` |
| `coven hub routing` | Routing decisions | `GET /api/v1/hub/routing` |
| `coven sessions show <id>` | One session's record | `GET /api/v1/sessions/:id` |
| `coven sessions events <id>` | Recorded events (redacted) | `GET /api/v1/sessions/:id/events` |
| `coven sessions log <id>` | Log lines | `GET /api/v1/sessions/:id/log` |

## coven status

The "what is my coven doing" front door. Complements `coven doctor`
(is my *setup* healthy?) with runtime state:

```text
Coven status

  daemon     running (pid 63321, socket /Users/alice/.coven/coven.sock)
  version    0.1.7
  sessions   3 open
  familiars  1 active / 2 total
  skills     4 installed
  research   12 iterations (last Δ 2)
  hub        1/2 nodes available (details: coven hub status)

Next: coven sessions · coven familiars · coven run <harness> "<task>"
```

- The `familiars` line counts a familiar as **active** when it has an open
  session; the roster comes from `~/.coven/familiars.toml`.
- The `research` line appears only when the research log has rows; the `hub`
  line appears only when executor nodes are registered — a fresh single-host
  install stays quiet instead of alarming.
- `coven overview` is an alias for readers coming from the API route name.

`coven status --json` prints a CLI-level composition of the two stable API
bodies (this shape is owned by the CLI, like `coven daemon status --json`):

```json
{
  "health": { "ok": true, "apiVersion": "coven.daemon.v1", "…": "…" },
  "overview": { "open_sessions": 3, "total_familiars": 2, "…": "…" }
}
```

The `health.daemon` block reflects a *live* daemon only; a stale status file
shows up as `daemon: null` here, while `coven daemon status --json` reports
the `stale` state explicitly.

## Session inspection without a PTY

`coven attach` replays and follows interactively. For scripts, CI, and quick
glances, the `sessions` subcommands read the same ledger non-interactively:

```bash
coven sessions show 9099                   # metadata; unique id prefixes work
coven sessions events 9099 --limit 100     # recorded events, redacted payloads
coven sessions events 9099 --after-seq 42  # resume from a cursor
coven sessions log 9099                    # replay-style log lines, then exit
```

`events --json` returns the paginated envelope
`{ "events": [...], "nextCursor": { "afterSeq": n }, "hasMore": bool }` —
the same contract as `GET /api/v1/sessions/:id/events`, so a shell loop can
page with `--after-seq` exactly like an API client. Event payloads are
redacted by default before display, matching the API.

## Hub operations without curl

`coven hub` replaces hand-rolled `curl --unix-socket` calls for the read side
of hub operations (see [HUB-OPERATIONS](../HUB-OPERATIONS.md) for the
write-side protocol, which stays machine-to-machine):

```bash
coven hub status                 # role, hubId, node availability, queue depth
coven hub nodes                  # registered executors with capabilities
coven hub nodes <id>             # one node: transport, health, capabilities
coven hub jobs --state queued    # global queue by state
coven hub jobs <id>              # one job: state, route, payload preview
coven hub dispatch <jobId>       # executor dispatch record + result envelope
coven hub routing                # job→node routing decisions
```

Every verb takes `--json`, which prints the matching `/api/v1/hub/*` response
body unchanged, so scripts and humans read the same contract.

## Empty states teach setup

Each command's empty state points at the file or flow that populates it, so a
fresh install can navigate the ecosystem without reading source:

```text
$ coven familiars
No familiars configured.
Add [[familiar]] entries to ~/.coven/familiars.toml to build your roster.
```

## In the interactive surfaces

The same views are reachable without leaving the interactive surfaces: the
Cast composer and the chat UI accept `/status` (alias `/overview`),
`/familiars`, `/skills`, `/memory`, `/research`, `/calls`, and `/hub`, plus the
bare words (`status`, `familiars`, `roster`, …). Every card shows the
scriptable `coven <view>` spelling so the terminal form stays discoverable.
`status` in a composer means this ecosystem overview; setup checks stay on
`doctor`/`health`.

