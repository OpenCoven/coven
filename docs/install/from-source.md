---
summary: "Clone the repo and build coven with cargo."
read_when:
  - Developing Coven or running unreleased changes
title: "Install from source"
description: "Build and install Coven from source: clone the repo, build the Rust daemon and CLI with cargo, and drop the binary on PATH for daily use."
---

# Install from source

Use a source checkout when you are contributing to Coven, testing unreleased changes, or running on a platform where the npm native package is not available.

## Requirements

- Rust stable.
- Git.
- A supported shell for your platform.
- At least one harness CLI if you want to launch real sessions.

## Build and verify

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

Run the binary through Cargo while developing:

```sh
cargo run -p coven-cli -- daemon start
cargo run -p coven-cli -- run codex "describe this repo"
cargo run -p coven-cli -- sessions
```

## Install the built binary

After building, copy the release binary to a directory on `PATH`:

```sh
cargo build -p coven-cli --release
mkdir -p "$HOME/.local/bin"
cp target/release/coven "$HOME/.local/bin/coven"
coven doctor
```

On Windows, copy `target\release\coven.exe` to a directory on `PATH`.

## Harness setup

Install and authenticate a harness in the same shell environment:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Then run:

```sh
coven doctor
```

## Development checks

Before changing daemon, session, attach, or ritual behavior, run the workspace checks described in [Getting started](/GETTING-STARTED) and [Documentation maintenance](/DOCS-MAINTENANCE).

For docs-only install work:

```sh
python scripts/check-secrets.py
git diff --check
```

For code work:

```sh
cargo fmt --check
cargo test --workspace --locked
```

## Related

- [Install via npm](/install/npm)
- [Install via cargo](/install/cargo)
- [Linux install](/install/linux)
