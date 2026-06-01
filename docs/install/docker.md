---
summary: "Run the Coven daemon inside a Docker container."
read_when:
  - Containerizing Coven for CI or homelab use
title: "Docker"
description: "Run Coven in Docker: a containerized daemon plus harness CLIs, with bind mounts for COVEN_HOME and the project root for each session."
---

Stub — fill in with Coven-specific install steps. See [Install overview](/install/index) for the canonical layout.

## Compose log rotation

When you run Coven through Docker Compose, cap the `json-file` logs on every
service. Long-lived daemon and harness containers can otherwise grow Docker's
per-container JSON log files without bound.

```yaml
services:
  coven:
    # image/build/volumes/command omitted until the Docker runtime guide
    # is finalized
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
```

Apply the same `logging` block to each Compose service that participates in the
Coven runtime, including sidecars such as local databases, bridges, or dashboards
if your deployment adds them.
