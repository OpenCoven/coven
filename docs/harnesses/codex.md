---
summary: "Run OpenAI Codex CLI under Coven supervision. Harness id `codex`."
read_when:
  - Setting up Codex for Coven
  - Diagnosing Codex-specific harness failures
title: "Codex harness"
description: "Run the OpenAI Codex CLI under Coven supervision with harness id codex, a project-rooted PTY, and the usual session, attach, and ritual flows."
---


Codex is OpenAI's coding-agent CLI. Coven wraps it in a project-rooted PTY so launches, attaches, and rituals work the same as for any other harness.

| Field | Value |
|---|---|
| Harness id | `codex` |
| Install | `npm install -g @openai/codex` |
| Auth | `codex login` (one-time, OpenAI side) |
| Doctor check | `coven doctor` reports the resolved Codex path and version. |

## Setup

<Steps>
  <Step title="Install Codex">
    ```bash
    npm install -g @openai/codex
    ```
    Other install methods (Homebrew cask, package managers) are listed at the [Codex repo](https://github.com/openai/codex).
  </Step>
  <Step title="Log in to OpenAI">
    ```bash
    codex login
    ```
    Provider credentials stay with Codex. Coven never reads them.
  </Step>
  <Step title="Confirm with Coven">
    ```bash
    coven doctor
    ```
    The output should include a line like `codex: ok (/usr/local/bin/codex)`.
  </Step>
  <Step title="Launch">
    ```bash
    coven run codex "fix the failing tests"
    ```
  </Step>
</Steps>

## Per-session flags

```bash
coven run codex "audit this repo" --cwd packages/cli --title "CLI audit"
```

- `--cwd` — canonicalized inside the project root.
- `--title` — sets a readable title in the session browser.
- `--json` — print structured launch metadata for clients.

## Provider auth boundary

Codex owns its own OAuth flow and token cache. If you see `Invalidated OAuth token`, run `codex login` again. Coven will keep the existing session record so you can re-launch with the same title.

For the local rescue path:

```bash
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
```

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `coven doctor` reports `codex` missing | Codex not on `PATH` | `npm install -g @openai/codex`, then re-run doctor. |
| Codex prompts for login each run | Stale token | `codex login`. |
| Session hangs at start | Codex waiting on TTY prompt | Detach with `Ctrl-]`, re-launch with `coven run` directly. |

## How Coven supervises Codex

```mermaid
sequenceDiagram
  participant U as User
  participant C as coven CLI
  participant D as Coven daemon
  participant Cx as Codex PTY
  participant Op as OpenAI API

  U->>C: coven run codex "audit this repo"
  C->>D: POST /api/v1/sessions
  D->>D: canonicalize root + cwd
  D->>D: lookup adapter for "codex"
  D->>Cx: spawn codex (prefix: exec --skip-git-repo-check --color never)
  Cx->>Op: provider auth (uses ~/.codex credentials — Coven does not see)
  Op-->>Cx: model response stream
  Cx-->>D: stdout / exit events
  D-->>C: SessionRecord (id, status=running)
  C-->>U: print session id, switch to attach view
```

The dotted line worth noticing: Coven never connects to the OpenAI API itself. The credential path is **Codex CLI ↔ OpenAI**, with Coven only observing the PTY output.


## Related

- [Installing harness CLIs](/harnesses/installing)
- [Provider auth boundary](/harnesses/provider-auth)
- [Harness troubleshooting](/harnesses/troubleshooting)
