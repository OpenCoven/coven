---
title: "Reach Coven core functionality"
summary: "Verify your local CLI, daemon, harness, and session ledger before launching project-scoped agent work."
read_when:
  - Setting up Coven on a development machine
  - Checking whether the CLI can reach its core local runtime
description: "A runnable workflow for doctor, daemon, harness, and session access in Coven."
---

# Reach Coven core functionality

This guide proves the useful local path before you start agent work: the CLI is present, a harness is usable, the daemon/socket is reachable, and session history is readable.

## 1. Inspect the command surface

```sh
coven --help
```

You should see `doctor`, `daemon`, `run`, and `sessions`. Help rendering proves the binary can parse commands, but not that it can reach local state.

## 2. Check readiness

```sh
coven doctor
```

For scripts, gate on the JSON envelope instead of scraping prose:

```sh
coven doctor --json | jq -e '.ok'
```

`doctor` checks the active `COVEN_HOME`, harness visibility in this shell, and daemon condition. You need at least one usable harness. Follow the per-harness hint it prints; provider authentication happens in the harness's own CLI, not in Coven.

## 3. Start or verify the local daemon

```sh
coven daemon status --json
```

If the daemon is stopped, start it and verify again:

```sh
coven daemon start
coven daemon status --json
```

The daemon is a same-user local service. Do not expose its socket over a network. See [daemon configuration](/daemon/configuration) before changing `COVEN_HOME` or service supervision.

## 4. Confirm session-ledger access

```sh
coven sessions --json
```

This reads recorded sessions without opening the interactive browser. For a human-readable table, use:

```sh
coven sessions --plain
```

## 5. Launch one deliberate project-scoped session

Change into the project you want Coven to supervise. The project root is an authority boundary, so do not use a parent directory just for convenience.

```sh
cd /path/to/project
coven run codex "summarize this repository in five bullets" --permission read-only
```

Replace `codex` with a harness that `coven doctor` reports as ready. `--permission read-only` is a useful first-run posture when your selected harness supports it. After the run starts, inspect it with:

```sh
coven sessions
```

## What success means

You have core access when `doctor --json` reports `ok: true`, `daemon status --json` reports a reachable state, `sessions --json` returns a document, and a selected harness can create a project-scoped session. A missing optional adapter does not prevent core use when another supported harness is ready.

## Related

- [Session operations](/guides/session-operations)
- [Troubleshooting core access](/guides/troubleshooting-core-access)
- [Doctor reference](/reference/cli-doctor)
- [Daemon reference](/reference/cli-daemon)
