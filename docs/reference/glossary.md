---
summary: "Short definitions for every recurring Coven product and architecture term."
read_when:
  - Looking up a Coven term
  - Onboarding a contributor or stakeholder
title: "Glossary"
description: "Glossary of Coven and OpenCoven terms: CastCodes, ACP, coven.daemon.v1, harness, adapter, familiar, ritual, project root, archive, sacrifice, and capability."
---

> **See also:** the repo-root [Coven glossary](/GLOSSARY) (`docs/GLOSSARY.md`) is the fuller canonical term list; this reference page is a condensed view that adds future orchestration terms.

## ACP

Agent Client Protocol. In this repo, ACP appears as an integration surface for external agent runtimes and OpenClaw compatibility. Coven itself is not an ACP implementation; the external OpenClaw plugin maps between OpenClaw runtime events and Coven sessions.

## API version

The compatibility version exposed by the daemon socket API. Current stable value: `coven.daemon.v1`.

## Adapter

The PTY-facing component that maps the daemon's harness contract onto a specific CLI (Codex, Claude Code, future Hermes/Aider/Gemini). See [Harness adapters](/reference/harness-adapters).

## Affinity (future)

A constraint on task routing. Example: "use OpenClaw's Cody agent", "requires GPU access". Phase 3 of orchestration.

## Archive

Hide a non-running session from the active list while preserving its record and events.

## Capability

A discoverable daemon or adapter feature returned by `GET /api/v1/capabilities`.

## CastCodes

The local-first AI coding workspace powered by Coven. CastCodes is the primary public proof surface.

## Client

Any process or UI that talks to the Coven daemon. CastCodes is the primary public client; the CLI, comux, and the OpenClaw plugin are operator, legacy, or advanced client shapes.

## comux

The legacy/reference cockpit layer for visible agent work, panes, worktrees, review, and merge flow. comux proved primitives that are being folded into CastCodes; it is not the Coven runtime or the future-facing flagship surface.

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

## Familiar

OpenCoven's product concept for a persistent named agent — name, voice, memory, tools, identity, role. A familiar can swap harnesses without losing identity. See [Familiars](/familiars).

## Handoff (future)

Explicit transfer of a task plus full context from one harness to another. Phase 1 of orchestration.

## Harness

A supported coding-agent CLI that Coven can launch and supervise.

## Harness capability (future)

A declared skill of a harness, used by the router for task matching. Example: `"code_fix"`, `"testing"`, `"research"`. Phase 2 of orchestration.

## OpenClaw plugin

The external package external OpenClaw bridge plugin, which lets OpenClaw use Coven through the socket API. It is not part of OpenClaw core.

## OpenCoven

The broader organization and lab around CastCodes, Coven, and related integrations.

## Project root

The explicit repository or project boundary for a session.

## PTY

Pseudoterminal. Coven uses PTYs so harnesses behave like terminal-native tools while their output can still be recorded and replayed.

## Prompt-first TUI

The default `coven` and `coven tui` interface. Accepts free-form task text or slash commands like `/run codex <task>` as input, alongside arrow-key menu navigation.

## Relief

Write-side operations in `coven pc` that mutate system state (process termination, cache deletion). Always require an explicit `--confirm` flag.

## Ritual

Coven's human-friendly verb for a session-state operation: archive, summon, sacrifice. See [Rituals](/rituals).

## Router (future)

Orchestration component that automatically selects the best-fit harness for a task based on capability, availability, and load. Phase 2 of orchestration.

## Sacrifice

Permanently delete a non-running session and its events. Requires `--yes`.

## Session

A Coven-owned record of one harness run.

## Socket API

The local HTTP-over-Unix-socket API exposed by the daemon.

## Store

Coven's local SQLite database. Contains session metadata and append-only event history.

## Summon

Restore an archived session to the active list and then replay/follow it.
