---
summary: "Use GET /api/v1/health to negotiate apiVersion and capabilities before depending on response shapes."
read_when:
  - Writing a client handshake
title: "Capabilities handshake"
description: "How clients handshake with the Coven daemon by negotiating apiVersion coven.daemon.v1 and the capabilities object before calling sessions or events."
---

Start compatibility negotiation with `GET /api/v1/health`. Its `apiVersion`
must name the expected contract, currently `coven.daemon.v1`. Then check every
capability required by the operation before sending a dependent request.
Capabilities advertise availability and never grant permission.

Treat a missing, false, or malformed required capability as unsupported and
fail before sending the dependent request. Clients should ignore unknown
additive capabilities.

`GET /api/v1/api-version` is a legacy route-family diagnostic. Its literal
`apiVersion: "v1"` and `supportedApiVersions: ["v1"]` values describe the
`/api/v1/*` namespace. New clients must not use it as proof of
`coven.daemon.v1` support.

See [Daemon overview](/daemon/index) and [API contract](/reference/api-contract).
