---
summary: "What Coven can do today — harnesses, sessions, rituals, capabilities, and the local API."
read_when:
  - Comparing Coven's surface against another runtime
title: "Features"
description: "Feature reference for the Coven runtime: project-rooted launches, harness-neutral PTYs, append-only events, rituals, capability discovery, and action routing."
---

<Columns>
  <Card title="Project-rooted launches" icon="folder-tree">
    Every session pins a canonical project root. Cwd must canonicalize inside that root.
  </Card>
  <Card title="Harness-neutral PTYs" icon="terminal">
    Bundled: Codex, Claude Code, GitHub Copilot CLI. Trusted recipes: Hermes 1.0.3, OpenCode 0.1.1; experimental: Grok Build 1.0.0. Future: Aider, Gemini, Cline, custom.
  </Card>
  <Card title="Append-only event log" icon="scroll">
    Output, exit, and metadata events stored in SQLite for replay.
  </Card>
  <Card title="Rituals" icon="moon">
    Archive, summon, sacrifice — explicit, beginner-safe verbs around destructive operations.
  </Card>
  <Card title="Same-user local IPC API" icon="plug">
    Versioned HTTP-over-same-user-local-IPC contract under `/api/v1`.
  </Card>
  <Card title="Control plane" icon="compass">
    Capability discovery + action routing for CastCodes and advanced clients.
  </Card>
</Columns>
