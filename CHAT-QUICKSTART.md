# Coven Interactive UI — Quick Start

`coven` (and its explicit forms `coven chat` / `coven tui`) opens the
interactive Coven UI. The UI is provided by the separate
[`coven-code`](https://github.com/OpenCoven/coven-code) front-end; the `coven`
binary finds it on `PATH` or under `~/.coven-code/bin` and hands the terminal
over to it.

## Install

```bash
# The Coven CLI
npm install -g @opencoven/cli

# The interactive front-end
npm install -g @opencoven/coven-code
# or:
curl -fsSL https://github.com/OpenCoven/coven-code/releases/latest/download/install.sh | bash
```

## Run

```bash
cd /path/to/your/project
coven
```

If `coven-code` is not installed, `coven` prints the install commands above
instead of opening the UI.

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
