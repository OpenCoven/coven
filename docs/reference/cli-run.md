---
summary: "Launch a harness session inside a project root."
read_when:
  - Looking up run
title: "coven run"
description: "Reference for coven run: one-shot harness execution that spawns a session, streams events, and records the result in the Coven ledger."
---

## Usage

```bash
coven run <harness> <prompt> [flags]
```

`<harness>` is a configured harness id such as `codex` or `claude`.

## Common Flags

| Flag | Behavior |
|---|---|
| `--cwd <path>` | Launch from a directory inside the resolved project root. |
| `--title <text>` | Store a readable session title. |
| `--model <id>` | Forward a model override to harnesses that declare model support. Namespaced ids such as `anthropic/claude-sonnet-4` are forwarded to the harness as the bare model id. |
| `--think` | Request deeper reasoning. Claude maps this to `--effort high`; unsupported harnesses warn and continue. |
| `--speed <level>` | Set a latency/reasoning hint: `fast`, `balanced`, or `thorough`. Claude maps these to `--effort low`, `medium`, or `high`; unsupported harnesses warn and continue. |
| `--detach` | Create the session record without launching the harness. |
| `--continue [id]` | Resume a specific session, or the latest active session for this project when `id` is omitted. |
| `--labels <a,b>` | Attach comma-separated labels to a new session. |
| `--visibility <private\|workspace\|shared>` | Set session visibility metadata. |
| `--archive` | Archive the session after the run completes. |
| `--familiar <id>` | Inject familiar identity context. |
| `--stream-json` | Emit Coven JSONL events on stdout. stdout carries only JSONL for every harness: non-stream harnesses (codex, external adapters) have their raw PTY output wrapped in `output` events. See `docs/STREAM-JSON.md`. |
| `--stream-json-input` | With `--stream-json`, read JSONL user messages from stdin for Claude stream mode. |

Examples:

```bash
coven run claude "audit this branch" --think
coven run claude "make the smallest fix" --speed fast
coven run codex "fix the failing tests" --speed thorough
```
