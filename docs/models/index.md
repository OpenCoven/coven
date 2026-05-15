---
summary: "Coven does not own provider credentials. Each harness keeps using its own login."
read_when:
  - Auditing where model credentials live in a Coven install
  - Setting up a fresh provider for an existing familiar
title: "Models"
---

Coven is **not** a model gateway. It does not store API keys, OAuth tokens, or session cookies for any model provider. Each harness keeps using its own local auth flow.

<Columns>
  <Card title="Provider boundary" href="/models/provider-boundary" icon="shield-check">
    Where the credential boundary is and why it sits at the harness, not the daemon.
  </Card>
  <Card title="Why credentials stay with harnesses" href="/models/why-coven-does-not-own-credentials" icon="lock">
    The local-first rationale.
  </Card>
</Columns>

## Per-provider setup

<Columns>
  <Card title="Anthropic" href="/models/anthropic" icon="brain">
    Through Claude Code.
  </Card>
  <Card title="OpenAI" href="/models/openai" icon="binary">
    Through Codex.
  </Card>
  <Card title="Google" href="/models/google" icon="globe">
    Through the Gemini CLI adapter (planned).
  </Card>
  <Card title="Local models" href="/models/local-models" icon="cpu">
    Going fully offline through a local-model backend.
  </Card>
</Columns>

## What this means for clients

A client (comux, OpenMeow, the OpenClaw plugin) cannot pass an API key into Coven. It must launch a harness whose own provider auth is already complete. `coven doctor` will report any harness whose auth is missing.

## Related

- [Provider auth boundary](/harnesses/provider-auth)
- [Safety model](/daemon/safety-model)
