# Coven Architecture

Coven is a local-first harness substrate. The Rust CLI/daemon is the authority layer; clients such as the CLI TUI, comux, and the optional OpenClaw plugin are presentation/integration layers.

The versioned local socket API contract lives in [`docs/API-CONTRACT.md`](API-CONTRACT.md). Clients should use `GET /api/v1/health` and its `apiVersion` / `supportedApiVersions` fields as the handshake before depending on session or event response shapes.

## Runtime topology

```mermaid
flowchart LR
  User[Developer] --> CLI[coven CLI / TUI]
  CLI -->|direct commands| Rust[Coven Rust CLI]
  Rust --> Daemon[Coven daemon]

  Comux[comux cockpit] -->|HTTP over Unix socket| Daemon
  OpenClaw[OpenClaw] --> Plugin[external @opencoven/coven plugin]
  Plugin -->|HTTP over Unix socket| Daemon
  OpenMeow[OpenMeow intake/status] -. future status/intake .-> Daemon

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

## Current user-facing surface

- `coven` and `coven tui` open the beginner-friendly slash-command palette.
- `coven doctor` checks store/project/harness readiness and prints next steps.
- `coven daemon start/status/restart/stop` manages the local daemon.
- `coven run codex|claude <prompt>` launches a project-scoped PTY session.
- `coven sessions` opens the human session browser in a terminal; `--plain` keeps scriptable output.
- Session browser actions surface readable choices: **Rejoin**, **View Log**, **Summon**, **Archive**, and **Sacrifice**.
- `coven attach|summon|archive|sacrifice <session-id>` remain explicit lower-level verbs for scripts and copy/paste workflows.

## Distribution snapshot

The npm wrapper packages are live for early adopters:

- `@opencoven/cli`
- `@opencoven/cli-macos`
- `@opencoven/cli-linux-x64`

The source package versions stay template-like in the repo; release workflow dispatch supplies the published version and builds platform packages. As of the current documentation pass, npm latest is `0.0.10` for all three packages.
