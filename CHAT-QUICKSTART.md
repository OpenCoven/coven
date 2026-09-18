# Coven Interactive UI — Quick Start

`coven` (and its explicit forms `coven chat` / `coven tui`) opens the
interactive Coven UI. The UI is provided by the separate
[`coven-code`](https://github.com/OpenCoven/coven-code) engine, which `coven`
installs and manages for you and hands the terminal over to.

## Install

```bash
# The Coven CLI
npm install -g @opencoven/cli

# The interactive engine, pinned to the version this CLI was released with
coven engine install
```

`coven engine install` downloads the pinned `coven-code` release into
`~/.coven/engine/`, verifies its checksum, and is also how you upgrade it
later. `coven doctor` reports the engine it will run.

### Where `coven` looks for the engine

In order, the first match wins:

1. `COVEN_ENGINE_BIN`, when set to an executable
2. the managed install under `~/.coven/engine/` (what `coven engine install` writes)
3. a `coven-code` on `PATH`
4. `~/.coven-code/bin/`, left by the older standalone installer

Because the managed install wins, a `coven-code` you installed separately —
`npm install -g @opencoven/coven-code`, or the standalone `install.sh` /
`install.ps1` from the coven-code releases — is **not** what `coven` runs once
an engine is managed. `coven-code --version` in your shell will then describe a
different binary than `coven --version` reports. Keep one: use
`coven engine install` and remove the separate copy, or leave the separate copy
as your only engine and never run `coven engine install`. `coven doctor` warns
when both are present.

## Run

```bash
cd /path/to/your/project
coven
```

If no engine is installed, `coven` prints `coven engine install` instead of
opening the UI.

## Prefer plain commands?

Everything the UI does is also available as explicit CLI commands:

```bash
coven doctor                      # check your setup
coven status                      # daemon, sessions, familiars, skills, hub at a glance
coven run codex "fix the tests"   # launch a recorded session
coven sessions                    # browse sessions (plain table when piped)
coven attach <session-id-prefix>  # follow a session
```

You can also hand a task straight to Cast — Coven shows a plan card, then runs
it in a recorded session:

```bash
coven "explain this repo in 5 bullets"
```

## Legacy in-process shell

The previous built-in slash shell is deprecated and will be removed. If you
need it during the transition:

```bash
COVEN_LEGACY_TUI=1 coven
```
