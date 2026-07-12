---
title: "Coven runtime architecture"
summary: "How Coven's Rust daemon, CLI, TUI, comux cockpit, and OpenClaw plugin compose around the local socket API, PTY adapters, and the event store."
read_when:
  - Understanding Coven's runtime topology
  - Designing a client around the local socket API
description: "Coven runtime topology: the Rust daemon, CLI, TUI, comux, and OpenClaw composed around the local socket API, PTY adapters, and the event store."
---

# Coven Architecture

Coven is a local-first harness substrate. The Rust CLI/daemon is the authority layer; clients such as the CLI TUI, comux, and the optional OpenClaw plugin are presentation/integration layers.

The versioned local socket API contract lives in [`docs/API-CONTRACT.md`](API-CONTRACT.md). Clients should use `GET /api/v1/health` and its `apiVersion` / `supportedApiVersions` fields as the handshake before depending on session or event response shapes.

## Runtime topology

```mermaid
flowchart LR
  subgraph Clients["Client layer"]
    User[Developer]
    CLI["coven CLI / TUI"]
    Comux["comux cockpit"]
    OpenClaw[OpenClaw]
    Plugin["OpenClaw bridge plugin"]
  end

  subgraph DaemonCore["Daemon core"]
    Daemon[Coven daemon]
    Control["Control plane\n(capability discovery + action routing)"]
    Policy["Policy + permission hints"]
    AdapterBus["Adapter / event bus"]
    Boundary["Project-root + cwd guard"]
    HarnessRouter["Harness adapter router"]
  end

  subgraph Adapters["Harness adapters"]
    Codex["Codex PTY"]
    Claude["Claude Code PTY"]
    DesktopUse["desktop-use adapters"]
    Future["Hermes / Aider / Gemini\n(future adapters)"]
  end

  subgraph Storage["Persistent storage"]
    Store[(SQLite session ledger)]
    Events[(append-only event log)]
  end

  User --> CLI
  CLI -->|direct commands| Rust["Coven Rust CLI"]
  Rust --> Daemon

  Comux -->|"HTTP over Unix socket"| Daemon
  OpenClaw --> Plugin
  Plugin -->|"HTTP over Unix socket"| Daemon

  Daemon --> Control
  Control --> Policy
  Control --> AdapterBus
  AdapterBus -.->|desktop.automation| DesktopUse

  Daemon --> Boundary
  Boundary --> HarnessRouter
  HarnessRouter --> Codex
  HarnessRouter --> Claude
  HarnessRouter -.->|future| Future

  Daemon --> Store
  Daemon --> Events
  Codex --> Events
  Claude --> Events
```

## Session lifecycle

```mermaid
sequenceDiagram
  participant U as User
  participant C as coven CLI/TUI
  participant D as Rust daemon
  participant S as SQLite store
  participant H as Harness PTY

  U->>C: coven run codex "fix tests"
  activate C
  C->>D: POST /api/v1/sessions (projectRoot, cwd, harness, prompt)
  activate D
  D->>D: canonicalize projectRoot + cwd
  D->>D: reject outside-root or unsupported harness
  D->>S: create session metadata
  D->>H: spawn validated argv in PTY
  activate H
  H-->>S: output / exit events
  D-->>C: session id + running status
  deactivate D
  deactivate C

  Note over U,C: Browse and manage sessions

  U->>C: coven sessions
  activate C
  C->>S: list active sessions, or all with --all flag
  C-->>U: interactive session browser
  deactivate C

  U->>C: Rejoin / View Log / Summon / Archive / Sacrifice
  activate C
  C->>D: attach / input / kill (when session is live)
  C->>S: archive / summon / sacrifice (non-live session rituals)
  deactivate C
  deactivate H
```

## Authority boundary

```mermaid
flowchart TD
  Client["CLI / TUI / comux / OpenClaw plugin"]
  Request["Launch / input / kill / list request"]
  Rust["Rank 0 authority: Rust daemon"]
  RootCheck{"projectRoot\nexplicit?"}
  CwdCheck{"cwd canonicalized\ninside root?"}
  HarnessCheck{"harness\nallowlisted?"}
  RejectRoot["❌ Reject"]
  RejectCwd["❌ Reject"]
  RejectHarness["❌ Reject with install hint"]
  Spawn["Spawn harness with argv APIs"]
  Ledger["Persist session + events"]

  Client --> Request
  Request --> Rust
  Rust --> RootCheck
  RootCheck -->|no| RejectRoot
  RootCheck -->|yes| CwdCheck
  CwdCheck -->|no| RejectCwd
  CwdCheck -->|yes| HarnessCheck
  HarnessCheck -->|no| RejectHarness
  HarnessCheck -->|yes| Spawn
  Spawn --> Ledger

  style RejectRoot  fill:#fca5a5,stroke:#dc2626,color:#000
  style RejectCwd   fill:#fca5a5,stroke:#dc2626,color:#000
  style RejectHarness fill:#fca5a5,stroke:#dc2626,color:#000
  style Spawn       fill:#86efac,stroke:#16a34a,color:#000
  style Ledger      fill:#86efac,stroke:#16a34a,color:#000
```

## Intake / automation boundary

The chat/intake client should remain a chat UI, local echo/optimistic rendering surface, intent-capture layer, and tiny fast-path host for ultra-simple local actions. It should not become the automation engine.

Coven is the canonical shared local runtime for reusable automation because it centralizes:

- daemon/process ownership
- policy and permission decisions
- config/profile storage
- capability discovery
- action routing and event emission
- adapter ownership for Accessibility, AppleScript, keyboard/mouse, window, filesystem, clipboard, and app-specific bridges

The intended flow is:

```text
user -> chat/intake client -> Coven -> adapters -> desktop/apps
desktop/apps -> Coven -> chat/intake client UI updates
```

`GET /api/v1/capabilities` lets the chat/intake client and other clients discover what Coven can route. `POST /api/v1/actions` gives clients a stable intent envelope without coupling them directly to brittle OS automation APIs.

## Current user-facing surface

- `coven` and `coven tui` open the beginner-friendly slash-command palette.
- `coven doctor` checks store/project/harness readiness and prints next steps.
- `coven daemon start/status/restart/stop` manages the local daemon.
- `coven run codex|claude <prompt>` launches a project-scoped PTY session.
- `coven sessions` opens the human session browser in a terminal; `--plain` keeps scriptable output.
- Session browser actions surface readable choices: **Rejoin**, **View Log**, **Summon**, **Archive**, and **Sacrifice**.
- `coven attach|summon|archive|sacrifice <session-id>` remain explicit lower-level verbs for scripts and copy/paste workflows.

## Managed engine

Coven drives `coven-code` as a separately-installed engine process — it is
never linked or imported as a library. The process boundary is also the license
boundary (coven: MIT; coven-code: GPL-3.0). The exact CLI flags, environment
variables, stream-json events, and exit codes that constitute the integration
surface are specified in [`docs/ENGINE-CONTRACT.md`](ENGINE-CONTRACT.md);
any breaking change to that surface requires a coordinated version bump in both
repositories.

## Distribution snapshot

The npm wrapper packages are live for early adopters:

- `@opencoven/cli`
- `@opencoven/cli-macos`
- `@opencoven/cli-linux-x64`
- `@opencoven/cli-windows` once the next Windows-enabled release is published

The source package versions stay template-like in the repo; release workflow dispatch supplies the published version and builds platform packages. As of the current documentation pass, npm latest is `0.0.10` for the wrapper plus macOS/Linux packages; Windows x64 release wiring is staged for the next package release.
