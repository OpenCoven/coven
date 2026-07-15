---
summary: "Run one always-on Coven daemon on a cloud VM, reachable only over your Tailnet."
read_when:
  - Hosting a familiar you reach from your phone or another machine
  - Standing up an always-on daemon on a server
title: "Cloud host runbook"
description: "Minimal-lift runbook for hosting the Coven daemon on a cloud VM: a single systemd-supervised `coven daemon serve` bound to loopback, fronted by Tailscale so the unauthenticated API is never exposed to the public internet."
---

Host one familiar as an always-on daemon on a cheap VM and reach it from
anywhere on your Tailnet. No new service to build — the "cloud agent" is just
`coven daemon serve` under systemd.

## Trust model (read this first)

The daemon's HTTP API is **unauthenticated** — the `--tcp` flag's own help says
so. Its only auth check is filesystem permissions on the local socket
(see [Auth posture](/daemon/auth-posture) and [Trust boundary](/daemon/trust-boundary)).

So the network boundary is **Tailscale**, not the daemon:

- Bind the TCP API to `127.0.0.1` only. Never a `0.0.0.0` or public address.
- Tailscale (WireGuard, device-authenticated) is what carries remote traffic.
- The host firewall blocks the public interface as a backstop.

Do not "just open port 3000." Anyone who reaches that port owns your familiar.

## What you need

- A small Linux VM (1 vCPU / 1 GB is plenty to start). Ubuntu/Debian assumed.
- A [Tailscale](https://tailscale.com) account (free tier is fine).
- Coven **v0.0.40 or newer** — earlier builds have the socket-takeover orphan
  storm. Confirm with `coven --version`.

## 1. Create a service user + install the binary

```sh
sudo useradd --system --create-home --home-dir /var/lib/coven --shell /usr/sbin/nologin coven
# Install the coven binary to /usr/local/bin (adjust to your install method):
sudo install -m 0755 ./coven /usr/local/bin/coven
coven --version   # must be >= v0.0.40
```

`COVEN_HOME` will be `/var/lib/coven` (set in the unit). That is where the
SQLite ledger, event log, and socket live — see [COVEN_HOME](/daemon/coven-home).

## 2. Install the systemd unit

Copy [`coven-daemon.service`](https://github.com/OpenCoven/coven/blob/main/docs/daemon/coven-daemon.service)
to `/etc/systemd/system/`, then:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now coven-daemon
systemctl status coven-daemon        # should be active (running)
```

Confirm the API is up on loopback:

```sh
curl -fsS http://127.0.0.1:3000/api/v1/health | jq .
```

You should get the health envelope (see [Health](/daemon/health)). If it fails,
tail logs with `journalctl -u coven-daemon -f` and see [Logs](/daemon/logs).

## 3. Front it with Tailscale

Install Tailscale and join the VM to your Tailnet:

```sh
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up --hostname coven-host
```

Now expose the loopback API **into the Tailnet only** — the daemon stays bound
to `127.0.0.1`; `tailscale serve` proxies to it:

```sh
sudo tailscale serve --bg http://127.0.0.1:3000
```

From any other device on your Tailnet:

```sh
curl -fsS https://coven-host.<your-tailnet>.ts.net/api/v1/health
```

That URL is your familiar in the cloud. It is reachable only by devices you have
authorized in Tailscale, over an encrypted WireGuard link, and Tailscale
terminates TLS for you.

## 4. Lock the public interface (backstop)

Even with loopback binding, close everything but SSH and let Tailscale carry the
rest:

```sh
sudo ufw default deny incoming
sudo ufw allow ssh
sudo ufw allow in on tailscale0
sudo ufw enable
```

Port 3000 is never allowed from the public interface — it does not need to be,
because the API only listens on `127.0.0.1`.

## Operating it

| Task | Command |
| --- | --- |
| Status | `systemctl status coven-daemon` / `coven daemon status` |
| Follow logs | `journalctl -u coven-daemon -f` |
| Restart | `sudo systemctl restart coven-daemon` |
| Stop | `sudo systemctl stop coven-daemon` |
| Effective COVEN_HOME | `sudo -u coven COVEN_HOME=/var/lib/coven coven doctor` |
| Upgrade | stop, replace binary, start — see [Upgrades](/daemon/upgrades) |

### One instance only

systemd owns supervision. Do **not** run `coven daemon serve` or `coven daemon
start` by hand on this host — a second `serve` against the live socket is the
exact orphan-storm failure mode that older builds hit. If the daemon wedges,
`systemctl restart coven-daemon` and check [Orphan recovery](/daemon/orphan-recovery).

## When you outgrow this

This is the minimal footing: one host, one familiar, loopback + Tailnet. When
you want push to iOS, multi-peer routing, or an authenticated public surface,
that is the `coven-relay` WebSocket relay's job (still a scaffold as of writing)
— not a wider bind on this daemon. Keep `--tcp` on loopback regardless.

See [Remote access](/daemon/remote-access) and [Safety model](/daemon/safety-model).
