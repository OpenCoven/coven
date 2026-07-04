---
summary: "Run Coven on Raspberry Pi as a low-power home agent host."
read_when:
  - Hosting Coven on a Pi
title: "Raspberry Pi"
description: "Install Coven on a Raspberry Pi: arm64 daemon binary, COVEN_HOME on persistent storage, and systemd supervision for headless agent work."
---

# Raspberry Pi

Raspberry Pi is a source-build path today. Use a 64-bit Raspberry Pi OS image and keep `COVEN_HOME` on persistent local storage.

## Install prerequisites

```sh
sudo apt-get update
sudo apt-get install -y git curl build-essential pkg-config libssl-dev nodejs npm ca-certificates
```

Install Rust stable if it is not already present:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
```

## Build Coven

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build -p coven-cli --release
mkdir -p "$HOME/.local/bin"
cp target/release/coven "$HOME/.local/bin/coven"
coven doctor
```

Make sure `$HOME/.local/bin` is on `PATH`.

## Harness setup

Install only harness CLIs that support your Pi architecture and auth flow. Then verify from the same shell:

```sh
coven doctor
```

If Codex or Claude Code is installed with npm:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

## State and daemon

Use an explicit state directory:

```sh
export COVEN_HOME="$HOME/.coven"
coven daemon start
coven daemon status
```

For always-on operation, use [systemd unit](/install/systemd) after the manual `coven doctor` path works.

## First session

```sh
cd /path/to/project
coven run codex "describe this repo"
coven sessions
```

## Notes

- Build times can be long on small Pi models.
- Keep swap and disk space healthy before building Rust dependencies.
- Avoid storing `COVEN_HOME` on removable media that may disappear while the daemon is running.

## Related

- [Linux install](/install/linux)
- [Headless server](/install/headless-server)
- [systemd unit](/install/systemd)
