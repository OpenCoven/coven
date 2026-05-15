---
summary: "Capability discovery and action routing for clients that don't want to know which adapter handles what."
read_when:
  - Adding a new client that integrates with Coven
title: "Control plane"
---

The control plane sits in front of adapters. It lets clients:

- Discover what Coven can do with `GET /api/v1/capabilities`.
- Send known intents via `POST /api/v1/actions`.
- Stay decoupled from brittle OS automation APIs.

Unknown action ids fail closed.
