---
title: "Coven glossary"
description: "Definitions for Coven terms: ACP, API version, archive, capability, client, comux, harness, project root, ritual, sacrifice, session, and summon."
---

# Glossary

## ACP

Agent Client Protocol. In this repo, ACP appears as an integration surface for external agent runtimes and OpenClaw compatibility. Coven itself is not an ACP implementation; the external OpenClaw plugin maps between OpenClaw runtime events and Coven sessions.

## API version

The named compatibility contract exposed by the daemon socket API. Current stable value: `coven.daemon.v1`.

## Archive

Hide a non-running session from the active list while preserving its record and events.

## Capability

A discoverable daemon or adapter feature returned by `GET /api/v1/capabilities`.

## Client

Any process or UI that talks to the Coven daemon, including the CLI, comux, OpenMeow, or the OpenClaw plugin.

## comux

The cockpit layer for visible agent work, panes, worktrees, review, and merge flow. comux can consume Coven sessions but is not the Coven runtime.

## Control plane

The daemon layer that exposes capabilities and routes known action ids to owned adapters.

## Coven

The OpenCoven local runtime substrate and command-line product.

## `coven`

The user-facing command.

## `coven pc`

macOS-first system diagnostics and relief subcommand. Reports CPU, memory, disk, and top processes. Write operations (process kill, cache clear) are gated behind `--confirm`.

## `COVEN_HOME`

The local directory where Coven stores daemon/socket/database state when configured. Runtime state should not be committed to source control.

## Daemon

The local Rust process that owns live session state and the socket API.

## Event

An append-only record for session output, exit, or metadata.

## Harness

A supported coding-agent CLI that Coven can launch and supervise.

## OpenCoven

The broader ecosystem and organization around Coven, comux, and related integrations.

## OpenClaw plugin

The external package `@opencoven/coven`, which lets OpenClaw use Coven through the socket API. It is not part of OpenClaw core.

## Project root

The explicit repository or project boundary for a session.

## PTY

Pseudoterminal. Coven uses PTYs so harnesses behave like terminal-native tools while their output can still be recorded and replayed.

## Prompt-first TUI

The default `coven` and `coven tui` interface. Accepts free-form task text or slash commands like `/run codex <task>` as input, alongside arrow-key menu navigation.

## Relief

Write-side operations in `coven pc` that mutate system state (process termination, cache deletion). Always require an explicit `--confirm` flag.

## Sacrifice

Permanently delete a non-running session and its events.

## Session

A Coven-owned record of one harness run.

## Socket API

The local HTTP-over-Unix-socket API exposed by the daemon.

## Summon

Restore an archived session to the active list and then replay/follow it.

## Future coordination

Multi-harness handoff and task routing are not current public CLI/API features. They should be documented only as roadmap work until implemented.
