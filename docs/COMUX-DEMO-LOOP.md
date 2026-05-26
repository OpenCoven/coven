---
title: "comux to CastCodes migration reference"
summary: "Legacy reference for the comux + Coven demo loop and the primitives being folded into CastCodes."
read_when:
  - Understanding what comux proved
  - Migrating the public demo loop to CastCodes
description: "Legacy reference for the comux + Coven demo loop. The future-facing public demo is CastCodes + Coven."
---

# comux to CastCodes migration reference

The future-facing public demo loop is **CastCodes + Coven**. See [CastCodes and Coven integration](/CASTCODES-INTEGRATION) for the current product direction.

This page remains as a migration reference for the legacy comux + Coven demo loop and for the durable primitives CastCodes should absorb.

comux proved the terminal cockpit model. Its durable primitives are being folded into CastCodes so Coven has one primary product surface.

```mermaid
flowchart LR
  subgraph Dev["Developer"]
    Open[Open repo in comux]
  end

  subgraph Local["Local machine"]
    Comux[legacy comux cockpit]
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

## Legacy loop

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

## CastCodes targets

| comux primitive | CastCodes target |
| --- | --- |
| Pane | Agent lane / terminal tab / workspace lane |
| Worktree isolation | CastCodes/Coven isolated task lane |
| Agent launcher registry | CastCodes harness picker backed by Coven/Cast Agent |
| Multi-select launch | Multi-harness CastCodes lane creation |
| Ritual | CastCodes command palette ritual/template |
| File browser/diff | Native editor diff/review surface |
| Merge/PR flow | CastCodes review, verification, PR, cleanup workflow |
| Lifecycle hooks | Coven/Cast Agent events and hooks |
| Coven bridge | Direct CastCodes/Coven integration |

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

The broader OpenCoven roadmap now tracks CastCodes as the primary public proof surface: [ROADMAP.md](/ROADMAP).
