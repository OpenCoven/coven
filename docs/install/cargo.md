---
summary: "Build and install Coven directly from crates.io with cargo."
read_when:
  - You prefer building Rust binaries yourself
title: "Install via cargo"
description: "Install Coven from source with cargo: build the Rust daemon and CLI, drop the binary on PATH, and verify the install with coven doctor."
---

# Install via cargo

Use this route when you want to build the Rust CLI yourself. For most users, [Install via npm](/install/npm) is the shorter path.

## From a checkout

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build -p coven-cli --release
mkdir -p "$HOME/.local/bin"
cp target/release/coven "$HOME/.local/bin/coven"
coven doctor
```

On Windows, copy `target\release\coven.exe` to a directory on `PATH`, then open a new terminal and run:

```powershell
coven doctor
```

## Running without copying

From the repository checkout:

```sh
cargo run -p coven-cli -- doctor
cargo run -p coven-cli -- daemon start
cargo run -p coven-cli -- run codex "describe this repo"
```

## Harness setup

Coven still needs a harness CLI for real agent work:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Run `coven doctor` after installing or changing harness auth.

## Updating a cargo-built binary

```sh
cd /path/to/coven
git pull --ff-only
cargo build -p coven-cli --release
cp target/release/coven "$HOME/.local/bin/coven"
coven daemon restart
coven doctor
```

Use the Windows binary name when updating a Windows install.

## Related

- [Install from source](/install/from-source)
- [COVEN_HOME layout](/daemon/coven-home)
- [Updating Coven](/install/updating)
