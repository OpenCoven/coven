# Coven Product Spec

## Product thesis

Coven is a Rust-first harness substrate for running coding agents as project-scoped, observable, attachable sessions. It lets developers bring the harnesses they already trust into a controlled local runtime instead of forcing one agent provider or UI.

North star: **One project. Any harness. Visible work.**

## MVP scope

The MVP proves the core runtime loop:

- A standalone CLI binary named `coven`
- A local daemon for supervised sessions
- Explicit project-root boundaries
- Interactive PTY session execution
- Session metadata and event persistence
- Commands for running, listing, attaching to, and killing sessions
- A minimal local API for first-party clients
- Private distribution and documentation for early testers

Out of scope for MVP: public launch, marketplace plugins, cloud sync, multi-user collaboration, a full comux rewrite, or replacing OpenClaw.

## Built-in v0 harness direction

Coven v0 should ship with built-in adapters for Codex and Claude Code. These adapters should detect local CLI availability, construct commands without shell interpolation where possible, run the harness inside a validated project `cwd`, and expose output/input through Coven-managed PTY sessions.

Terminal UX should stay centered on the lightweight `coven` command:

```sh
coven run codex "fix tests"
coven run claude "polish this UI"
```

## Future Hermes and adapter path

Hermes and other harnesses should arrive through a small adapter contract after the built-in v0 path is stable. The adapter model should support future targets such as Hermes, Aider, Gemini, OpenCode, OpenClaw, and custom command adapters without requiring Coven to become a full plugin marketplace in the MVP.

## Relationship to comux, OpenClaw, and OpenMeow

Coven is the local runtime substrate. comux can become the visual cockpit for Coven-managed panes and session history. OpenClaw can delegate project-scoped harness launches to Coven instead of spawning harnesses directly. OpenMeow can consume Coven session status, intake, or notifications where useful.

Coven should integrate with these projects without being owned by any one of them: it is the shared room where harnesses run, not the entire UI or orchestrator.

## Private-first status

Coven starts private while the safety model, daemon behavior, adapter contracts, and user experience mature. Public packaging and launch should wait until private testers can reliably run Codex and Claude Code in visible, attachable, project-scoped sessions.

## Canonical community handles

Use these exact public handles/links when Coven docs or package metadata mention community channels:

- Discord: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`

