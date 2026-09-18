---
summary: "Environment readiness check."
read_when:
  - Looking up doctor
title: "coven doctor"
source_adjacent_reason: "Documents the doctor checks, install-origin classification, and JSON envelope implemented in this repository's coven-cli crate."
description: "Reference for coven doctor: the first command to run after install. It checks COVEN_HOME, the socket, harness PATH, and the SQLite store."
---

`coven doctor` is the first command to run after installing Coven, changing
`PATH`, authenticating a harness, or moving `COVEN_HOME`.

```sh
coven doctor
```

The command is read-only. It prints local setup state and a next step without
starting a session.

The prose report uses `[OK]` for passing checks, `[--]` for advisory warnings,
and `[!!]` only for blocking failures. It is line-oriented plain text and does
not emit or pass through ANSI escape sequences, including when global color is
forced or configuration text contains terminal controls.

## Machine-readable output

`coven doctor --json` emits one JSON document for scripts and CI gates, with
the same exit contract as the prose output (exit 1 when a blocking problem is
found):

```json
{
  "ok": true,
  "blocking": false,
  "store": "<coven-home>",
  "project": "<project>",
  "checks": [
    { "id": "install:conflicts", "status": "pass", "message": "one coven on PATH" },
    { "id": "daemon", "status": "pass", "message": "running (pid 12345, socket <daemon-socket>)" },
    { "id": "harness:codex", "status": "pass", "message": "`codex` executable is available (built-in)" },
    { "id": "harnesses", "status": "pass", "message": "2 of 4 configured harness executables available" },
    { "id": "engine", "status": "pass", "message": "<engine> (managed install), version 0.6.1 (pin 0.6.1)" },
    { "id": "credentials:engine", "status": "warn", "message": "authentication configured; provider turn not verified", "hint": "run an explicitly authorized test turn to verify provider access" },
    { "id": "credentials:codex", "status": "warn", "message": "executable available; authentication not verified", "hint": "authenticate or inspect local setup with: coven setup codex; verify provider access with an explicitly authorized test turn" }
  ],
  "nextSteps": ["coven run codex \"explain this repo in 5 bullets\"", "coven sessions"]
}
```

Known Doctor-owned absolute path roles are replaced with stable tokens such as
`<coven-home>`, `<project>`, `<engine>`, `<daemon-socket>`, `<repo>`, and
`<repos-config>`. This keeps repeated output comparable across machines and
safer to attach to CI logs or bug reports without implying that arbitrary
user-authored hint text is sanitized. Run the prose form locally when you need
the concrete paths. `project` is `null` when the command runs outside a project
root.

Check `status` is `pass`, `warn`, or `fail`. Every `fail` is blocking — `ok`
is false and the command exits 1 — while `warn` needs attention but does not
block (for example a daemon that has not been started yet). Failing checks
carry a `hint` with the repair command. Gate scripts on the envelope:

```sh
coven doctor --json | jq -e .ok
```

`coven adapter doctor [id] --json` uses the same envelope for adapter
availability, where any missing adapter is a `fail`.

## What it checks

| Section | Meaning |
| --- | --- |
| `Install` / `Installs` | Every `coven` executable on this shell's `PATH`, in resolution order, with how each one was installed. See [Installs](#installs). |
| `Store` | The active Coven state directory. Defaults to `<home>/.coven` unless `COVEN_HOME` is set. |
| `Project` | The current git/project root when the command runs inside a project. |
| `Daemon` | Whether the background daemon is stopped, running, or stale. |
| `Repos` | Configured repositories from Coven repo settings, if present. |
| `Harnesses` | Supported harness executables that are visible on this shell's `PATH`. |
| `Engine` | Whether the Coven engine is installed and meets the minimum supported version. An advisory `[--]` line (JSON check `engine:path`) names a `coven-code` that is first on `PATH` but is not the engine `coven` runs. |
| `Familiars` | Configured familiar identities from `familiars.toml`, if present. |
| `Credentials` | Advisory local engine auth configuration and explicit `authentication not verified` rows for external harnesses. Doctor calls only the engine's contractually offline `auth status --json`; it launches no provider CLI process, performs no provider network request, does not inspect provider tokens or credential stores, and does not verify authentication. |
| `Next steps` | The safest next command based on the detected state. |

## Installs

More than one `coven` on `PATH` is the most expensive install failure: the
first entry answers every command, so an upgrade applied to any other copy
appears to do nothing. Doctor prints this block first, before `Store`, because
every later line describes whichever binary won.

With one install the block is a single line naming its origin:

```text
Install: ~/.local/bin/coven (npm, prefix ~/.local)
```

With several, each copy is listed in `PATH` order with its origin and the
command that removes exactly that copy:

