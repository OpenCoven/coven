---
summary: "Trusted Hermes adapter recipe for running the native Hermes CLI through Coven."
read_when:
  - Installing the Hermes adapter
  - Reviewing Hermes model and prompt forwarding
title: "Hermes (recipe)"
description: "Install and use Coven's trusted Hermes 1.0.3 adapter recipe. Hermes is not a bundled default harness."
---

Hermes is available through a trusted, installable adapter recipe. It is
**not** a bundled default harness: install it explicitly with
`coven adapter install hermes`.

## Install the local adapter recipe

Install and complete setup for
[Hermes Agent](https://github.com/NousResearch/hermes-agent), then ensure the
native `hermes` executable resolves on `PATH`.

Install the trusted recipe instead of hand-writing a manifest:

```sh
coven adapter install hermes
coven adapter doctor hermes
coven run hermes "what is in this project?"
```

`coven adapter install hermes` writes canonical recipe 1.0.3 to
`COVEN_HOME/adapters/hermes.json`. Its bytes match
`coven-runtimes/registry/runtimes/hermes/1.0.3.json`, including the native
executable and provider-qualified model behavior used by Cave.

If Hermes is installed outside the daemon's `PATH`, add its directory to
`PATH` before starting Coven. For example, an install at
`$HOME/.local/bin/hermes` should expose `$HOME/.local/bin` to the Coven daemon;
adapter manifests intentionally take executable names, not absolute paths.

```sh
export PATH="$HOME/.local/bin:$PATH"
coven adapter install hermes
coven adapter doctor hermes
coven run hermes "what is in this project?"
```

## Adapter contract

| Coven behavior | Hermes argv |
|---|---|
| Native executable | `hermes` on Windows, macOS, and Linux |
| One-shot prompt | `chat --source coven -Q --query=<prompt>` |
| Interactive prompt | `chat --source coven --query=<prompt>` |
| Model selection | `--model <provider/model>` (`model_id_transform: preserve`) |
| Stream / continuity | none — every turn is an independent process |

The native `--query` binding replaces the historical `hermes-coven` POSIX
shim and works consistently on every platform. Coven still recognizes the
exact legacy trusted manifest and executes the current 1.0.3 recipe in memory;
modified legacy bytes are not trusted.

## Promotion checklist

Before Hermes becomes public support, finish:

- client compatibility notes for OpenClaw and CastCodes;
- `coven doctor` behavior that is backed by a real install path;
- a real-install smoke test for launch, event capture, and exit handling;
- a clear decision about resume behavior.

Until then, describe Hermes as an opt-in recipe rather than a bundled default,
and avoid scattered `hermes` string checks in product code.
