---
summary: "Discoverable daemon and adapter features exposed via the control plane."
read_when:
  - Building a client that adapts to what Coven can do
  - Adding a new capability to Coven
title: "Capabilities"
description: "Capabilities are the discoverable feature set of the Coven daemon. Clients query GET /api/v1/capabilities before using sessions, events, or actions."
---

A **capability** is a discoverable feature of the Coven daemon or one of its adapters. Clients call `GET /api/v1/capabilities` to find out what is available before they call anything else.

<Columns>
  <Card title="Discovery" href="/capabilities/discovery" icon="search">
    `GET /api/v1/capabilities` — what is returned and how to use it.
  </Card>
  <Card title="Action routing" href="/capabilities/action-routing" icon="route">
    `POST /api/v1/actions` — send a known intent through the control plane.
  </Card>
</Columns>

## Capability record

```json
{
  "id": "desktop.automation.window.activate",
  "label": "Activate window",
  "owner": "adapter.desktop-use",
  "status": "enabled",
  "policy": "approval-required",
  "actions": ["window.activate", "window.focus"]
}
```

Records include:

- **id** — stable identifier.
- **label** — human-readable name.
- **owner** — the adapter that fulfils the capability.
- **status** — `enabled` / `disabled` / `degraded`.
- **policy** — hint for whether the action needs an approval prompt.
- **actions** — the action ids the capability accepts.

## Fail-closed routing

Unknown action ids fail closed. Adding a new capability is the only way to surface a new intent to clients.

## Related

- [Control plane](/concepts/control-plane)
- [API: capabilities endpoint](/reference/api-capabilities)
- [API: actions endpoint](/reference/api-actions)
