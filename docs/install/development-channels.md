---
summary: "Nightly, beta, and stable channel rules."
read_when:
  - Following pre-release Coven builds
title: "Development channels"
description: "Coven development channels: how to run pre-release builds, switch between stable and nightly, and pin a specific @opencoven/cli version per host."
---

# Development channels

Use this page when you need to test unreleased Coven changes or pin a host to a known package version.

## Stable wrapper path

For ordinary installs:

```sh
npm install -g @opencoven/cli
coven doctor
```

Pin a specific published version when reproducing user reports:

```sh
npm install -g @opencoven/cli@<version>
coven --version
coven doctor
```

Replace `<version>` with a concrete version from the package registry or release notes you are investigating.

## Source channel

For unreleased changes:

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
git checkout <branch-or-tag>
cargo run -p coven-cli -- doctor
```

Use an isolated state directory so development builds do not disturb your normal daemon:

```sh
export COVEN_HOME="$PWD/.coven-dev"
cargo run -p coven-cli -- daemon start
cargo run -p coven-cli -- run codex "describe this repo"
```

## Switching channels

Stop the daemon before switching between wrapper installs and source builds:

```sh
coven daemon stop
```

Then run the target channel's doctor command:

```sh
coven doctor
```

or from source:

```sh
cargo run -p coven-cli -- doctor
```

## Verification

After switching channels:

```sh
coven --version
coven doctor
coven daemon restart
coven daemon status
```

If you are using a source checkout, run the same commands through Cargo or copy the release binary to the `coven` location you intend to test.

## Related

- [Updating Coven](/install/updating)
- [Install from source](/install/from-source)
- [COVEN_HOME layout](/daemon/coven-home)
