---
summary: "Run GitHub Copilot CLI under Coven supervision. Harness id `copilot`."
read_when:
  - Setting up GitHub Copilot CLI for Coven
  - Diagnosing Copilot-specific harness failures
title: "Copilot CLI harness"
description: "Run the GitHub Copilot CLI under Coven supervision with harness id copilot, project-rooted sessions, and the usual attach and ritual flows."
---


GitHub Copilot CLI is GitHub's coding-agent CLI. Coven uses a project-rooted
PTY for both interactive and one-shot launches, so sessions, attaches, and
rituals work the same as for any other harness.

| Field | Value |
|---|---|
| Harness id | `copilot` |
| Install | `npm install -g @github/copilot` or `brew install --cask copilot-cli` |
| Auth | `copilot login` (one-time, GitHub side) |
| Doctor check | `coven doctor` reports Copilot CLI availability and the install hint when missing. |

## Setup

<Steps>
  <Step title="Install Copilot CLI">
    ```bash
    npm install -g @github/copilot
    # or
    brew install --cask copilot-cli
    ```
    Other install methods are listed in the [Copilot CLI docs](https://docs.github.com/en/copilot/concepts/agents/about-copilot-cli).
  </Step>
  <Step title="Log in to GitHub">
    ```bash
    copilot login
    ```
    GitHub credentials stay with Copilot. Coven never reads them.
  </Step>
  <Step title="Confirm with Coven">
    ```bash
    coven doctor
    ```
    The Harnesses section should include `[OK] Copilot CLI` with the resolved `copilot` executable.
  </Step>
  <Step title="Launch">
    ```bash
    coven run copilot "fix the failing tests"
    ```
  </Step>
</Steps>

## Per-session flags

```bash
coven run copilot "audit this repo" --permission read-only --speed fast
```

- `--cwd` — canonicalized inside the project root.
- `--title` — sets a readable title in the session browser.
- `--model <id>` — forwards to `copilot --model`. Note that Copilot's `auto`
  model rejects effort configuration, so don't combine `--model auto` with
  `--think`/`--speed`.
- `--think` / `--speed fast|balanced|thorough` — map to `copilot --effort`.
- `--permission full|read-only` — see the permission mapping below.
- `--add-dir <DIR>` — forwards to `copilot --add-dir` (repeatable); Copilot
  natively refuses file access outside its allowed directories.
- `--json` — print structured launch metadata for clients.

## Permission mapping

Copilot's permission surface is boolean/multi-token flags rather than a
single mode flag, so Coven's `--permission` maps to argv lists:

| Coven policy | Copilot argv | Effect |
|---|---|---|
| `full` | `--allow-all` | All tools, paths, and URLs run without confirmation. |
| `read-only` | `--deny-tool write --deny-tool shell` | File writes and shell commands are denied outright (deny rules beat every allow rule). Reads inside the working directory stay allowed. |
| *(none)* | *(no flags)* | Copilot's own defaults apply. In non-interactive mode Copilot auto-denies any tool that would have prompted, so an unflagged `coven run copilot` can read and reply but not modify anything. |

Operators who want unattended full access without passing `--permission full`
every run can use Copilot's own `COPILOT_ALLOW_ALL` environment variable —
that stays a harness-side decision, exactly like
`COVEN_CLAUDE_BYPASS_PERMISSIONS` for Claude.

## Session continuity

Copilot supports pre-assigned session ids: `coven chat` sends
`--session-id <uuid>` on the first turn and the same flag on later turns.
`--session-id` both creates a fresh session under a chosen UUID and resumes
an existing one, so stale ids self-heal into a new conversation instead of
erroring. One-shot stats output ends with a
`Resume     copilot --resume=<id>` hint you can use directly with the
Copilot CLI outside Coven.

## Output shape

Copilot has no long-lived stream-json stdin mode, so Coven always launches it
as a one-shot process under the PTY (the chat TUI uses its per-turn path, the
same posture as Codex). Non-interactive launches pass `--no-color`, and each
run closes with a columnar stats block (`Changes` / `Requests` / `Tokens` /
`Resume`) that Coven's chat view hides from the transcript.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `coven doctor` reports `copilot` missing | Copilot CLI not on `PATH` | `npm install -g @github/copilot` (or `brew install --cask copilot-cli`), then re-run doctor. |
| Runs fail immediately with an auth error | Not logged in | `copilot login`. |
| `Error: Model "auto" does not support reasoning effort configuration` | `--model auto` combined with `--think`/`--speed` | Drop the effort flag or pick a concrete model. |
| Session can't read a file outside the repo | Copilot's path verification | Re-run with `--add-dir <that-directory>`. |
| Stale version reported | Older binary earlier on `PATH` | `which -a copilot` and remove the duplicate. |

## Provider auth boundary

Copilot owns its GitHub auth flow and token cache (under `~/.copilot/`).
Coven never reads, proxies, or stores those credentials — see the
[provider auth boundary](/harnesses/provider-auth).

## Related

- [Installing harness CLIs](/harnesses/installing)
- [Provider auth boundary](/harnesses/provider-auth)
- [Harness adapter guide](/HARNESS-ADAPTERS)
- [Troubleshooting](/TROUBLESHOOTING#harness-missing)
