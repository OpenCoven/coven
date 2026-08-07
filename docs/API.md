# Coven local API

The canonical public API guide and interactive endpoint reference live at:

**https://docs.opencoven.ai/docs/reference/api**

Clients begin with `GET /api/v1/health` and require the named
`coven.daemon.v1` contract plus the capabilities needed for their operation.
Capabilities advertise feature availability; they never grant permission.

`GET /api/v1/api-version` is a legacy route-family diagnostic. Its literal
`v1` value is not proof of named-contract compatibility.

The normative source contract remains in [`API-CONTRACT.md`](API-CONTRACT.md).
Client implementers should also read
[`CLIENT-INTEGRATION.md`](CLIENT-INTEGRATION.md).
