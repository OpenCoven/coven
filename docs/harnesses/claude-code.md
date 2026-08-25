---
summary: "Run Anthropic Claude Code under Coven supervision. Harness id `claude`."
read_when:
  - Setting up Claude Code for Coven
  - Diagnosing Claude-specific harness failures
title: "Claude Code harness"
description: "Run the Anthropic Claude Code CLI under Coven supervision with harness id claude, a project-rooted PTY, and the standard attach and ritual flows."
---


Claude Code is Anthropic's coding-agent CLI. Coven wraps it in a project-rooted PTY so launches, attaches, and rituals work the same as for any other harness.

| Field | Value |
|---|---|
| Harness id | `claude` |
| Install | `npm install -g @anthropic-ai/claude-code` |
| Auth | `claude auth login` (one-time, Anthropic side) |
| Guided setup | `coven setup claude` |
| Doctor check | `coven doctor` reports local availability; it does not verify auth. |

## Setup

<Steps>
  <Step title="Install Claude Code">
    ```bash
    npm install -g @anthropic-ai/claude-code
    ```
  </Step>
  <Step title="Run guided provider login">
    ```bash
    coven setup claude
    ```
    After explicit consent, Coven hands the terminal to `claude auth login`.
    Provider credentials stay with Claude Code; Coven never reads them.
  </Step>
  <Step title="Optionally verify provider access">
    ```bash
    coven setup claude --verify-only
    ```
    Verification requires separate consent, network access, and may incur
    provider usage or cost. It runs in ephemeral state.
  </Step>
  <Step title="Confirm with Coven">
    ```bash
    coven doctor
    ```
    Doctor confirms only that Claude Code is locally available. It remains
    offline and does not verify provider authentication.
  </Step>
  <Step title="Launch">
    ```bash
    coven run claude "polish this UI"
    ```
  </Step>
</Steps>

## Per-session flags

```bash
coven run claude "refactor for clarity" --cwd packages/web --title "Web refactor"
```

- `--cwd` — canonicalized inside the project root.
- `--title` — sets a readable title in the session browser.
- `--json` — print structured launch metadata for clients.

## Provider auth boundary

Claude Code owns its own OAuth flow and token cache. Coven never reads Anthropic keys or session cookies.

For the complete TTY, consent, timeout, verification, and redacted report
contract, see [`coven setup`](/reference/cli-setup).

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `coven doctor` reports `claude` missing | Claude Code not on `PATH` | `npm install -g @anthropic-ai/claude-code`, then re-run doctor. |
| Claude prompts for login | Auth not finished | `claude auth login` or `coven setup claude`. |
| Session shows long pre-flight pause | Claude resolving config | First run only; subsequent launches are fast. |

## How Coven supervises Claude Code

```mermaid
sequenceDiagram
  participant U as User
  participant C as coven CLI
  participant D as Coven daemon
  participant Cl as Claude PTY
  participant An as Anthropic API

  U->>C: coven run claude "refactor for clarity"
  C->>D: POST /api/v1/sessions
  D->>D: canonicalize root + cwd
  D->>D: lookup adapter for "claude"
  D->>Cl: spawn claude (prefix: --print for non-interactive, none for interactive)
  Cl->>An: provider auth (uses Anthropic local credentials — Coven does not see)
  An-->>Cl: model response stream + tool calls
  Cl-->>D: stdout / exit events
  D-->>C: SessionRecord (id, status=running)
  C-->>U: print session id, switch to attach view
```

Claude Code's tool calls run inside the Claude process — Coven does not arbitrate them. The PTY captures their output as ordinary stdout/stderr.


## Related

- [Installing harness CLIs](/harnesses/installing)
- [Provider auth boundary](/harnesses/provider-auth)
- [`coven setup`](/reference/cli-setup)
- [Troubleshooting](https://docs.opencoven.ai/docs/harnesses/troubleshooting)
