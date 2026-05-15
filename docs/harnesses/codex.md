---
summary: "Run OpenAI Codex CLI under Coven supervision. Harness id `codex`."
read_when:
  - Setting up Codex for Coven
  - Diagnosing Codex-specific harness failures
title: "Codex"
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

## Related

- [Installing harness CLIs](/harnesses/installing)
- [Provider auth boundary](/harnesses/provider-auth)
- [Harness troubleshooting](/harnesses/troubleshooting)
