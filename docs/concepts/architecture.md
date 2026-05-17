---
summary: "Runtime topology, session lifecycle, and authority boundary diagrams for Coven."
read_when:
  - Understanding how Coven, comux, and clients fit together
  - Auditing trust boundaries before adding a new client
title: "Architecture"
description: "Conceptual map of Coven: the Rust daemon as authority, the CLI, TUI, comux, and OpenClaw clients, and the versioned local socket API contract that joins them."
---

Coven is a local-first harness substrate. The Rust CLI/daemon is the authority layer; clients such as the CLI TUI, comux, and the optional OpenClaw plugin are presentation/integration layers.

The versioned local socket API contract lives in [API contract](/reference/api-contract). Clients should handshake with [`GET /api/v1/health`](/daemon/health) and negotiate against `apiVersion: "coven.daemon.v1"` plus the `capabilities` object before depending on session or event response shapes. All error responses use the structured `{ error: { code, message, details } }` envelope documented in [Error envelope](/daemon/error-envelope).

## Runtime topology

```mermaid
flowchart LR
  User[Developer] --> CLI[coven CLI / TUI]
  CLI -->|direct commands| Rust[Coven Rust CLI]
  Rust --> Daemon[Coven daemon]

  Comux[comux cockpit] -->|HTTP over Unix socket| Daemon
  OpenClaw[OpenClaw] --> Plugin[external @opencoven/coven plugin]
  Plugin -->|HTTP over Unix socket| Daemon
  OpenMeow[OpenMeow chat/intent client] -->|capabilities + actions| Daemon

  Daemon --> Control[Control plane: capability discovery + action routing]
  Control --> Policy[Policy + permission hints]
  Control --> AdapterBus[Adapter/event bus]
  AdapterBus -. desktop.automation .-> DesktopUse[desktop-use adapters]

  Daemon --> Boundary[Project-root + cwd guard]
  Boundary --> Adapter[Harness adapter router]
  Adapter --> Codex[Codex PTY]
  Adapter --> Claude[Claude Code PTY]
  Adapter -. future .-> Future[Hermes / Aider / Gemini / custom adapters]

  Daemon --> Store[(SQLite session ledger)]
  Daemon --> Events[(append-only event log)]
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
  C->>D: POST /api/v1/sessions(projectRoot, cwd, harness, prompt)
  D->>D: canonicalize projectRoot + cwd
  D->>D: reject outside-root or unsupported harness
  D->>S: create session metadata
  D->>H: spawn validated argv in PTY
  H-->>S: output / exit events
  D-->>C: session id + running status

  U->>C: coven sessions
  C->>S: list active sessions, or all with --all
  C-->>U: interactive session browser

  U->>C: Rejoin / View Log / Summon / Archive / Sacrifice
  C->>D: attach/input/kill when live
  C->>S: archive/summon/sacrifice non-live session rituals
```

## Authority boundary

```mermaid
flowchart TD
  Client[CLI, TUI, comux, OpenClaw plugin] --> Request[Launch / input / kill / list request]
  Request --> Rust[Rank 0 authority: Rust daemon]
  Rust --> RootCheck{projectRoot explicit?}
  RootCheck -- no --> RejectRoot[Reject]
  RootCheck -- yes --> CwdCheck{cwd canonicalized inside root?}
  CwdCheck -- no --> RejectCwd[Reject]
  CwdCheck -- yes --> HarnessCheck{harness allowlisted?}
  HarnessCheck -- no --> RejectHarness[Reject with install hint]
  HarnessCheck -- yes --> Spawn[Spawn harness with argv APIs]
  Spawn --> Ledger[Persist session + events]
```

## OpenMeow / automation boundary

OpenMeow should remain a chat UI, local echo/optimistic rendering surface, intent-capture layer, and tiny fast-path host for ultra-simple local actions. It should not become the automation engine.

Coven is the canonical shared local runtime for reusable automation because it centralizes:

- daemon/process ownership;
- policy and permission decisions;
- config/profile storage;
- capability discovery;
- action routing and event emission;
- adapter ownership for Accessibility, AppleScript, keyboard/mouse, window, filesystem, clipboard, and app-specific bridges.

The intended flow is:

```text
user -> OpenMeow -> Coven -> adapters -> desktop/apps
desktop/apps -> Coven -> OpenMeow UI updates
```

`GET /api/v1/capabilities` lets OpenMeow and other clients discover what Coven can route. `POST /api/v1/actions` gives clients a stable intent envelope without coupling them directly to brittle OS automation APIs.

## Future: multi-harness orchestration (Phase 1-4)

Coven v0 is single-harness per session. Future phases will add multi-harness orchestration:

<Columns>
  <Card title="Phase 1 — Handoff" href="/familiars/handoff" icon="git-branch">
    Explicit transfer of task + full context between harnesses. Adds `POST /api/v1/handoff`.
  </Card>
  <Card title="Phase 2 — Routing" href="/familiars/orchestration" icon="route">
    Capability discovery, router, load balancing. Adds `POST /api/v1/task/execute`.
  </Card>
  <Card title="Phase 3 — Affinity" icon="anchor">
    Multi-instance, distributed context, affinity constraints. Adds health heartbeat + node registration.
  </Card>
  <Card title="Phase 4 — Audit" icon="scroll">
    Handoff ledger queries and metrics for compliance and observability.
  </Card>
</Columns>

The Rust daemon stays the authority boundary throughout. Orchestration logic can live above the daemon, delegating safety-critical decisions (process spawning, cwd validation, capability checking) to Coven.

## Related

- [Runtime topology](/concepts/runtime-topology)
- [Authority boundary](/concepts/authority-boundary)
- [Control plane](/concepts/control-plane)
- [Session lifecycle](/sessions/lifecycle)
- [Safety model](/daemon/safety-model)
