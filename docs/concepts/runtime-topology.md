---
summary: "How the daemon, harnesses, store, and clients fit together."
read_when:
  - Understanding which Coven component owns which responsibility
title: "Runtime topology"
description: "Runtime topology of Coven: how CastCodes, the Coven CLI/TUI, and advanced clients connect to a single Rust daemon over same-user local IPC."
---

```mermaid
flowchart LR
  User[Developer] --> CastCodes[CastCodes workspace]
  CastCodes --> Daemon[Coven daemon]
  User --> CLI[coven CLI / TUI]
  CLI --> Daemon
  Comux[comux legacy/reference] -.-> Daemon
  Plugin["OpenClaw bridge plugin"] -.-> Daemon
  Daemon --> Adapter[Adapter router]
  Adapter --> Codex[Codex PTY]
  Adapter --> Claude[Claude Code PTY]
  Daemon --> Store[(SQLite)]
  Daemon --> Events[(Event log)]
```

The daemon accepts same-user local IPC: a filesystem-permission-protected Unix
socket on Unix-like hosts or an owner-only named pipe on Windows. It does not
bind TCP by default.

See [Architecture](/concepts/architecture) for the full picture and [Authority boundary](/concepts/authority-boundary) for trust rules.
