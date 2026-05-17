---
summary: "Capability discovery and action routing for clients that don't want to know which adapter handles what."
read_when:
  - Adding a new client that integrates with Coven
title: "Control plane"
description: "Coven's control plane lets clients discover capabilities and send typed action intents instead of poking brittle OS automation APIs directly."
---

The control plane sits in front of adapters. It lets clients:

- Discover what Coven can do with `GET /api/v1/capabilities`.
- Send known intents via `POST /api/v1/actions`.
- Stay decoupled from brittle OS automation APIs.

Unknown action ids fail closed.
