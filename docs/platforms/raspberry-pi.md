---
summary: "Notes on running Coven on Raspberry Pi."
read_when:
  - Hosting on a Pi
title: "Raspberry Pi platform notes"
description: "Coven on Raspberry Pi: arm64 daemon binary, COVEN_HOME on persistent storage, systemd supervision, and headless agent work on a low-power host."
---

## Install path

Use a 64-bit Raspberry Pi OS image and build from source:

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build -p coven-cli --release
mkdir -p "$HOME/.local/bin"
cp target/release/coven "$HOME/.local/bin/coven"
coven doctor
```

Make sure `$HOME/.local/bin` is on `PATH`.

## Operating model

Keep `COVEN_HOME` on persistent local storage:

```sh
export COVEN_HOME="$HOME/.coven"
```

Install only harness CLIs that support your Pi architecture and auth flow, then
run `coven doctor` from the same shell. For always-on use, prove the manual
daemon path first, then use [systemd](/install/systemd).

## Verify

```sh
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "describe this repo"
coven sessions
```

Small Pi models can take a long time to build Rust dependencies. Check disk
space and swap before rebuilding, and avoid removable media for active daemon
state.

## Related

- [Raspberry Pi install](/install/raspberry-pi)
- [Headless servers](/platforms/headless)
- [Linux](/platforms/linux)
