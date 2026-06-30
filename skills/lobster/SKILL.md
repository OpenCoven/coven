---
name: lobster
description: Run deterministic typed pipelines, workflow files, and approval-gated automations via the Lobster CLI. Use for multi-step automations that need safety (approval gates), statefulness (cursors/checkpoints), and token efficiency (one call vs many LLM re-plans). Triggers on workflow automation, pipeline, deterministic execution, approval gate, email triage, PR monitor.
---

# Lobster — Workflow Shell

Typed, local-first pipeline engine. Deterministic execution, approval gates, persistent state. Saves tokens by replacing multi-step LLM re-planning with one-shot pipeline calls.

## CLI

```bash
lobster '<pipeline>'                    # inline pipeline
lobster run path/to/workflow.lobster    # YAML workflow file
lobster run --file wf.lobster --args-json '{"key":"val"}'
lobster resume --token <token> --approve yes|no
lobster doctor                          # health check
lobster help <command>                  # per-command help
```

## Pipeline Commands

### Data flow
| Command | Purpose | Example |
|---------|---------|---------|
| `exec` | Run OS command | `exec --json --shell 'echo [1,2]'` |
| `where` | Filter | `where 'status=OPEN'` |
| `pick` | Project fields | `pick 'title,author,url'` |
| `head` | First N items | `head --n 5` |
| `sort` | Stable sort | `sort --key updatedAt --desc` |
| `groupBy` | Group by key | `groupBy --key category` |
| `dedupe` | Remove dupes | `dedupe --key id` |
| `map` | Transform | `map --wrap items` / `map status=done` |
| `template` | Render text | `template --text '{{title}} by {{author}}'` |

### Output
| Command | Purpose |
|---------|---------|
| `json` | JSON output |
| `table` | Table output |

### Control flow
| Command | Purpose |
|---------|---------|
| `approve` | Halt for approval (`--emit` for non-TTY/tool mode) |
| `state.get` | Read persistent state |
| `state.set` | Write persistent state |
| `diff.last` | Compare to last snapshot |

### Integrations
| Command | Purpose |
|---------|---------|
| `clawd.invoke` | Call OpenClaw tool endpoint |
| `gog.gmail.search` | Fetch Gmail threads |
| `gog.gmail.send` | Send Gmail |
| `email.triage` | Categorize + draft replies |
| `llm_task.invoke` | Call LLM with typed payloads |
| `workflows.run` | Run named workflow |
| `workflows.list` | List available workflows |

## Pipe syntax

Commands are piped with `|`. Data flows as typed JSON arrays (not text).

```bash
lobster "exec --json --shell 'gh pr list --json number,title,state' | where 'state=OPEN' | pick 'number,title' | head --n 5 | table"
```

## Workflow files (.lobster)

YAML files with steps, env vars, conditions, and approval gates:

```yaml
name: deploy-check
steps:
  - id: tests
    command: exec --json --shell 'pnpm test --json'
  - id: approve
    command: approve --prompt "Tests passed. Deploy to staging?"
    approval: required
  - id: deploy
    command: exec --shell 'pnpm deploy:staging'
    condition: $approve.approved
```

Run: `lobster run deploy-check.lobster`

Steps can reference prior outputs: `stdin: $tests.stdout`

## Tool mode (for OpenClaw integration)

```bash
lobster run --mode tool '<pipeline>'
```

Returns a JSON envelope: `{ ok, status, output, requiresApproval }`

When `status: "needs_approval"`, call `lobster resume --token <token> --approve yes` to continue.

## Built-in recipes

### GitHub PR Monitor
```bash
lobster "workflows.run --name github.pr.monitor --args-json '{\"repo\":\"owner/repo\",\"pr\":123}'"
```
Tracks PR state changes with diff detection.

## State persistence

`state.set` / `state.get` persist JSON values across runs (e.g., last-processed cursor). `diff.last` compares current data to a stored snapshot and emits only changes.

## Key principles

- **One call** — replace 10+ LLM tool calls with one pipeline
- **Approval gates** — hard stops, not prompt hints. Pipeline cannot continue without explicit resume.
- **Stateful** — cursors/checkpoints survive across runs
- **No auth surface** — Lobster never owns tokens; uses env vars or OpenClaw's existing auth
