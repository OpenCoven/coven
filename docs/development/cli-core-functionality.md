---
title: "Coven CLI core functionality for developers"
summary: "The command ownership map, access contract, and verification loop for maintaining Coven's core CLI and daemon surfaces."
read_when:
  - Changing a core coven command or flag
  - Debugging whether a local Coven installation can reach its core runtime
  - Updating CLI, daemon, or workflow documentation
description: "Developer guide to Coven CLI command ownership, local runtime access, safe verification, and documentation maintenance."
---

# Coven CLI core functionality for developers

This is the maintainer map for the CLI paths that make Coven useful: discover a ready environment, reach the local daemon, launch a project-scoped harness session, and inspect or manage the resulting session. It complements the user-facing [CLI reference](/reference/cli), not replaces it.

## Core access contract

Core functionality is reachable when all of these are true:

1. `coven doctor` can inspect a writable `COVEN_HOME` and reports at least one usable harness.
2. `coven daemon status --json` can reach the same-user local socket. A stopped daemon is an expected first-run state; a stale daemon is not.
3. A command runs from an explicit project root, and any `--cwd` remains inside that root after canonicalization.
4. The selected harness is available in the same shell environment as Coven. Provider authentication stays with that harness; Coven does not own provider credentials.
5. The session ledger can persist metadata and events beneath `COVEN_HOME`.

Do not call the CLI healthy merely because `--help` renders. A healthy core path proves readiness, daemon reachability, and a read of the session ledger. A harness launch is a separate, intentional operation because it may cause model work.

## Command ownership

| User outcome | CLI entry point | Primary Rust owner | Contract to preserve |
| --- | --- | --- | --- |
| Discover commands and route free-text input | `coven`, `coven chat`, `coven tui`, `coven help` | `crates/coven-cli/src/main.rs`, `tui/`, `engine.rs` | Bare Coven opens the interactive route; free-text work remains confirmable and recorded. |
| Check local readiness | `coven doctor [--json]` | `main.rs`, `harness.rs`, `paths.rs`, `daemon.rs` | Human output gives repair hints; JSON stays a single machine-readable document. |
| Control the daemon | `coven daemon start/status/restart/stop` | `daemon.rs`, `api.rs`, `paths.rs` | One same-user local daemon owns the socket and state directory. |
| Launch work | `coven run <harness> <prompt>` | `session_launch.rs`, `harness.rs`, `pty_runner.rs`, `store.rs` | Validate project root and cwd in Rust, construct argv safely, then record session/events. |
| Inspect and manage history | `coven sessions`, `coven attach`, `coven archive`, `coven summon`, `coven sacrifice`, `coven kill` | `store.rs`, `daemon.rs`, `tui/` | Archive is reversible; sacrifice and other destructive actions keep explicit confirmation. |
| Read runtime state | `coven status`, `coven familiars`, `coven skills`, `coven memory`, `coven research`, `coven calls`, `coven hub`, `coven scheduler`, `coven travel` | `observe.rs`, `hub.rs`, `control_plane.rs` | Read surfaces do not silently become write paths. |
| Coordinate parallel work | `coven wt`, `coven claim`, `coven hooks` | `parallel_protocol.rs` | Claims are shared through git's common directory and remain TTL-bounded. |
| Repair or diagnose a machine | `coven patch`, `coven pc`, `coven logs`, `coven vacuum` | `patch.rs`, `pc.rs`, `store.rs` | Keep inspection separate from explicit write/repair confirmation. |

`main.rs` is the authoritative parser and dispatch map. When a command changes, start there and follow its delegated module before changing documentation.

## Access paths developers should exercise

Run these from a clean, representative project directory. They are ordered from read-only inspection to daemon activation; the final launch is intentionally opt-in.

```sh
# Parser and command inventory: no daemon or harness execution.
coven --help

# Readiness envelope: succeeds only when a usable local path exists.
coven doctor --json | jq -e '.ok'

# Daemon reachability and socket identity.
coven daemon status --json

# Session ledger read without opening the interactive browser.
coven sessions --json
```

If the daemon is stopped, start it deliberately and repeat the status check:

```sh
coven daemon start
coven daemon status --json
```

Only after those checks should a developer launch a harness session:

```sh
coven run codex "explain this repository in five bullets" --permission read-only
```

Use the harness selected by `doctor`; `codex` is an example, not a requirement. The [core access guide](/guides/core-access) gives the equivalent operator walkthrough.

## Failure classification

| Symptom | Boundary to inspect | First safe action |
| --- | --- | --- |
| `doctor` reports no usable harness | Shell PATH and harness-owned login | Run the printed harness install/login hint, then rerun `coven doctor`. |
| `daemon status` is stale or cannot connect | `COVEN_HOME`, socket, daemon lifecycle | Read `coven daemon status`; then follow [daemon troubleshooting](/help/daemon-wont-start). |
| Launch rejects a cwd | Project-root and canonical-path guard | Run from the intended project root; ensure `--cwd` resolves inside it. |
| Sessions cannot be listed | Daemon/store reachability | Check `coven doctor --json`, then `coven daemon status --json`. |
| JSON consumer breaks | Command-specific serialization contract | Verify the exact command's `--json` reference before changing stdout or stderr. |

Never document a maintainer's absolute path, real session id, token, or provider environment dump. Use `/path/to/project`, `session-1`, and synthetic output in examples.

## Documentation change checklist

When a CLI behavior changes, update the matching layer in the same change:

1. `docs/reference/cli.md` for the command inventory and flags.
2. The focused `docs/reference/cli-*.md` page for semantics and examples.
3. A `docs/guides/` workflow when the change affects an end-to-end task rather than one flag.
4. `README.md` when it changes the first path a developer should discover.
5. `docs/API-CONTRACT.md` for a socket/API compatibility change.
6. `scripts/onboarding-docs-test.mjs` when a discovery link or core workflow must not regress.

For docs-only work, run:

```sh
node scripts/onboarding-docs-test.mjs
python scripts/check-secrets.py
git diff --check
```

## Related

- [CLI reference](/reference/cli)
- [Core access guide](/guides/core-access)
- [Automation and JSON guide](/guides/automation-json)
- [Runtime architecture](/ARCHITECTURE)
- [Documentation maintenance](/DOCS-MAINTENANCE)
