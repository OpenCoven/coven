---
summary: "Current daemon configuration surface."
read_when:
  - Configuring a Coven install
  - Relocating COVEN_HOME
title: "Configuration"
---

Coven's current daemon configuration surface is intentionally small.

## State directory

Set `COVEN_HOME` to move Coven state away from the default `~/.coven`:

```bash
export COVEN_HOME="$HOME/.local/share/coven"
coven daemon restart
```

When `COVEN_HOME` is set, the daemon uses:

- `<COVEN_HOME>/coven.sock` for the Unix socket.
- `<COVEN_HOME>/coven.sqlite3` for the session ledger and event log.
- `<COVEN_HOME>/daemon.json` for background daemon metadata.

## Unsupported knobs

The current CLI does not read `coven.toml`, `COVEN_SOCKET`, `COVEN_LOG_LEVEL`, or `COVEN_DAEMON_FOREGROUND`. Do not rely on those names until support lands in the Rust daemon.

## Applying changes

Restart after changing `COVEN_HOME`:

```bash
coven daemon restart
```

`restart` rebinds the socket under the selected state directory.

## Related

- [`$COVEN_HOME`](/daemon/coven-home)
- [launchd service](/install/launchd)
- [systemd unit](/install/systemd)
