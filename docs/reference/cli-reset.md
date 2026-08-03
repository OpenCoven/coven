---
summary: "Preview or recoverably reset selected local Coven state."
read_when:
  - Debugging local Coven configuration or cached state
  - Recovering a familiar, project registry, GitHub integration, or session profile
title: "coven reset"
description: "Reference for coven reset: preview and recoverably reset selected COVEN_HOME state categories with explicit apply confirmation, local backups, JSON output, and no remote side effects."
---

# `coven reset`

`coven reset` is a local debugging and recovery tool. It only handles an
explicit allowlist of paths below the active `COVEN_HOME` (normally
`~/.coven`). It never accepts an arbitrary path, follows a reset-target
symlink, deletes a project directory, contacts GitHub, revokes a credential,
or changes remote state.

Preview is the default. A reset moves selected state into a recoverable backup
under `COVEN_HOME/reset-backups/`; it does not permanently delete it. Apply is
currently supported on Unix platforms. Windows supports feature discovery and
preview, but rejects `--apply` before creating locks or changing state because
Windows does not provide the durable directory-entry ordering this recovery
contract requires.

```sh
# Discover exact local categories and their warnings.
coven reset --list-features

# Preview one category. No files change.
coven reset --feature familiars

# Preview several independent categories.
coven reset --feature projects --feature github

# Stop the daemon, then move selected local state into a timestamped backup.
coven daemon stop
coven reset --feature projects --feature github --apply

# Reset every registered safe category. --all always requires --apply.
coven reset --all --apply
```

## Features

| Feature | Local state selected | Sensitivity | Boundary |
| --- | --- | --- | --- |
| `familiars` | `familiars.toml`, `familiars/` | Standard | Does not affect external repositories. |
| `projects` | `repos.toml` | Standard | Does not delete project directories, Git repositories, or XDG `settings.json` entries. |
| `github` | Coven-local GitHub/Copilot adapter records | Sensitive | Does not contact GitHub, revoke tokens, alter accounts, dispatch workflows, or alter repositories. |
| `claude` | Coven-local `adapters/claude.json`, if present | Sensitive | Does not alter Claude Code's own configuration, login, projects, or provider account. |
| `openclaw` | Coven-local `adapters/openclaw.json`, if present | Sensitive | Does not alter OpenClaw core, the external bridge plugin configuration, ACP routing, or provider credentials. |
| `hermes` | Coven-local `adapters/hermes.json` | Sensitive | Does not alter Hermes Agent configuration, login, models, or provider credentials. |
| `opencode` | Coven-local `adapters/opencode.json` | Sensitive | Does not alter OpenCode configuration, project files, login, or provider credentials. |
| `grok-build` | Coven-local `adapters/grok.json` | Sensitive | Does not alter Grok Build configuration, login, `XAI_API_KEY`, or provider account. |
| `gemini` | Coven-local `adapters/gemini.json`, if present | Sensitive | Does not alter Gemini CLI configuration, login, MCP servers, or Google account state. |
| `secrets` | Reserved `secrets/` references | Sensitive | Does not move the artifact key independently from encrypted session data. |
| `caches` | `cache/`, `capabilities-cache.json` | Standard | Coven recreates local cache data only through normal use; reset performs no refresh. |
| `sessions` | SQLite ledger files, `session-artifacts/`, `chat-conversations/`, and `keys/session-artifacts.key` | Sensitive | Keeps encrypted records and the key generation that decrypts them in one backup. |
| `mobile` | `mobile/` gateway configuration, pairings, host identity, and audit state | Sensitive | Devices must pair again after reset. |
| `metadata` | Local calls, research, travel, pending, executor, and recovery metadata | Standard | Does not reset daemon connection settings or external services. |

`--all` expands only to this explicit allowlist. It does not mean “delete all
files in `COVEN_HOME`.” New features must be registered in the client before
they can be selected.

## Safety model

Every normal Coven command holds a shared state lock for its lifetime. Before
applying a reset, Coven must acquire that lock exclusively; this fails fast
while the daemon, a harness run, or any other Coven command is active. It also
holds the daemon lifecycle and serve locks and probes daemon health so a daemon
started by an older CLI cannot overlap the reset. Stale lock files alone do not
block reset. The command opens the active `COVEN_HOME` once and acquires reset
locks and performs moves relative to that same directory handle. On Unix, the
direct parent is required to be current-user-owned and not writable by other
users, and its identity is checked across the handle open. Windows preview
walks parent components without following reparse points. Reset-target
ancestors are always opened without following symlinks or reparse points, so
replacing a validated path component cannot redirect a move. If one path in a
feature fails, earlier moves for that feature are rolled back; independent
features continue and each outcome is reported.

Before the first move, Coven durably records the complete relative-path
transaction in `reset-transaction.json`. Normal commands refuse to start while
that marker exists. If reset is interrupted, the next `coven reset ... --apply`
rolls every completed move back to its original location before starting the
new reset. This keeps session records and their artifact key together even
across termination or power loss.

This durable apply contract is currently Unix-only. Windows preview uses the
same allowlist and no-follow path validation, but apply remains fail-closed
until Coven has a Windows transaction mechanism with equivalent crash ordering.

Successful moves are placed under a directory such as:

```text
COVEN_HOME/reset-backups/2026-07-27T22-15-00Z-<id>/projects/repos.toml
```

To recover, move the backed-up item back to the original feature path after
stopping any process that uses it. Never copy secret values into issue reports
or terminal transcripts; the reset command prints only category names and
relative state paths, never file contents.

## Debugging examples

### Familiar state

Preview before clearing a broken familiar registry or managed familiar
workspace:

```sh
coven reset --feature familiars
coven reset --feature familiars --apply
```

### Project registration

Use this when an old `repos.toml` registration points at a moved checkout. The
registered project itself stays untouched:

```sh
coven reset --feature projects --apply
```

### Runtime integration state

Use these only for Coven-local adapter state. Each selector is independent, so
debugging a Hermes, OpenCode, Grok Build, Gemini, Claude, or OpenClaw bridge
record does not disturb the other runtime records. None of them changes the
provider CLI, its login, or its configuration:

```sh
coven reset --feature github --apply
coven reset --feature hermes --feature opencode --apply
coven reset --feature grok-build
```

### Local secret and encrypted-session state

Artifact keys cannot be reset independently from their encrypted records.
Preview and reset `sessions` to keep both in the same generated backup:

```sh
coven reset --feature sessions
coven daemon stop
coven reset --feature sessions --apply
```

## Output and exit codes

Use `--json` for a machine-readable plan or result. JSON contains feature
names, relative state paths, outcomes, and backup locations. The active home is
reported as `<coven-home>` rather than an absolute host path, and output never
contains secret values, tokens, or credential contents.

| Exit code | Meaning |
| --- | --- |
| `0` | Preview or reset completed; missing categories may be reported when other selected work completed. |
| `2` | Invalid selection, unknown feature, duplicate feature, incompatible flags, or `--apply` on Windows. |
| `3` | Confirmation is required, another Coven command holds the shared state lock, or a legacy daemon is active. |
| `4` | None of the selected categories has local state. |
| `5` | One or more selected categories could not be backed up/reset; inspect the report and local backup. |
| `6` | Coven refused an unsafe path or symlinked reset target. |

Related: [COVEN_HOME](/daemon/coven-home), [Troubleshooting](/TROUBLESHOOTING),
and the [CLI reference](cli.md).