```text
Installs:
  [OK] ~/.local/bin/coven (active, this process) — npm, prefix ~/.local
  [!!] ~/.nvm/versions/node/v24.18.1/bin/coven (shadowed) — npm, prefix ~/.nvm/versions/node/v24.18.1
       remove: npm uninstall -g --prefix ~/.nvm/versions/node/v24.18.1 @opencoven/cli
  [!!] ~/.cargo/bin/coven (shadowed) — cargo install
       remove: cargo uninstall coven-cli
  The first entry wins. Keep one install per machine: remove the shadowed copies with the commands above, then re-check with `coven --version`.
  [!!] `npm install -g` writes to ~/.nvm/versions/node/v24.18.1, a shadowed copy, so upgrades never reach the active install. Upgrade the active copy with: npm install -g --prefix ~/.local @opencoven/cli@latest
```

Origins Doctor recognizes:

| Origin | How it is detected | Removal command |
| --- | --- | --- |
| `npm, prefix <dir>` | The executable is npm's shim: a symlink into `<prefix>/lib/node_modules/@opencoven/cli` (Unix) or a `coven.cmd` beside `<prefix>\node_modules\@opencoven\cli` (Windows). | `npm uninstall -g --prefix <dir> @opencoven/cli` |
| `cargo install` | It lives in `$CARGO_HOME/bin` (default `~/.cargo/bin`). | `cargo uninstall coven-cli` |
| `source build` | It resolves into a `target/debug` or `target/release` directory. | Delete the link or file; rebuilding recreates it. |
| `legacy coven-code installer` | It lives in `~/.coven-code/bin`, where older standalone engine installers wrote a `coven` alias. | Delete the file. |
| `unknown origin` | None of the above. | Find how it was installed and remove it with that tool. |

`this process` marks the entry that is actually running Doctor. When none
matches — for example `cargo run -p coven-cli -- doctor` from a checkout —
Doctor says so and prints the executable path instead.

The npm prefix matters because a bare `npm install -g` writes to whichever
prefix is active in the current shell (`npm prefix -g`), and Node version
managers such as nvm, fnm, and volta give every Node version its own prefix.
When installs conflict and at least one is an npm install, Doctor asks npm for
that prefix and says outright whether the copy it would overwrite is the active
one. The prefix-explicit `npm install -g --prefix <dir>` form always targets
the copy you name.

Removing a shadowed npm copy that sits in the active Node's prefix is fine:
the active copy keeps working, and the next `npm install -g` recreates a copy
in that prefix only if you run it without `--prefix`.

The JSON report keeps this check path-free (`install:conflicts` reports counts
and origin kinds only), so bug reports and CI logs do not carry account or
project directory names. Run the prose form for the paths and commands.

## Expected first-run loop

```sh
coven --version
coven doctor
coven setup codex
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "explain this repo in 5 bullets"
```

If you use Claude Code instead:

```sh
coven run claude "explain this repo in 5 bullets"
```

## Missing harness output

When no supported harness is visible, Doctor points each built-in harness to
the guided setup path:

```sh
coven setup codex
coven setup claude
coven setup copilot
coven doctor
```

Setup prints official install guidance when the selected executable is missing.
When present, those commands hand the terminal to `codex login`,
`claude auth login`, or `copilot login` only after explicit consent.

If you installed a harness in another shell, open a new terminal and run
`coven doctor` again. Coven can only launch CLIs that are visible from the
environment where the daemon/session starts.

## Daemon status

`doctor` summarizes daemon state, but use the daemon command for scriptable
status:

```sh
coven daemon status --json
```

Typical Unix-like human output from `coven daemon status`:

```text
Coven daemon: running (pid 12345, socket /path/to/coven-home/coven.sock)
```

On Windows, `socket` is diagnostic pipe metadata. Clients needing a connection
path use `state.daemon_ipc` from `coven config paths --json`.

`not running` means no background daemon is running yet. Start it with:

```sh
coven daemon start
```

`stale` means metadata exists for a process/socket that no longer looks
healthy. Try:

```sh
coven daemon stop
coven daemon start
```

## Exit behavior

`coven doctor` exits `0` when local structural prerequisites are ready, so
scripts can gate on them (`coven doctor && …`). Provider access is deliberately
outside that claim and still requires an explicitly authorized test turn. The
command exits `1` when it finds a blocking local problem:

- no supported harness is available on `PATH`
- the daemon is stale (`running` and `stopped` are both healthy states)
- a registered repo entry points at a missing or non-git path
- `coven-code` is missing
- the installed `coven-code` version is older than the supported minimum

Each missing harness prints an advisory `[--]` line with an install hint. When
none is available, Doctor adds a blocking `[!!] No supported harness is
available` line and exits 1; one working harness keeps the aggregate usable.
Executable discovery does not prove provider authentication. A harness's own
`coven setup` can configure local authentication, while only its separately
consented `--verify` or `--verify-only` turn verifies provider access.

`coven adapter doctor` is stricter about its own subject: it exits `1` if any
listed adapter is unavailable. `coven wt --doctor` exits `1` when managed hooks
are missing or a worktree sits outside the protocol layout.

## Related

- [`coven setup`](/reference/cli-setup)
- [Provider auth boundary](/harnesses/provider-auth)
