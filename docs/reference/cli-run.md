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

`<harness>` is a configured harness id such as `codex`, `claude`, or `copilot`.

## Common Flags

| Flag | Behavior |
|---|---|
| `--cwd <path>` | Launch from a directory inside the resolved project root. |
| `--add-dir <path>` | Grant the harness access to an additional directory beyond its cwd; repeat the flag for multiple directories. Maps to each harness's native trust flag (`--add-dir` for codex, claude, copilot, and coven-code). Harnesses with no add-dir mechanism warn and continue. |
| `--permission <full\|read-only>` | Set the harness sandbox policy. `full` is the default; `read-only` maps to the harness's native sandbox or permission-mode flag. Harnesses without a declared sandbox mechanism warn and continue. |
| `--title <text>` | Store a readable session title. |
| `--model <id>` | Forward a model override through the adapter's declared transform. `strip_provider` (the legacy default) removes the first non-empty provider segment when the remainder is non-empty and does not start with `/`; degenerate ids such as `openai//gpt` remain unchanged. `preserve` forwards the provider-qualified id unchanged. Values that are unsafe for process argv after transformation are rejected before launch. |
| `--think` | Request deeper reasoning. Claude, Coven Code, and Copilot map this to `--effort high`; unsupported harnesses warn and continue. |
| `--speed <level>` | Set a latency/reasoning hint: `fast`, `balanced`, or `thorough`. Claude, Coven Code, and Copilot map these to `--effort low`, `medium`, or `high`; unsupported harnesses warn and continue. |
| `--detach` | Create the session record without launching the harness. |
| `--continue [id]` | Resume a specific session, or the latest active session for this project when `id` is omitted. |
| `--labels <a,b>` | Attach comma-separated labels to a new session. |
| `--visibility <private\|workspace\|shared>` | Set session visibility metadata. |
| `--archive` | Archive the session after the run completes. |
| `--familiar <id>` | Inject familiar identity context. |
| `--stream-json` | Emit Coven JSONL events on stdout. Codex runs as `codex exec --json` over ordinary pipes and is normalized into `assistant` / `result` events; on Windows its npm `.cmd` shim receives the prompt on stdin rather than through ConPTY. Other external non-stream adapters have raw PTY output wrapped in `output` events. See `docs/STREAM-JSON.md`. |
| `--stream-json-input` | With `--stream-json`, read JSONL user messages from stdin for Claude stream mode. |

Examples:

```bash
coven run claude "audit this branch" --think
coven run claude "make the smallest fix" --speed fast
coven run codex "fix the failing tests" --speed thorough
```
