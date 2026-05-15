---
summary: "Coven launches and supervises coding-agent CLIs through PTY adapters. Each harness keeps its own provider auth."
read_when:
  - Browsing what runtimes Coven can drive
  - Choosing which harness to install first
title: "Harnesses"
description: "Coven supports coding-agent CLIs as harnesses through PTY adapters. Each harness keeps its own provider authentication and command surface."
---


A **harness** is an external coding-agent CLI that Coven can launch and supervise inside an explicit project root. Coven owns the PTY, the session record, and the event log; the harness owns the conversation, the tool calls, and provider authentication.

<Columns>
  <Card title="Codex" href="/harnesses/codex" icon="binary">
    OpenAI Codex CLI. Harness id `codex`.
  </Card>
  <Card title="Claude Code" href="/harnesses/claude-code" icon="brain">
    Anthropic Claude Code. Harness id `claude`.
  </Card>
  <Card title="Future harnesses" href="/reference/future-harnesses" icon="compass">
    Hermes, Aider, Gemini CLI, Cline — adapter direction and roadmap signals.
  </Card>
</Columns>

## What Coven supervises

```mermaid
flowchart LR
  Coven[Coven daemon] --> Adapter[Adapter router]
  Adapter --> CodexAdapter[Codex adapter]
  Adapter --> ClaudeAdapter[Claude adapter]
  Adapter -. future .-> Future[Hermes / Aider / Gemini]
  CodexAdapter --> CodexPty[Codex PTY]
  ClaudeAdapter --> ClaudePty[Claude Code PTY]
```

## What every harness has in common

- A stable **harness id** that clients pass to `coven run` or `POST /api/v1/sessions`.
- A guaranteed launch inside a canonical **project root**.
- A Coven-owned PTY for I/O, replay, and `coven attach`.
- An append-only event stream stored under the session id.
- The same **rituals**: archive, summon, sacrifice.

## What stays with the harness

- **Provider auth.** Coven does not store API keys or OAuth tokens. `codex login` and `claude doctor` keep working as they did before.
- **Conversation state.** The harness owns its own prompt cache, system prompt, and tool registry.
- **Tool execution.** Tools run in-process inside the harness; Coven's job is to give it a clean PTY and a project-rooted cwd.

See [Provider auth boundary](/harnesses/provider-auth) for the credential-isolation rationale.

## Installing a harness CLI

<Steps>
  <Step title="Install one">
    ```bash
    npm install -g @openai/codex
    # or
    npm install -g @anthropic-ai/claude-code
    ```
  </Step>
  <Step title="Finish provider auth">
    ```bash
    codex login
    claude doctor
    ```
  </Step>
  <Step title="Verify Coven sees it">
    ```bash
    coven doctor
    ```
  </Step>
</Steps>

If `coven doctor` reports a harness as missing, see [Installing harness CLIs](/harnesses/installing).

## Related

- [Project root](/harnesses/project-root)
- [Working directory](/harnesses/working-directory)
- [Harness adapters reference](/reference/harness-adapters)
- [Adapter spec](/reference/adapter-spec)
