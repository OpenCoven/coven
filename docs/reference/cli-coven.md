---
summary: "The default interactive command."
read_when:
  - Looking up coven
title: "coven"
description: "Reference for the top-level coven command: subcommands, global flags, and the interactive menu that opens when you run coven with no arguments."
---

# `coven`

`coven` is the interactive entry point for the local Coven runtime. With no
arguments it opens the interactive Coven UI; with a free-text task it prepares
that task for a recorded, project-scoped session. Use an explicit subcommand
when you need a scriptable or repeatable operation.

## Usage

```sh
# Open the interactive Coven UI.
coven
coven chat
coven tui

# Describe work in plain language; Coven presents a plan before running it.
coven "summarize this repository in five bullets"

# Discover the full command inventory and command-specific help.
coven --help
coven run --help
```

`chat` and `tui` are explicit aliases for the no-argument interactive route.
The UI is powered by the managed Coven engine. The engine is installed or
updated through `coven engine` rather than by treating it as a library inside
the Rust CLI.

## Global option

```sh
coven --color never doctor
```

`--color auto` is the default and honors `NO_COLOR` and `CLICOLOR_FORCE`.
Choose `always` or `never` only when a terminal or a captured output path needs
an explicit ANSI-color policy.

## Choose a path

| Goal | Start with |
| --- | --- |
| Check whether the local runtime is usable | `coven doctor` |
| Control the local daemon | `coven daemon status` |
| Launch an explicit harness task | `coven run <harness> "<prompt>"` |
| Browse or manage history | `coven sessions` |
| Automate a readiness or status check | `coven doctor --json` |
| Find an end-to-end example | [Workflow guides](/guides) |

The top-level command is not a shortcut around project or permission policy.
Launches still validate project roots and working directories in the Rust
authority layer, and harness credentials remain owned by the selected harness.

## Related

- [CLI reference](/reference/cli)
- [Core access guide](/guides/core-access)
- [Coven TUI](/start/coven-tui)
- [Engine contract](/ENGINE-CONTRACT)
