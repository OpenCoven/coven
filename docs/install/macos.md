---
summary: "Install Coven on macOS via npm, Homebrew, or source."
read_when:
  - Installing on macOS
title: "macOS install"
description: "Install Coven on macOS: install the @opencoven/cli wrapper, place the daemon binary on PATH, and supervise the daemon with launchd."
---

# macOS install

Use the npm wrapper on macOS unless you are developing Coven itself.

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

The universal wrapper selects the native macOS package for Apple Silicon. On other macOS hosts, use the source install path unless the release matrix adds a matching native package.

## Harness setup

Install and authenticate at least one harness CLI from the same shell where you run Coven:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Then verify:

```sh
coven doctor
```

If `doctor` reports a missing harness, open a new terminal and check the command directly:

```sh
command -v codex
command -v claude
```

## First session

```sh
cd /path/to/project
coven daemon start
coven daemon status
coven run codex "describe this repo"
coven sessions
```

Use `coven run claude "describe this repo"` when Claude Code is the configured harness.

## COVEN_HOME

The default state directory is:

```sh
$HOME/.coven
```

To isolate state for a demo, test account, or project:

```sh
export COVEN_HOME="$HOME/.coven-demo"
coven doctor
coven daemon start
```

Keep `COVEN_HOME` on a local disk owned by your user. See [COVEN_HOME layout](/daemon/coven-home).

## Source install for contributors

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

Use [Install from source](/install/from-source) for the full contributor path.

## Related

- [Install via npm](/install/npm)
- [Updating Coven](/install/updating)
- [Troubleshooting](/TROUBLESHOOTING)
