---
summary: "GET /api/v1/events for replay."
read_when:
  - Looking up the events API
title: "Events endpoint"
description: "Reference for the events endpoint: how clients stream and replay append-only Coven session events from the daemon's local socket API."
---

`GET /api/v1/events?sessionId=<id>` and `GET /api/v1/sessions/:id/events` return the same paginated envelope:

```json
{
  "events": [],
  "nextCursor": null,
  "hasMore": false
}
```

Supported cursors:

- `afterSeq`
- `afterEventId`
- `limit`

Payloads are redacted before storage and before API display. Raw sensitive artifacts are not included in event responses.
