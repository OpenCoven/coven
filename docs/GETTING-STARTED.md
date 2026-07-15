---
title: "Get started with Coven"
summary: "Install Coven, launch your first project-scoped Codex or Claude Code session, and verify the daemon, store, and harnesses with coven doctor."
read_when:
  - Installing Coven for the first time
  - Verifying a local Coven daemon and harness setup
description: "Install Coven, launch your first project-scoped Codex or Claude Code session, and verify the daemon, store, and harnesses with coven doctor."
---

# Getting started with Coven

This guide takes a new user from a fresh checkout or npm install to a visible project-scoped agent session.

## What Coven is

Coven is a local-first runtime for coding-agent harnesses. It runs supported CLIs such as Codex, Claude Code, and GitHub Copilot CLI inside explicit project boundaries, records session metadata and events, and exposes the work through a CLI, TUI, and local socket API.

The short promise:

> One project. Any harness. Visible work.

## Install paths

Use the npm wrapper for a one-off setup check:

```sh
npx @opencoven/cli doctor
pnpm dlx @opencoven/cli doctor
```

Install the wrapper when you want to keep using the `coven` command:

```sh
npm install -g @opencoven/cli
coven doctor
```

Build from source when you are contributing to Coven:

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

## Prerequisites

Coven needs:

- Rust stable, if building from source.
- Git.
- A Unix-like local runtime for the current daemon socket and PTY path.
- At least one supported harness CLI on `PATH`.

Supported v0 harnesses:

- `codex`
- `claude`

Install and authenticate a harness before expecting `coven run` to work:

```sh
npm install -g @openai/codex
codex login

npm install -g @anthropic-ai/claude-code
claude doctor
```

## First run

From a project directory:

```sh
coven
```

The default command opens the prompt-first TUI. You can either:

- type a task directly and press Enter (e.g. `fix the failing tests` or a slash command like `/run codex fix the failing tests`);
- select a menu item with arrow keys or its single-key shortcut and press Enter;
- press `h` or type `/help` to see natural-language and slash-command examples;
- press `Ctrl+C` or `Esc` to quit.

If you prefer to run the explicit setup checks:

```sh
coven doctor
```

`coven doctor` checks:

- store readiness;
- project detection;
- daemon/socket status;
- built-in harness availability; and
- next steps for missing setup.

## Run a session

Start the daemon, then launch a harness session from a repository or project directory:

```sh
coven daemon start
coven run codex "fix the failing tests"
```

or:

```sh
coven run claude "polish the CLI help text"
```

For a more readable session list, pass a title:

```sh
coven run codex "update the docs" --title "Docs refresh"
```

Use a specific working directory only when it is inside the detected project root:

```sh
coven run codex "inspect this package" --cwd packages/cli
```

Coven rejects outside-root working directories. Clients may validate for nicer UX, but the Rust daemon is the authority.

## Browse sessions

In an interactive terminal:

```sh
coven sessions
```

This opens the session browser. You can select a session and choose contextual actions:

- **Rejoin** for live sessions.
- **View Log** for completed sessions.
- **Summon** for archived sessions.
- **Archive** for completed visible sessions.
- **Sacrifice** for permanent deletion of non-running sessions and events.

For scripts or copy/paste workflows:

```sh
coven sessions --plain
coven sessions --all --plain
coven sessions --json
coven sessions --json --all
```

To inspect one session without attaching (unique id prefixes work):

```sh
coven sessions show <session-id>
coven sessions events <session-id> --limit 100
coven sessions log <session-id>
```

To see the whole coven at a glance — daemon, open sessions, familiars,
skills, and hub — run:

```sh
coven status
```

## Attach, archive, summon, and sacrifice

Lower-level session verbs remain available:

```sh
coven attach <session-id>
coven archive <session-id>
coven summon <session-id>
coven sacrifice <session-id> --yes
```

Archive is reversible. Summon restores an archived session to the active list. Sacrifice is destructive and refuses live sessions.

## Stop the daemon

```sh
coven daemon stop
```

Use `restart` when the socket or daemon state looks stale:

```sh
coven daemon restart
```

## End-to-end flow

```mermaid
flowchart LR
  Install["Install\nnpm install -g @opencoven/cli"] --> Doctor["coven doctor"]
  Doctor --> Daemon["coven daemon start"]
  Daemon --> Run["coven run codex prompt"]
  Run --> Sessions["coven sessions\n(rejoin / view log / archive)"]
  Sessions --> Sacrifice["coven sacrifice id --yes\n(when done)"]

  Doctor -. on failure .-> Harness["Install harness CLI\n+ provider login"]
  Harness --> Doctor
```

The "install → doctor → daemon → run → sessions" loop is the entire happy path for a first session. Everything else belongs in deeper guides or troubleshooting.


## Contributor verification loop

Before opening a PR:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

For daemon/session changes, also run the smoke test:

```sh
cargo test -p coven-cli --test smoke -- --nocapture
```

The smoke test uses a temporary `COVEN_HOME` and a fake harness executable. It does not require private harness credentials.
