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
    npm install -g @xai-official/grok
    ```

    Or follow the official install guide at https://docs.x.ai/build. Then authenticate with the CLI:

    ```bash
    grok login
    # Headless or remote machine:
    grok login --device-auth
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

The `--session-id`/`--resume` rows above apply to **Coven chat sessions**, which pre-assign a conversation UUID on the first turn and cold-start later turns with `--resume <same UUID>` — the same mechanism Copilot chat uses. A plain `coven run grok <prompt>` turn does not pre-assign a session id (true of every non-stream harness today), so following it with `coven run grok --continue <id>` would ask Grok to resume a session it never created; unlike Copilot's `--session-id` (which creates-or-resumes), Grok's `--resume` requires the session to exist. Treat plain-CLI `--continue` with Grok as unsupported for now — see the maturity list below.

## Permissions

`coven run --permission` maps to both Grok's permission policy and its process sandbox:

| Coven policy | Grok Build mapping |
|---|---|
| `full` | `--permission-mode bypassPermissions --sandbox off` |
| `read-only` | `--permission-mode default --sandbox read-only` |

**Always pass `--permission` explicitly when running Grok.** Coven's general convention is that omitting `--permission` leaves a harness at its own native default, treated as equivalent to `full` — this holds for Codex, Claude Code, and Copilot CLI, whose native one-shot defaults are known to be non-blocking. Grok Build has not yet been verified against this convention: it is an interactive-first tool that also supports headless use, and its headless behavior when no `--permission-mode` is given at all (whether it blocks waiting for an approval that can never arrive, or fails safe) has not been confirmed with a real authenticated run. Until that's verified, treat an omitted `--permission` with Grok as unsupported, not as "defaults to full."

Grok's public source accepts `plan` as a compatibility value on the command line but does not activate a plan permission policy from that value, so the adapter explicitly selects `default` and relies on the native read-only sandbox for the filesystem boundary. Grok's own documentation notes that child-process network blocking in restrictive sandbox profiles is currently enforced on Linux but not macOS; treat that platform limitation as part of Grok's boundary, not a guarantee supplied by Coven.

Grok Build does not document a native additional-directory flag, so `coven run grok --add-dir ...` is a warned no-op. Start the session at the intended project root instead.

## Current maturity

This recipe covers:

- safe command construction for normal and `-`-prefixed prompts (unit-tested);
- `coven adapter install` and a basic executable-presence check in `coven adapter doctor`;
- model, familiar-identity, and permission/sandbox argv mapping (unit-tested);
- preassigned session ids and resume argv (unit-tested);
- unauthenticated fail-fast behavior verified against the real Grok Build 0.2.102 binary: a headless run with no credentials fails in about a second with a structured, human-readable error and a non-zero exit code — no protocol-specific handling was needed for this, since Coven treats any non-zero exit as a failed turn the same way it does for every other one-shot harness.

Not yet done, and required before this graduates past experimental:

- a real authenticated multi-turn smoke test (first turn, resume, read-only vs. full permission enforcement) against a live Grok Build account;
- resolving the open permission-mode question above — specifically, confirming what headless Grok does when `--permission-mode` is omitted entirely, so the "omit `--permission` ⇒ full" convention can either be extended to Grok or the docs' current explicit-permission-required guidance can be relaxed;
- repeating both on Linux and Windows, not just macOS;
- a continuity story for plain `coven run --continue` (today only chat sessions pre-assign Grok's session id, so plain-CLI resume would target a session Grok never created).

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
