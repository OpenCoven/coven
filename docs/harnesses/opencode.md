---
summary: "Trusted OpenCode adapter recipe for running the OpenCode CLI through Coven."
read_when:
  - Installing the OpenCode adapter
  - Reviewing OpenCode launch, prompt, and session behavior
title: "OpenCode (recipe)"
description: "Install and use Coven's trusted OpenCode adapter recipe without promoting OpenCode to a bundled default harness."
---

OpenCode is available through a trusted, installable Coven adapter recipe. It is **not** a bundled default harness: users opt in with `coven adapter install opencode`. Recipe 0.1.1 is byte-identical to the accepted [`coven-runtimes`](https://github.com/OpenCoven/coven-runtimes) registry manifest (`registry/runtimes/opencode/0.1.1.json`), so a manifest installed by Coven Cave and one installed by the CLI are the same trusted artifact.

Coven does not embed or fork OpenCode; it launches the installed CLI's `run` subcommand and reads its output like any other one-shot coding-agent CLI.

The Coven harness id is `opencode`; the executable is `opencode`.

## Install

<Steps>
  <Step title="Install the OpenCode CLI">
    ```bash
    npm i -g opencode-ai
    ```

    Other install paths are documented at https://opencode.ai/docs/cli. Verify with `opencode run --help` — the similarly-named Go app some package managers ship has **no** `run` subcommand and will not work with this adapter.

    Then finish OpenCode's own provider auth:

    ```bash
    opencode auth login
    ```
  </Step>
  <Step title="Install the trusted Coven recipe">
    ```bash
    coven adapter install opencode
    coven adapter doctor opencode
    ```

    The first command writes the versioned recipe to
    `COVEN_HOME/adapters/opencode.json`. Coven loads only exact current or
    recognized historical trusted recipe bytes; historical bytes execute the
    current recipe in memory.
  </Step>
  <Step title="Run a project-scoped session">
    ```bash
    cd /path/to/project
    coven run opencode "explain this repository"
    ```
  </Step>
</Steps>

## Adapter contract

| Coven behavior | OpenCode argv |
|---|---|
| One-shot prompt | `run -- <prompt>` |
| Model selection | `--model <provider/model>` (`model_id_transform: preserve`) |
| Familiar identity | prepended to the prompt (no system-prompt flag) |
| Sandbox / permission | none — `coven run opencode --permission …` warns and forwards no flag |
| Session resume | none — every launch is an independent `opencode run` |

OpenCode keeps system prompts, tool registries, and permissions in its own project configuration (`AGENTS.md`, `opencode.json`), not behind CLI flags, so the adapter intentionally declares no system-prompt, sandbox, or continuity surface. Configure those in the OpenCode project itself.

## Chat behavior

`coven chat` lists OpenCode once the recipe is installed and the binary is on `PATH`. Because the adapter declares no continuity args, chat resume is off: each chat turn is an independent `opencode run` with no carried conversation. See [Chat conversation persistence](/chat-persistence) for the per-harness resume matrix.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `coven adapter doctor opencode` reports `missing` | Binary not on the daemon's `PATH` | Install with `npm i -g opencode-ai`, then `coven daemon restart`. |
| `opencode run` errors with unknown command | The Go `opencode` app is shadowing the Node CLI | `which -a opencode` and remove or reorder the duplicate. |
| Adapter listed but launches fail with auth errors | OpenCode provider auth incomplete | Run `opencode auth login`; Coven never touches provider credentials. |

## Related

- [Installing harness CLIs](/harnesses/installing)
- [Harness adapters](/HARNESS-ADAPTERS)
- [Chat conversation persistence](/chat-persistence)
