---
summary: "Current daemon configuration surface."
read_when:
  - Configuring a Coven install
  - Relocating COVEN_HOME
title: "Configuration"
description: "Daemon configuration surface for Coven: COVEN_HOME, socket location, log paths, and the minimal knobs the Rust process reads on start and restart."
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
- `<COVEN_HOME>/privacy.toml` for local log privacy settings when present.
- `<COVEN_HOME>/keys/session-artifacts.key` for the local encrypted artifact key when raw artifact persistence is enabled.

## Log privacy

Session logs are redacted by default before they are stored in SQLite or returned through the API.

Optional `privacy.toml`:

```toml
persist_raw_artifacts = false
raw_artifact_retention_days = 7
log_retention_days = 30
extra_patterns = ["custom-sensitive-[0-9]+"]
```

Environment overrides:

- `COVEN_PERSIST_RAW_ARTIFACTS=1`
- `COVEN_RAW_ARTIFACT_RETENTION_DAYS=<days>`
- `COVEN_LOG_RETENTION_DAYS=<days>`

Raw artifacts are unavailable unless explicitly enabled. When enabled, raw artifacts are encrypted at rest with a local key outside SQLite. If the key cannot be loaded or created, Coven keeps redacted logging working and skips raw artifact persistence.

## Unsupported knobs

The current CLI does not read `coven.toml`, `COVEN_SOCKET`, `COVEN_LOG_LEVEL`, or `COVEN_DAEMON_FOREGROUND`. Do not rely on those names until support lands in the Rust daemon.

## Applying changes

Restart after changing `COVEN_HOME`:

```bash
coven daemon restart
```

`restart` rebinds the socket under the selected state directory.

Prune retained log data:

```bash
coven logs prune --dry-run
coven logs prune
```

## Related

- [`$COVEN_HOME`](/daemon/coven-home)
- [launchd service](/install/launchd)
- [systemd unit](/install/systemd)
