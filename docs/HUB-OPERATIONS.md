---
title: "Hub operations — supervision, watchdog, and restart guidance"
summary: "Running a Coven daemon as a server hub: what control-plane state survives restarts, systemd/launchd supervisor setup, and the restart runbook."
read_when:
  - Deploying a Coven hub on a server machine
  - Recovering hub queue/registry state after a crash or reboot
---

# Hub operations — supervision, watchdog, and restart guidance

This guide covers running a Coven daemon as a **server hub**: the durable,
canonical home for the node registry, routing table, global job queue, and
per-executor subqueues (see `specs/coven-multi-host-daemon` and
[`API-CONTRACT.md`](API-CONTRACT.md) "Hub control-plane shapes").

The short version: hub state is already durable, so your only operational job
is making sure the daemon process comes back after a crash or reboot. Use your
platform supervisor for that — do not hand-roll restart loops.

## What survives a restart (and what doesn't)

All hub control-plane state persists in `<covenHome>/coven.sqlite3`
(WAL mode):

| State | Survives restart | Notes |
| --- | --- | --- |
| Node registry | Yes | Roles, transports, capabilities, availability, last health. |
| Global job queue | Yes | `queued` / `assigned` / `held` states, priorities, loop ids. |
| Per-executor subqueues | Yes | Held work on unavailable executors is preserved. |
| Routing table | Yes | Job-to-node assignments and decision references. |
| Scheduler decisions and loop state | Yes | Explanation records for Cave/debugging. |
| Travel profiles and deltas | Yes | See travel-mode contract. |
| Live harness PTY sessions | No | In-flight local sessions are marked orphaned on restart; queue and loop state is unaffected. |

Nothing needs to be replayed manually: reopening the store reloads the
registry and queues. After a restart, verify recovery with:

```sh
curl --unix-socket ~/.coven/coven.sock http://coven/api/v1/hub/status
```

and confirm `role`, `hubId`, node availability, and queue depths match
expectations. `GET /api/v1/health` includes the same summary in its `hub`
block.

## Supervisor setup

Run the hub under a process supervisor with restart-on-failure. The daemon's
own `coven daemon start` backgrounding is fine for laptops; a server hub
should use the OS supervisor so restarts also survive reboots.

### systemd (Linux servers)

`~/.config/systemd/user/coven-hub.service`:

```ini
[Unit]
Description=Coven hub daemon
After=network.target

[Service]
# `daemon serve` runs in the foreground, which is what systemd wants.
ExecStart=%h/.local/bin/coven daemon serve
Restart=on-failure
RestartSec=2
# Give the store time to checkpoint WAL on shutdown.
TimeoutStopSec=15
# The hub is same-user local; do not widen socket exposure here.
NoNewPrivileges=true

[Install]
WantedBy=default.target
```

Enable it:

```sh
systemctl --user enable --now coven-hub.service
loginctl enable-linger "$USER"   # keep the user manager alive without a login session
```

`Restart=on-failure` with a short `RestartSec` is the watchdog: if the daemon
crashes, systemd restarts it and the hub reloads its registry and queues from
the store.

### launchd (macOS servers)

`~/Library/LaunchAgents/dev.opencoven.hub.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.opencoven.hub</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/coven</string>
    <string>daemon</string>
    <string>serve</string>
  </array>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key><false/>
  </dict>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
```

Load it:

```sh
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.opencoven.hub.plist
```

`KeepAlive` with `SuccessfulExit=false` restarts the daemon on crashes but
respects a clean `coven daemon stop`.

## Restart runbook

1. **Planned restart:** `systemctl --user restart coven-hub` (or
   `launchctl kickstart -k gui/$(id -u)/dev.opencoven.hub`). Executors do not
   need to be told; the hub polls/dispatches, so held work resumes when the
   hub is back.
2. **Crash recovery:** the supervisor restarts the daemon automatically.
   Check `GET /api/v1/hub/status` and confirm:
   - `nodesTotal` matches your registered fleet;
   - jobs that were `assigned` are still `assigned` (or `held` if their
     executor's health has gone stale in the meantime);
   - `executorQueues` still lists work for offline executors.
3. **Executor loss while the hub was down:** report the executor's health
   (`POST /api/v1/hub/nodes/:id/health` with `available: false`). Its
   assigned jobs transition to `held` and stay in its subqueue until the node
   recovers or the scheduler redispatches
   (`POST /api/v1/scheduler/redispatch`).
4. **Store integrity:** if SQLite reports corruption (unexpected after crash
   thanks to WAL, but possible with disk failure), restore
   `coven.sqlite3` + `coven.sqlite3-wal` from backup as a unit. Never edit
   queue tables by hand while the daemon is running.

## Safety boundaries

Unchanged from the single-host model:

- Do **not** expose `<covenHome>/coven.sock` over TCP, and do not run
  `daemon serve --tcp` on non-loopback interfaces — the API is
  unauthenticated.
- The supervisor must run the daemon as the same user that owns
  `<covenHome>`; the daemon fails closed on foreign-owned homes/sockets.
- Hub-to-executor transports are explicit (SSH/private network) and are the
  subject of the spoke protocol work (#267); supervision of executor daemons
  follows the same pattern as this document on each executor host.
