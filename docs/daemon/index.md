---
summary: "The Coven daemon is the Rank 0 authority for sessions, PTYs, and the local IPC API."
read_when:
  - Operating Coven on a workstation or server
  - Auditing what Coven validates vs. trusts from clients
title: "Daemon"
description: "Overview of the Coven daemon: a single Rust process per host that owns PTY lifecycle, the SQLite session ledger, the event log, and the local IPC API."
---

The Coven daemon is a single Rust process per host. It owns:

- Live session state and PTY lifecycle for every supported harness.
- The SQLite session ledger and the append-only event log.
- The HTTP API over same-user local IPC under `/api/v1`: `<COVEN_HOME>/coven.sock`
  on Unix-like hosts, or an owner-only named pipe selected by `COVEN_HOME` on
  Windows. Health and `coven daemon status` report the active endpoint; clients
  must not construct a Windows pipe name from the Unix convention.
- Capability discovery and action routing in front of adapters.
- Path canonicalization, project-root validation, and authority checks.

Clients (CastCodes, `coven` CLI/TUI, comux, the external OpenClaw plugin) **never** spawn harness PTYs themselves. They ask the daemon.

<Columns>
  <Card title="Lifecycle" href="/daemon/lifecycle" icon="play-circle">
    `start`, `status`, `restart`, `stop` — and what each one actually does.
  </Card>
  <Card title="Socket API" href="/daemon/socket-api" icon="plug">
    HTTP over local IPC. Handshake with `GET /api/v1/health`.
  </Card>
  <Card title="Safety model" href="/daemon/safety-model" icon="shield">
    Trust boundary, secret handling, and automation approvals.
  </Card>
</Columns>

## Where the daemon lives

| Path | Purpose |
|---|---|
| `$COVEN_HOME` | Root state directory. Default `~/.coven` on macOS/Linux. |
| `$COVEN_HOME/coven.sock` | Same-user Unix socket on Unix-like hosts. |
| daemon-reported named pipe | Owner-only local IPC endpoint on Windows. |
| `$COVEN_HOME/coven.sqlite3` | SQLite session ledger and append-only event log. |
| `$COVEN_HOME/daemon.json` | Background daemon pid, start time, and socket metadata. |

See [`$COVEN_HOME`](/daemon/coven-home) for the full layout and how to relocate it.

## Daemon control

```bash
coven daemon start
coven daemon status
coven daemon restart
coven daemon stop
```

`status` shows the pid, active endpoint, uptime, and negotiated `apiVersion`.
Use it before depending on the daemon in a script.

## Health handshake

Every client should begin with:

```http
GET /api/v1/health
```

The response includes:

- `apiVersion` — the named contract (`coven.daemon.v1`).
- `capabilities` — the discoverable feature set.
- `daemon.uptime`, `daemon.pid`, `daemon.startedAt`.
- `daemon.socket` — the active local IPC endpoint.

See [Capabilities handshake](/daemon/capabilities-handshake).

## Authority boundary

The daemon validates every request, even from local clients. See [Authority boundary](/concepts/authority-boundary). Clients are convenience layers, not trust roots.

## Related

- [Configuration](/daemon/configuration)
- [Auth posture](/daemon/auth-posture)
- [Remote access](/daemon/remote-access)
- [Cloud host runbook](/daemon/cloud-host-runbook)
- [Logs](/daemon/logs)
- [Diagnostics](/daemon/diagnostics)
