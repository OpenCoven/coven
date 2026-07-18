---
summary: "Experimental Grok Build adapter recipe for running xAI's coding-agent CLI through Coven."
read_when:
  - Installing the Grok Build adapter
  - Reviewing Grok Build launch, permission, and session behavior
title: "Grok Build (experimental)"
description: "Install and use Coven's trusted Grok Build adapter recipe without promoting Grok to a bundled default harness."
---

Grok Build is available through a trusted, installable Coven adapter recipe. It is **not** a bundled default harness yet: users opt in with `coven adapter install grok`, and the recipe stays experimental until the promotion checklist below is complete.

Coven does not embed or fork Grok Build; it launches the installed CLI and reads its plain-text headless output like any other one-shot coding-agent CLI (Codex, Hermes) — no custom protocol or event translation is involved.

The Coven harness id is `grok`; the executable is `grok`.

## Install

<Steps>
  <Step title="Install and authenticate Grok Build">
    ```bash
    curl -fsSL https://x.ai/cli/install.sh | bash   # macOS / Linux / Git Bash
    irm https://x.ai/cli/install.ps1 | iex          # Windows PowerShell
    ```

    These are the official installers from the Grok Build README; see the install guide at https://docs.x.ai/build for other options. Then authenticate with the CLI:

    ```bash
    grok login
    # Headless or remote machine:
    grok login --device-code
    ```

    Grok also supports `XAI_API_KEY` for headless automation. Coven does not read, store, or inject that credential; Grok resolves it from its own inherited environment.
  </Step>
  <Step title="Install the trusted Coven recipe">
    ```bash
    coven adapter install grok
    coven adapter doctor grok
    ```

    The first command writes the versioned recipe to `COVEN_HOME/adapters/grok.json`. Coven only loads the file while it exactly matches its bundled trusted recipe.
  </Step>
  <Step title="Run a project-scoped session">
    ```bash
    cd /path/to/project
    coven run grok --permission full "explain this repository"
    ```

    Always pass `--permission` explicitly with Grok — see [Permissions](#permissions) below for why.
  </Step>
</Steps>

## Adapter contract

| Coven behavior | Grok Build argv |
|---|---|
| One-shot prompt | `--single=<prompt>` |
| Model selection | `--model <model>` |
| Familiar identity | `--rules <identity>` |
| New named conversation | `--session-id <uuid>` |
| Resume conversation | `--resume <uuid>` |
| Headless output | `--output-format plain` (Grok Build's own default) |
| Deterministic startup | `--no-auto-update` |

The prompt is bound with the long flag's `=` form and remains the final argv entry. A prompt beginning with `-` therefore stays user data and cannot become a Grok CLI option. Coven launches the executable directly and never constructs a shell command string.

Grok Build's `--output-format plain` headless mode prints only the final response text to stdout (with a trailing newline) — per its own public source, reasoning/"thought" content is dropped before it ever reaches stdout in this mode, and every other event (errors, compaction notices, max-turns) goes to stderr instead. Coven therefore treats Grok exactly like any other one-shot CLI: no JSON parsing, no event schema, no translation layer. Every prompt starts one finite `grok --single` process.

The `--session-id`/`--resume` rows above apply to **Coven chat sessions**, which pre-assign a conversation UUID on the first turn and cold-start later turns with `--resume <same UUID>` — the same mechanism Copilot chat uses. A plain `coven run grok <prompt>` turn does not pre-assign a session id (true of every non-stream harness today), so following it with `coven run grok --continue <id>` would ask Grok to resume a session it never created; unlike Copilot's `--session-id` (which creates-or-resumes), Grok's `--resume` requires the session to exist — and its `--session-id` symmetrically refuses an id that already exists, so neither flag can self-heal a stale id the way Copilot's does. Instead, `coven chat` recognizes Grok's `Session does not exist` resume error (e.g. after Grok's local session store is cleaned) and auto-recovers the same way it does for Claude and Codex: it drops the stale id and transparently starts a fresh conversation with the original message re-sent. Treat plain-CLI `--continue` with Grok as unsupported for now — see the maturity list below. Its failure mode is at least honest: in headless/piped use the doomed `--resume` hits Grok's strict session resolver and fails fast with Grok's own `Error: Session does not exist` and a non-zero exit (no hang, no silent fresh session), and in interactive use the continuity hint is not applied at all — a property shared with every harness, since continuity args only apply to non-interactive launches.

## Permissions

`coven run --permission` maps to both Grok's permission policy and its process sandbox:

| Coven policy | Grok Build mapping |
|---|---|
| `full` | `--permission-mode bypassPermissions --sandbox off` |
| `read-only` | `--permission-mode default --sandbox read-only` |

**Always pass `--permission` explicitly when running Grok.** Coven's general convention is that omitting `--permission` leaves a harness at its own native default, treated as equivalent to `full` — this holds for Codex, Claude Code, and Copilot CLI, whose native one-shot defaults are known to be non-blocking. Per Grok's public source, that convention does **not** extend to Grok: its headless runner never waits for approval input — any tool call that would prompt is automatically **cancelled** and the cancellation is reported to the model, which then continues the turn. So an omitted `--permission` leaves headless Grok in its `default` mode, where every non-auto-approved (i.e. mutating) tool call is silently cancelled — the run does not hang, but it also is nothing like `full`; it behaves closer to a degraded read-only turn. Treat an omitted `--permission` with Grok as unsupported, not as "defaults to full."

On the flag itself, Grok's source accepts `plan`, `dontAsk`, and `acceptEdits` as command-line compatibility values but only ever activates `bypassPermissions` and `default` from `--permission-mode`; the other policies are settings-file-only (`defaultMode` in its Claude-compatible settings). The adapter therefore selects `default` for read-only and relies on the native read-only sandbox for the filesystem boundary — a mapping that is fail-closed twice over in headless runs: a write attempt's approval request is auto-cancelled by the headless client, and the sandbox denies the write at the OS level regardless. Grok's own documentation notes that child-process network blocking in restrictive sandbox profiles is currently enforced on Linux but not macOS; treat that platform limitation as part of Grok's boundary, not a guarantee supplied by Coven.

Grok Build does not document a native additional-directory flag, so `coven run grok --add-dir ...` is a warned no-op. Start the session at the intended project root instead.

**`coven chat` turns run in Grok's read-and-answer default.** The chat launch API currently carries no permission field for any harness, so chat turns always launch without `--permission-mode`. For Codex/Claude/Copilot that lands on their non-blocking native defaults; for Grok it lands on the auto-cancel `default` mode described above — chat with Grok can read the project and answer questions, but a turn that tries to edit files or run non-auto-approved commands has those tool calls auto-cancelled (the model is told, and will typically say so in its reply). For edit tasks, use `coven run grok --permission full` directly. Lifting this properly means adding permission plumbing to the chat launch API — a chat-surface change that affects every harness and is out of scope for this recipe.

## Current maturity

This recipe covers:

- safe command construction for normal and `-`-prefixed prompts (unit-tested);
- `coven adapter install` and a basic executable-presence check in `coven adapter doctor`;
- model, familiar-identity, and permission/sandbox argv mapping (unit-tested);
- preassigned session ids and resume argv (unit-tested);
- chat stale-session auto-recovery: `coven chat` matches Grok's `Session does not exist` resume error and transparently restarts the conversation, since Grok's strict `--session-id`/`--resume` pair cannot self-heal a stale id (unit-tested);
- unauthenticated fail-fast behavior verified against the real Grok Build 0.2.102 binary: a headless run with no credentials fails in about a second with a structured, human-readable error and a non-zero exit code — no protocol-specific handling was needed for this, since Coven treats any non-zero exit as a failed turn the same way it does for every other one-shot harness.

Not yet done, and required before this graduates past experimental:

- a real authenticated multi-turn smoke test (first turn, resume, read-only vs. full permission enforcement) against a live Grok Build account;
- live confirmation of the source-verified headless permission behavior (would-prompt tool calls auto-cancelled and reported to the model, never a hang) — note that because an omitted `--permission-mode` leaves headless Grok cancelling writes rather than acting like `full`, the "omit `--permission` ⇒ full" convention cannot be extended to Grok as its CLI stands today, so the explicit-permission-required guidance above is expected to stay;
- repeating both on Linux and Windows, not just macOS;
- a continuity story for plain `coven run --continue` (today only chat sessions pre-assign Grok's session id, so plain-CLI resume would target a session Grok never created);
- permission plumbing for `coven chat` (the chat launch API passes no `--permission` for any harness; with Grok that means chat turns are read-and-answer only, with mutating tool calls auto-cancelled — see [Permissions](#permissions)).

## Upstream references

- [Grok Build getting started](https://docs.x.ai/build/overview)
- [Grok Build source](https://github.com/xai-org/grok-build)
- [CLI reference](https://docs.x.ai/build/cli/reference)
- [Headless and scripting](https://docs.x.ai/build/cli/headless-scripting)
- [Sandbox and permission controls](https://docs.x.ai/build/enterprise)

## Related

- [Harnesses](/harnesses)
- [Harness adapter guide](/HARNESS-ADAPTERS)
- [Provider auth boundary](/harnesses/provider-auth)
