---
summary: "Clients can ask; only the daemon decides through same-user local IPC."
read_when:
  - Documenting trust for a security review
title: "Trust boundary"
description: "Trust boundary for the Coven daemon: same-user local IPC is the trust surface."
---

The daemon accepts only same-user local IPC: a filesystem-permission-protected
Unix socket on Unix-like hosts or an owner-only named pipe on Windows. It does
not bind TCP by default.

Clients are not authority boundaries. The Rust daemon revalidates sensitive
requests before acting. See [Auth posture](/daemon/auth-posture) and the
[socket API](/daemon/socket-api).
