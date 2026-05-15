---
summary: "coven.toml, environment variables, and overrides for the Coven daemon."
read_when:
  - Configuring a Coven install
  - Relocating COVEN_HOME or changing the socket path
title: "Configuration"
---

Coven's daemon is configurable through three layers, in priority order:

1. CLI flags on `coven daemon start`.
2. `coven.toml` under `$COVEN_HOME`.
3. Environment variables.
4. Built-in defaults.

## `coven.toml`

```toml
[daemon]
home = "~/.coven"
socket = "~/.coven/coven.sock"
log_dir = "~/.coven/logs"

[harnesses.codex]
enabled = true

[harnesses.claude]
enabled = true

[control_plane]
desktop_automation = false
```

## Environment variables

| Variable | Purpose |
|---|---|
| `COVEN_HOME` | Override the state directory. |
| `COVEN_SOCKET` | Override the socket path. |
| `COVEN_LOG_LEVEL` | `error`, `warn`, `info` (default), `debug`, `trace`. |
| `COVEN_DAEMON_FOREGROUND` | Run the daemon in the foreground; do not fork. |

See [Environment variables](/help/environment) for the complete list.

## Reload behaviour

The daemon reads its configuration at start. Restart after changes:

```bash
coven daemon restart
```

`restart` rebinds the socket and reloads `coven.toml`.

## Related

- [`$COVEN_HOME`](/daemon/coven-home)
- [launchd service](/install/launchd)
- [systemd unit](/install/systemd)
