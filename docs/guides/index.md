---
title: "Coven workflow guides"
summary: "Task-oriented guides for reaching and using Coven's local CLI, daemon, sessions, automation output, and parallel-work protocol."
read_when:
  - Looking for a runnable Coven workflow
  - Choosing between the command reference and an end-to-end example
description: "Runnable, task-oriented Coven guides for core access, session operations, automation, parallel work, and troubleshooting."
---

# Coven workflow guides

Use these guides when you want a verified task flow rather than a list of flags. The [CLI reference](/reference/cli) remains the complete command and option surface; each guide links back to the precise reference page for edge cases.

| Guide | Use it for | Starts with |
| --- | --- | --- |
| [Core access](/guides/core-access) | Installing or validating a usable local Coven path | `coven doctor` |
| [Session operations](/guides/session-operations) | Listing, replaying, archiving, restoring, or deleting sessions | `coven sessions --plain` |
| [Automation and JSON](/guides/automation-json) | Shell scripts and local clients that need stable machine-readable output | `coven doctor --json` |
| [Multi-agent worktrees](/guides/multi-agent-worktrees) | Isolated branches, shared claims, and managed hooks | `coven wt` |
| [Troubleshooting core access](/guides/troubleshooting-core-access) | Missing harnesses, daemon/socket state, and project-boundary failures | `coven doctor` |

## Choose the right document

- Need every accepted flag or subcommand? Read the [CLI reference](/reference/cli).
- Need the implementation and ownership boundary? Read the [developer core-functionality guide](/development/cli-core-functionality).
- Need to build a local client? Start with [the API contract](/API-CONTRACT) after the [automation and JSON guide](/guides/automation-json).
- Need a supported harness? Use [the harness setup guides](/harnesses).

## Related

- [Getting started](/GETTING-STARTED)
- [CLI reference](/reference/cli)
- [Developer core-functionality guide](/development/cli-core-functionality)
