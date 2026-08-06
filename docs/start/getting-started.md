---
summary: "Install Coven, run doctor, start the daemon, and launch your first harness session."
read_when:
  - First time setting up Coven on a workstation
title: "Getting started"
description: "Install Coven, run coven doctor, and launch your first Codex, Claude Code, or GitHub Copilot CLI harness session in about five minutes from one local Rust daemon."
---

Install Coven, run `coven doctor`, and launch your first harness session in about five minutes. By the end you will have a running daemon, a project-rooted session record, and a working PTY attached to Codex, Claude Code, or GitHub Copilot CLI.

## What you need

- **Rust stable** — only if you build from source. The published
  `@opencoven/cli` wrapper includes macOS arm64/x64, glibc-based Linux x64,
  and Windows x64 packages; build from source on musl/Alpine Linux.
- **At least one harness CLI on `PATH`** — Codex, Claude Code, or GitHub Copilot CLI. `coven doctor` will report what is missing and how to install it.

<Tip>
Coven does not store provider credentials. Each harness keeps using its own local auth flow (`codex login`, `claude doctor`).
</Tip>

## Quick setup

<Steps>
  <Step title="Install Coven">
    <Tabs>
      <Tab title="npm">
        ```bash
        npm install -g @opencoven/cli
        ```
      </Tab>
      <Tab title="From source">
        ```bash
        git clone https://github.com/OpenCoven/coven
        cd coven
        cargo build --workspace --release
        ```
      </Tab>
    </Tabs>
    <Note>
    Other install methods: [Install](/install).
    </Note>
  </Step>
  <Step title="Run doctor">
    ```bash
    coven doctor
    ```
    `doctor` checks the store, project boundary, daemon/socket status, and harness readiness. Follow its hints before continuing.
  </Step>
  <Step title="Start the daemon">
    ```bash
    coven daemon start
    coven daemon status
    ```
    On Unix-like systems, the daemon binds `$COVEN_HOME/coven.sock` (default:
    `~/.coven/coven.sock`); the path in this guide is Unix-specific. On Windows,
    the same API uses a per-profile owner-only named pipe.
  </Step>
  <Step title="Launch your first session">
    ```bash
    cd /path/to/your/project
    coven run codex "describe this repo"
    ```
    Or open the human session browser:
    ```bash
    coven sessions
    ```
  </Step>
</Steps>

## What to do next

<Columns>
  <Card title="Sessions and rituals" href="/sessions/lifecycle" icon="folder-tree">
    Attach, archive, summon, sacrifice — the safe ways to manage live and finished work.
  </Card>
  <Card title="Familiars" href="/familiars" icon="sparkles">
    Name your agents, give them roles, and let them remember.
  </Card>
  <Card title="Local API" href="/daemon/socket-api" icon="plug">
    Build a client that handshakes with `GET /api/v1/health`.
  </Card>
  <Card title="Tinkerer's next 30 minutes" href="/start/tinkerers-next-30-minutes" icon="terminal">
    Probe the daemon API, JSON output, event log, and fake-harness loop.
  </Card>
</Columns>

## Related

- [Install overview](/install)
- [Doctor](/start/doctor)
- [Coven TUI](/start/coven-tui)
- [Tinkerer's next 30 minutes](/start/tinkerers-next-30-minutes)
