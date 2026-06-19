---
summary: "Experimental Hermes adapter notes for the external manifest path."
read_when:
  - Tracking the Hermes adapter roadmap
title: "Hermes (experimental)"
description: "Experimental Hermes adapter notes for Coven's trusted local adapter recipe. Hermes is not a built-in Coven harness."
---

Hermes is **not** a built-in Coven harness today. Do not describe it as supported by default fallback selection, CastCodes slash commands, or OpenClaw's default agent mapping.

Hermes can be used as a research target for the generic external adapter system once a maintainer has a real Hermes install to smoke test.

## Install the local adapter recipe

Use the bundled recipe instead of hand-writing a manifest:

```sh
coven adapter install hermes
coven adapter doctor hermes
coven run hermes "what is in this project?"
```

`coven adapter install hermes` writes a trusted manifest to `COVEN_HOME/adapters/hermes.json`. Coven loads manifests from that Coven-owned trust store automatically, plus any manifests explicitly named with `COVEN_HARNESS_ADAPTER_MANIFEST` or `COVEN_HARNESS_ADAPTER_DIRS`.

If Hermes is installed outside the daemon's `PATH`, add its directory to
`PATH` before starting Coven. For example, a Raspberry Pi install at
`/home/o/.local/bin/hermes` should expose `/home/o/.local/bin` to the Coven
daemon; adapter manifests intentionally take executable names, not absolute
paths.

```sh
export PATH="$HOME/.local/bin:$PATH"
coven adapter install hermes
coven adapter doctor hermes
coven run hermes "what is in this project?"
```

## Promotion checklist

Before Hermes becomes public support, finish:

- command construction tests against the final CLI contract;
- client compatibility notes for OpenClaw and CastCodes;
- `coven doctor` behavior that is backed by a real install path;
- a real-install smoke test for launch, event capture, and exit handling;
- a clear decision about one-shot, interactive, and resume behavior.

Until then, keep Hermes documentation in research/experimental language and avoid scattered `hermes` string checks in product code.
