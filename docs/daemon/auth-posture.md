---
summary: "Same-user local access over local IPC. No daemon OAuth, JWTs, or browser cookies."
read_when:
  - Auditing daemon auth
title: "Auth posture"
description: "Auth posture of the Coven daemon: no OAuth, JWTs, bearer tokens, or cookies. Trust is same-user local IPC."
---

Coven trusts only same-user local IPC: a filesystem-permission-protected Unix
socket on Unix-like hosts or an owner-only named pipe on Windows. It does not
bind TCP by default.

There are no daemon auth tokens, cookies, OAuth, JWTs, or bearer credentials.
See the [socket API](/daemon/socket-api) and [trust boundary](/daemon/trust-boundary).
