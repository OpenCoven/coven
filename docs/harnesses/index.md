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
  <Card title="Copilot CLI" href="/harnesses/copilot-cli" icon="github">
    GitHub Copilot CLI. Harness id `copilot`.
  </Card>
  <Card title="OpenClaw bridge" href="/harnesses/openclaw" icon="plug">
    External ACP runtime bridge through external OpenClaw bridge plugin.
  </Card>
  <Card title="Grok Build (experimental)" href="/harnesses/grok-build" icon="terminal">
    Trusted xAI Grok Build recipe. Installable harness id `grok`.
  </Card>
  <Card title="Future harnesses" href="/FUTURE-HARNESSES" icon="compass">
    Hermes, Aider, Gemini CLI, Cline — adapter direction and roadmap signals.
  </Card>
</Columns>

## What Coven supervises

```mermaid
flowchart LR
  Coven[Coven daemon] --> Adapter[Adapter router]
  Adapter --> CodexAdapter[Codex adapter]
  Adapter --> ClaudeAdapter[Claude adapter]
  Adapter --> CopilotAdapter[Copilot adapter]
  Adapter -. recipe .-> GrokAdapter[Grok Build adapter]
  Adapter -. future .-> Future[Hermes / Aider / Gemini]
  OpenClaw[OpenClaw] --> Bridge["OpenClaw bridge plugin"]
  Bridge --> Coven
  CodexAdapter --> CodexPty[Codex PTY]
  ClaudeAdapter --> ClaudePty[Claude Code PTY]
  CopilotAdapter --> CopilotPty[Copilot CLI PTY]
  GrokAdapter --> GrokPty[Grok Build PTY]
```

## What every harness has in common

- A stable **harness id** that clients pass to `coven run` or `POST /api/v1/sessions`.
- A guaranteed launch inside a canonical **project root**.
- A Coven-owned PTY for I/O, replay, and `coven attach`.
- An append-only event stream stored under the session id.
- The same **rituals**: archive, summon, sacrifice.

## What stays with the harness

- **Provider auth.** Coven does not store API keys or OAuth tokens. `codex login`, `claude doctor`, and `copilot login` keep working as they did before.
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
    # or
    npm install -g @github/copilot
    ```
  </Step>
  <Step title="Finish provider auth">
    ```bash
    codex login
    claude doctor
    copilot login
    ```
  </Step>
  <Step title="Verify Coven sees it">
    ```bash
    coven doctor
    ```
  </Step>
</Steps>

If `coven doctor` reports a harness as missing, see [Installing harness CLIs](/harnesses/installing).

Grok Build is an experimental opt-in recipe rather than a bundled default. See [Grok Build](/harnesses/grok-build) for its separate CLI and adapter installation steps.

## Related

- [Provider auth boundary](/harnesses/provider-auth)
- [OpenClaw bridge](/harnesses/openclaw)
- [Grok Build adapter](/harnesses/grok-build)
- [Harness adapter guide](/HARNESS-ADAPTERS)
- [Future harness notes](/FUTURE-HARNESSES)
