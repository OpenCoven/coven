---
title: "Coven runtime architecture"
summary: "How CastCodes, the Coven Rust daemon, CLI/TUI, and advanced clients compose around the local socket API, PTY adapters, and event store."
read_when:
  - Understanding the Coven runtime topology
  - Designing a client around the local socket API
description: "How CastCodes, the Coven Rust daemon, CLI/TUI, and advanced clients compose around the local socket API, PTY adapters, and event store."
---

# Coven Architecture

Coven is a local-first harness substrate. The Rust CLI/daemon is the authority layer; CastCodes is the primary public workspace and proof surface. The CLI/TUI remains an operator surface, while comux, OpenMeow, and the optional OpenClaw plugin are advanced or legacy integration layers.

The versioned local socket API contract lives in [`docs/API-CONTRACT.md`](/API-CONTRACT). Clients should use `GET /api/v1/health` and negotiate against `apiVersion: "coven.daemon.v1"` and the `capabilities` object before depending on session or event response shapes. All error responses use the structured `{ error: { code, message, details } }` envelope documented there.

## Runtime topology

```mermaid
flowchart LR
  User[Developer] --> CastCodes[CastCodes workspace]
  CastCodes -->|HTTP over Unix socket| Daemon[Coven daemon]

  User --> CLI[coven CLI / TUI]
  CLI -->|direct commands| Rust[Coven Rust CLI]
  Rust --> Daemon

  Comux[comux legacy/reference] -.->|HTTP over Unix socket| Daemon
  OpenClaw[OpenClaw] --> Plugin[external @opencoven/coven plugin]
  Plugin -.->|HTTP over Unix socket| Daemon
  OpenMeow[OpenMeow advanced intake] -.->|capabilities + actions| Daemon

  Daemon --> Control[Control plane: capability discovery + action routing]
  Control --> Policy[Policy + permission hints]
  Control --> AdapterBus[Adapter/event bus]
  AdapterBus -. desktop automation .-> DesktopUse[desktop-use adapters]

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
  Client[CastCodes, CLI, TUI, advanced clients] --> Request[Launch / input / kill / list request]
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

## Advanced intake / automation boundary

OpenMeow-like surfaces should remain chat UI, local echo/optimistic rendering, intent-capture, or tiny fast-path hosts for ultra-simple local actions. They should not become the automation engine or the primary public Coven story.

Coven is the canonical shared local runtime for reusable automation because it centralizes:

- daemon/process ownership
- policy and permission decisions
- config/profile storage
- capability discovery
- action routing and event emission
- adapter ownership for Accessibility, AppleScript, keyboard/mouse, window, filesystem, clipboard, and app-specific bridges

The intended flow is:

```text
user -> OpenMeow -> Coven -> adapters -> desktop/apps
desktop/apps -> Coven -> OpenMeow UI updates
```

`GET /api/v1/capabilities` lets CastCodes and advanced clients discover what Coven can route. `POST /api/v1/actions` gives clients a stable intent envelope without coupling them directly to brittle OS automation APIs.

## Future adapter boundary

Coven's current public runtime is single-harness per session. The daemon already keeps the right lower-level boundary for future coordination work: clients can discover capabilities, launch known harnesses, read events, and preserve project-root enforcement in Rust.

Do not document future orchestration commands as user-facing until they exist in the CLI and socket API. Future coordination layers should build above the current session/event contract without bypassing daemon validation.

---

## Current user-facing surface

- `coven` and `coven tui` open the beginner-friendly slash-command palette.
- `coven doctor` checks store/project/harness readiness and prints next steps.
- `coven daemon start/status/restart/stop` manages the local daemon.
- `coven run codex|claude <prompt>` launches a project-scoped PTY session.
- `coven sessions` opens the human session browser in a terminal; `--plain` keeps scriptable output.
- Session browser actions surface readable choices: **Rejoin**, **View Log**, **Summon**, **Archive**, and **Sacrifice**.
- `coven attach|summon|archive|sacrifice <session-id>` remain explicit lower-level verbs for scripts and copy/paste workflows.

## Distribution snapshot

The npm wrapper packages are published for early adopters:

- `@opencoven/cli`
- `@opencoven/cli-macos`
- `@opencoven/cli-linux-x64`
- `@opencoven/cli-windows` for Windows x64

The source package versions stay template-like in the repo; release workflow dispatch supplies the published version and builds platform packages. Check the npm registry and GitHub releases before making version-specific release claims.
