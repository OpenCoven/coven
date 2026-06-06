---
summary: "Experimental Hermes adapter notes for the external manifest path."
read_when:
  - Tracking the Hermes adapter roadmap
title: "Hermes (experimental)"
description: "Experimental Hermes adapter notes for Coven's external harness manifest path. Hermes is not a built-in Coven harness."
---

Hermes is **not** a built-in Coven harness today. Do not describe it as supported by `coven doctor`, default fallback selection, CastCodes slash commands, or OpenClaw's default agent mapping.

Hermes can be used as a research target for the generic external adapter manifest once a maintainer has a real Hermes install to smoke test.

## Experimental manifest

Set `COVEN_HARNESS_ADAPTER_MANIFEST` to a JSON file like this while testing:

```json
{
  "adapters": [
    {
      "id": "hermes",
      "label": "Hermes Agent",
      "executable": "hermes",
      "interactive_prompt_prefix_args": ["chat", "--source", "coven", "-q"],
      "non_interactive_prompt_prefix_args": ["chat", "--source", "coven", "-Q", "-q"],
      "install_hint": "Install Hermes Agent and complete Hermes setup before using this adapter.",
      "system_prompt_flag": null
    }
  ]
}
```

The manifest only proves command construction and explicit opt-in loading. It does not make Hermes a default adapter.

## Promotion checklist

Before Hermes becomes public support, finish:

- command construction tests against the final CLI contract;
- client compatibility notes for OpenClaw and CastCodes;
- `coven doctor` behavior that is backed by a real install path;
- a real-install smoke test for launch, event capture, and exit handling;
- a clear decision about one-shot, interactive, and resume behavior.

Until then, keep Hermes documentation in research/experimental language and avoid scattered `hermes` string checks in product code.
