---
summary: "Model selection in Coven means choosing a supported CLI login path."
read_when:
  - Choosing which harness to install before running Coven
  - Auditing why Coven does not ask for API keys
title: "Model selection"
description: "Coven only offers supported CLI-login harness choices: Codex CLI, Claude Code, and GitHub Copilot CLI. It does not expose API-key model providers as selectable options."
---

Coven is **not** a model gateway. It does not ask for provider API keys, OAuth
tokens, or hosted account sessions. For now, model selection is intentionally
limited to the bundled harness CLI login paths.

<Columns>
  <Card title="Codex CLI" href="/harnesses/codex" icon="binary">
    Install `@openai/codex`, run `codex login`, then launch with harness id `codex`.
  </Card>
  <Card title="Claude Code" href="/harnesses/claude-code" icon="brain">
    Install `@anthropic-ai/claude-code`, run `claude doctor`, then launch with harness id `claude`.
  </Card>
  <Card title="Copilot CLI" href="/harnesses/copilot-cli" icon="github">
    Install `@github/copilot`, run `copilot login`, then launch with harness id `copilot`.
  </Card>
</Columns>

## Setup loop

```bash
npm install -g @openai/codex
codex login

npm install -g @anthropic-ai/claude-code
claude doctor

npm install -g @github/copilot
copilot login

coven doctor
```

## What this means for clients

A client (CastCodes, comux, or the OpenClaw plugin) cannot pass an API key into
Coven or select a raw provider account. It launches `codex`, `claude`, or
`copilot` after that CLI has completed its own login flow. `coven doctor`
reports whether each CLI is installed and visible on `PATH`.

## Related

- [Codex harness](/harnesses/codex)
- [Claude Code harness](/harnesses/claude-code)
- [Copilot CLI harness](/harnesses/copilot-cli)
- [Provider auth boundary](/harnesses/provider-auth)
- [Safety model](/daemon/safety-model)
