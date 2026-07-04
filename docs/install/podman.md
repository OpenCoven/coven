---
summary: "Run Coven under Podman with rootless containers."
read_when:
  - Daemonless container hosting
title: "Podman"
description: "Run Coven under Podman: a rootless containerized daemon plus harness CLIs, with bind mounts for COVEN_HOME and the project root per session."
---

# Podman

Podman is useful for rootless container experiments and homelab hosts. For ordinary workstation setup, prefer the native platform pages.

This page assumes you build a local image from the Coven source checkout. There is no install-docs promise of an official Podman image.

## Build a local image

Use the Dockerfile pattern from [Docker](/install/docker), then build with Podman:

```sh
podman build -t coven-local .
```

## Run doctor with persistent state

```sh
mkdir -p "$HOME/.coven-container"
podman run --rm -it \
  -e COVEN_HOME=/var/lib/coven \
  -v "$HOME/.coven-container:/var/lib/coven:Z" \
  -v "$PWD:/workspace:Z" \
  -w /workspace \
  coven-local coven doctor
```

Drop the `:Z` label suffix on systems that do not use SELinux.

## Harness setup

Install and authenticate harness CLIs inside the container image or in a derived image:

```Dockerfile
RUN npm install -g @openai/codex @anthropic-ai/claude-code
```

Then verify inside the same container environment:

```sh
podman run --rm -it \
  -e COVEN_HOME=/var/lib/coven \
  -v "$HOME/.coven-container:/var/lib/coven:Z" \
  -v "$PWD:/workspace:Z" \
  -w /workspace \
  coven-local coven doctor
```

## First session

```sh
podman run --rm -it \
  -e COVEN_HOME=/var/lib/coven \
  -v "$HOME/.coven-container:/var/lib/coven:Z" \
  -v "$PWD:/workspace:Z" \
  -w /workspace \
  coven-local coven run codex "describe this repo"
```

## Notes

- Rootless Podman changes UID/GID mappings. Keep mounted state owned by the user that runs Podman.
- Use one mounted `COVEN_HOME` per environment.
- Run `coven doctor` after every image or mount change.

## Related

- [Docker](/install/docker)
- [Headless server](/install/headless-server)
- [Linux install](/install/linux)
