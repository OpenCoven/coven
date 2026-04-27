# @opencoven/coven

OpenClaw ACP runtime bridge for local Coven daemon sessions.

This package installs an **opt-in** OpenClaw plugin with plugin id `opencoven-coven` and ACP backend id `coven`. It lets OpenClaw route ACP coding sessions through a local Coven daemon while keeping OpenClaw's direct ACPX backend as a separately configurable fallback.

OpenClaw core does not include OpenCoven or Coven. This package is the integration boundary: OpenClaw ACP runtime calls enter the plugin, and the plugin talks to the local Coven daemon over the configured Unix socket.

## Requirements

- OpenClaw `>=2026.4.26`
- A local Coven daemon with its Unix socket at `~/.coven/coven.sock` by default
- Harness auth/config handled by the harness itself, for example Codex or Claude Code

## Install

After this package is published to ClawHub:

```bash
openclaw plugins install clawhub:@opencoven/coven
```

During development, install from a local checkout:

```bash
openclaw plugins install ./packages/openclaw-coven --force
```

## Configure

Minimal opt-in config:

```json5
{
  acp: {
    enabled: true,
    backend: "coven",
    defaultAgent: "codex",
  },
  plugins: {
    entries: {
      "opencoven-coven": {
        enabled: true,
        config: {
          covenHome: "~/.coven",
          allowFallback: true,
          fallbackBackend: "acpx",
        },
      },
    },
  },
}
```

`allowFallback` defaults to `false`. Enable it only when you intentionally want failed/unavailable Coven launches to fall back to another ACP backend such as `acpx`.

## Architecture

The plugin:

1. Registers an ACP runtime backend named `coven`.
2. Checks Coven daemon health through the configured Unix socket.
3. Launches sessions with `POST /sessions`.
4. Polls `/events?sessionId=...` for output and exit events.
5. Maps Coven events into OpenClaw ACP runtime events.
6. Records the Coven session id on the ACP runtime handle.

OpenClaw remains responsible for chat/session routing, ACP bindings, task state, and user-facing delivery. Coven owns project-scoped harness supervision, session metadata, attachability, and event history.

The plugin is a client, not a trust root. The Rust daemon must still validate project roots, cwd, harness ids, session ids, input, and kill requests before acting.

## Safety boundaries

- Disabled by default.
- Requires explicit `plugins.entries["opencoven-coven"].enabled = true` and `acp.backend = "coven"` selection.
- Does not auto-start Coven.
- Does not expose OpenClaw tools to Coven-managed harnesses.
- Restricts socket configuration to `<covenHome>/coven.sock`.
- Rejects unknown ACP agent ids unless explicitly mapped in plugin config.

## Development notes

The source lives in the Coven repo so the bridge can mature with the Coven daemon/API. Do not add Coven or OpenCoven code back into OpenClaw core as part of normal plugin work.

Because the plugin is externalized, the Coven socket API is a compatibility contract. Plugin changes should be tested against representative daemon responses, and daemon changes that affect `/health`, `/sessions`, `/events`, input, or kill behavior should update this package in the same repo.
