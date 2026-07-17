---
summary: "Tailscale and SSH patterns for reaching a remote daemon."
read_when:
  - Exposing Coven to another machine you own
title: "Remote access"
description: "Guidance for exposing the Coven daemon socket beyond same-user local trust, and why remote access requires explicit transport instead of OAuth or tokens."
---

The daemon exposes no OAuth, tokens, or cookies — its only auth check is local
filesystem permissions on the socket (see [Auth posture](/daemon/auth-posture)).
So reaching it from another machine means providing your own authenticated
transport, never widening the daemon's own bind.

Two patterns, both keeping the API on loopback:

- **SSH tunnel** — ad hoc, nothing to install on the server:

  ```sh
  # On the client: forward local 3000 to the remote daemon's loopback API.
  ssh -N -L 3000:127.0.0.1:3000 you@your-host
  curl -fsS http://127.0.0.1:3000/api/v1/health
  ```

  Requires `coven daemon serve --tcp 127.0.0.1:3000` running on the host. The
  tunnel presents `Host: 127.0.0.1`, so the daemon's loopback guard passes with
  no extra configuration.

- **Tailscale** — always-on, reachable from your phone and every device on your
  Tailnet. This is the recommended setup for a hosted familiar; the full,
  systemd-supervised deploy is in the
  [Cloud host runbook](/daemon/cloud-host-runbook). One extra step versus the SSH
  tunnel: `tailscale serve` forwards the Tailnet FQDN as the `Host` header, which
  the loopback guard rejects, so start the daemon with `--allow-host
  <host>.<your-tailnet>.ts.net` to trust that one proxied hostname. The bind
  stays on loopback; Tailscale remains the boundary.

Never bind `--tcp` to a non-loopback or public address to achieve remote access
— the API is unauthenticated. Let SSH or Tailscale be the boundary.

See [Daemon overview](/daemon/index) and [Safety model](/daemon/safety-model).
