---
title: "comux and Coven demo loop"
description: "The Coven-side contract for making Codex and Claude Code sessions discoverable as visible comux panes through the local socket API."
---

# comux + Coven demo loop

This is the Coven-side contract for making Coven-managed Codex and Claude Code sessions visible in comux.

```mermaid
flowchart LR
  subgraph Dev["Developer"]
    Open[Open repo in comux]
  end

  subgraph Local["Local machine"]
    Comux[comux cockpit]
    CLI[coven CLI]
    Daemon[Coven daemon]
    PTY1[Codex PTY]
    PTY2[Claude PTY]
    Store[(SQLite store + events)]
  end

  Open --> Comux
  Comux -->|discover| CLI
  CLI -->|coven sessions --json| Daemon
  Daemon --> Store
  Daemon --> PTY1
  Daemon --> PTY2
  Comux -->|open / attach| Daemon
  Daemon -->|/events| Comux
  Comux --> Review[Inspect · Diff · Merge · PR]
  Review --> Ritual[Archive · Summon · Sacrifice]
  Ritual --> Daemon
```

The demo loop is end-to-end: comux never bypasses the daemon, and the daemon never trusts comux for project-root, harness, or destructive-deletion enforcement.

## Loop

1. Open the target repository in comux.
2. Start Coven if needed:

   ```sh
   coven daemon start
   ```

3. Launch a Coven-backed session from the same repository:

   ```sh
   coven run codex "fix the failing tests"
   coven run claude "review the diff"
   ```

4. Let comux discover sessions through either supported client path:
   - `coven sessions --json` for simple local CLI discovery.
   - `GET /api/v1/sessions` after `GET /api/v1/health` for daemon clients.
5. Open the session as a visible comux pane, or attach manually:

   ```sh
   coven attach <session-id>
   ```

6. Inspect files, diffs, and session output from comux.
7. Merge, create a PR, archive, summon, sacrifice, or clean up explicitly after verification.

## CLI discovery

`coven sessions --json` prints a stable object with a `sessions` array. Records use the same snake_case field names as the daemon API:

```json
{
  "sessions": [
    {
      "id": "session-1",
      "project_root": "/repo",
      "harness": "codex",
      "title": "Fix the tests",
      "status": "running",
      "exit_code": null,
      "archived_at": null,
      "created_at": "2026-05-14T07:00:00Z",
      "updated_at": "2026-05-14T07:00:01Z"
    }
  ]
}
```

Use `--all --json` when archived sessions should remain visible.

## Daemon discovery

Daemon clients should use the versioned socket API:

1. `GET /api/v1/health`
2. Verify `apiVersion === "coven.daemon.v1"` and `capabilities.sessions === true`.
3. `GET /api/v1/sessions`
4. Filter sessions by verified project root before showing them in a project-scoped UI.

The daemon socket defaults to `~/.coven/coven.sock`. The daemon remains the authority for project roots, cwd, harness ids, live-session checks, input, kill requests, archive state, and destructive deletion rules.

## Unavailable states

Clients should keep their core UI usable when Coven is missing or stopped:

- CLI missing: show install guidance for `@opencoven/cli`.
- Daemon stopped or socket missing: suggest `coven daemon start`.
- Harness missing: suggest `coven doctor`.
- Unsupported API version: ask the user to update Coven or the client.

## Roadmap

The broader OpenCoven roadmap remains the public tracking point for the end-to-end demo: [ROADMAP.md](/ROADMAP).

> **Image asset prompt (to be generated and dropped into `docs/images/comux-demo-loop.png`):** Render a 1920×1080 split-pane mockup that mimics a comux cockpit on the OpenCoven dark palette (`#1A1825` background, `#9A8ECD` chrome). Three tmux-like panes: (1) file explorer with a fake monorepo tree, (2) live attach view streaming a fake Codex session ("Fix the failing tests"), (3) diff/review pane with a small green/red unified diff. Bottom status bar reads `coven daemon · running · 1.2 MB events · 2 active sessions`. Use `Fragment Mono`.
