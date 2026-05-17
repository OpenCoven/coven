---
summary: "Common setup, daemon, harness, session, and API problems and their fixes."
read_when:
  - Diagnosing a broken Coven setup
title: "Troubleshooting"
description: "Common Coven problems and fixes: install hiccups, daemon and socket failures, harness errors, stuck sessions, and local socket API issues."
---

Start with `coven doctor`. Everything below assumes you have already done that.

## Setup

| Symptom | Likely cause | Fix |
|---|---|---|
| `coven: command not found` | Wrapper not installed | `npm install -g @opencoven/cli`. |
| Wrong wrapper version | Stale global install | `npm install -g @opencoven/cli@latest`. |
| `EACCES` on global install | npm prefix not writable | Use `npx @opencoven/cli` or fix npm prefix. |

## Daemon

| Symptom | Likely cause | Fix |
|---|---|---|
| `daemon: not running` | Daemon stopped | `coven daemon start`. |
| `socket already in use` | Stale socket file | `coven daemon stop`, then remove `<covenHome>/coven.sock`. |
| `daemon ipc timeout` | Slow first boot | Retry after a few seconds; check `coven daemon status`. |

See [Daemon will not start](/help/daemon-wont-start) for deeper triage.

## Harness

| Symptom | Likely cause | Fix |
|---|---|---|
| `harness 'codex' not found` | Codex not on `PATH` | `npm install -g @openai/codex`, then `coven doctor`. |
| `harness 'claude' not found` | Claude Code not on `PATH` | `npm install -g @anthropic-ai/claude-code`, then `coven doctor`. |
| Session exits immediately | Provider auth missing | Run `codex login` or `claude doctor`. |

## Sessions

| Symptom | Likely cause | Fix |
|---|---|---|
| Session stays `pending` | Daemon spawning failed silently | `coven daemon status`, then check `<covenHome>/logs/`. |
| `cwd outside project root` error | `--cwd` is outside the canonical root | Use a path inside the root, or pass an explicit `--project-root`. |
| Attach shows nothing | Replay buffer empty | The session may have already ended; check `coven sessions --all`. |

## API

| Symptom | Likely cause | Fix |
|---|---|---|
| `404` on `/api/v1/...` | Old daemon | Restart with the current binary; verify `apiVersion`. |
| Unknown error code | Daemon upgraded; client did not | Re-pull capabilities from `/api/v1/health` and remap. |

## Still stuck?

Generate a [diagnostics bundle](/help/diagnostics-bundle) and share it.
