---
summary: "Run the Coven daemon inside a Docker container."
read_when:
  - Containerizing Coven for CI or homelab use
title: "Docker"
description: "Run Coven in Docker: a containerized daemon plus harness CLIs, with bind mounts for COVEN_HOME and the project root for each session."
---

# Docker

Docker is an advanced setup path. This repository does not define a canonical Coven application image in the install docs; build your own image when you need container isolation for CI, demos, or a homelab.

Use native installs for normal workstation use: [macOS](/install/macos), [Linux](/install/linux), [Windows](/install/windows), or [WSL2](/install/wsl2).

## Minimal source-built image

Create a Dockerfile in your own deployment repo:

```Dockerfile
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build -p coven-cli --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates nodejs npm git \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/coven /usr/local/bin/coven
ENV COVEN_HOME=/var/lib/coven
WORKDIR /workspace
CMD ["coven", "doctor"]
```

Build it from a Coven source checkout:

```sh
docker build -t coven-local .
```

## Run with explicit mounts

```sh
mkdir -p "$HOME/.coven-container"
docker run --rm -it \
  -e COVEN_HOME=/var/lib/coven \
  -v "$HOME/.coven-container:/var/lib/coven" \
  -v "$PWD:/workspace" \
  -w /workspace \
  coven-local coven doctor
```

For real harness work, the container must also contain and authenticate the harness CLI. Provider credentials remain owned by that harness, not by Coven.

## First container session

```sh
docker run --rm -it \
  -e COVEN_HOME=/var/lib/coven \
  -v "$HOME/.coven-container:/var/lib/coven" \
  -v "$PWD:/workspace" \
  -w /workspace \
  coven-local coven daemon start
```

Then run a session in the same mounted environment:

```sh
docker run --rm -it \
  -e COVEN_HOME=/var/lib/coven \
  -v "$HOME/.coven-container:/var/lib/coven" \
  -v "$PWD:/workspace" \
  -w /workspace \
  coven-local coven run codex "describe this repo"
```

## Notes

- Bind-mount `COVEN_HOME` if you want session history to survive container exits.
- Bind-mount the project root you intend to run in.
- Do not expose the Coven daemon socket over TCP by default.
- Run `coven doctor` inside the same image and environment that will launch sessions.

## Related

- [Headless server](/install/headless-server)
- [Podman](/install/podman)
- [COVEN_HOME layout](/daemon/coven-home)
