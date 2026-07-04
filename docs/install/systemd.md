---
summary: "Run the Coven daemon as a systemd user unit."
read_when:
  - Keeping the daemon up on Linux
title: "systemd unit"
description: "Run the Coven daemon under systemd on Linux: a unit file, environment for COVEN_HOME, and journalctl access to daemon logs across reboots."
---

# systemd unit

Use a systemd user unit when you want the Coven daemon available after login on a Linux workstation or server. Install Coven and verify it manually first:

```sh
npm install -g @opencoven/cli
coven doctor
coven daemon start
coven daemon status
coven daemon stop
```

## User unit

Create the user unit directory:

```sh
mkdir -p "$HOME/.config/systemd/user"
```

Write `~/.config/systemd/user/coven-daemon.service`:

```ini
[Unit]
Description=Coven daemon
After=default.target

[Service]
Type=oneshot
RemainAfterExit=yes
Environment=COVEN_HOME=%h/.coven
ExecStart=/usr/bin/env coven daemon start
ExecStop=/usr/bin/env coven daemon stop
ExecReload=/usr/bin/env coven daemon restart

[Install]
WantedBy=default.target
```

Load and start it:

```sh
systemctl --user daemon-reload
systemctl --user enable --now coven-daemon.service
systemctl --user status coven-daemon.service
coven daemon status
```

If you need the user service to start without an active login session, enable linger for that Linux user:

```sh
loginctl enable-linger "$USER"
```

## PATH and harnesses

The service must resolve the same commands that `coven doctor` reports from your shell. If `coven`, `codex`, or `claude` is installed under a user-local path, add an explicit `Environment=PATH=...` line to the unit.

After changing PATH, harness auth, or `COVEN_HOME`:

```sh
systemctl --user daemon-reload
systemctl --user restart coven-daemon.service
coven doctor
```

## Logs

```sh
journalctl --user -u coven-daemon.service --since today
```

Use `coven daemon status` as the product-level health check; systemd only tells you whether the wrapper command ran.

## Related

- [Linux install](/install/linux)
- [Headless server](/install/headless-server)
- [COVEN_HOME layout](/daemon/coven-home)
