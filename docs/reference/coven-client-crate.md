---
summary: "The owner-adjacent opencoven-coven-client crate: what it packages, how it is verified, and how it is released to crates.io."
read_when:
  - Depending on the Coven daemon client from a Rust program
  - Cutting or auditing a crate release
  - Checking why a crate release workflow refused to publish
title: "Coven client crate"
description: "Reference for the opencoven-coven-client Rust crate: package identity, the coven.daemon.v1 contract it speaks, the package-contract gates, and the signed-tag release path."
source_adjacent_reason: "Documents the packaging and release contract of crates/coven-client, which is implemented and verified in this repository."
---

`opencoven-coven-client` is the published name of the workspace crate at
`crates/coven-client`. It is the Rust client the `coven` CLI is built on,
packaged so that other Rust programs can talk to a running Coven daemon over
the same owner-only local IPC and the same `coven.daemon.v1` contract.

The crate stays **owner-adjacent**: it lives in this repository, is released
from this repository, and is pre-1.0. Moving it elsewhere or promoting it to
1.0 needs its own design.

## Identity

| Field | Value |
| --- | --- |
| Package name | `opencoven-coven-client` |
| Library name | `coven_client` (unchanged from the workspace crate) |
| Version line | `0.x` |
| License | MIT (workspace) |
| Repository | `https://github.com/OpenCoven/coven` |
| Contract | `coven.daemon.v1` (`coven_client::PROTOCOL_VERSION`) |

Consumers depend on it as:

```toml
[dependencies]
coven-client = { package = "opencoven-coven-client", version = "0.1" }
```

and keep writing `use coven_client::…`. `coven-cli` depends on it the same way
through a path dependency, so the CLI and the published crate are always the
same code.

## What the package must satisfy

Two checks read the same contract from different sides:

- `crates/coven-client/tests/package_contract.rs` runs inside Cargo against the
  crate exactly as `cargo package` ships it: package name, `0.x` version,
  license/repository/readme/description metadata, the `coven_client` library
  name, no CLI/TUI/agent-runtime dependency, crate-level docs, fixtures shipped
  and parseable, README mentions of the contract and policy.
- `scripts/verify-coven-client-package.mjs` runs outside Cargo: it reads
  `cargo metadata`, checks the same identity and dependency rules, and lists
  the `cargo package` tarball to confirm the README, fixtures, and tests are
  inside it.

```sh
cargo test -p opencoven-coven-client --test package_contract --locked
node scripts/verify-coven-client-package.mjs
cargo package -p opencoven-coven-client --locked
cargo publish -p opencoven-coven-client --locked --dry-run
```

## Release path

`.github/workflows/release-crates.yml` publishes the crate. It is deliberately
separate from the npm release pipeline (`release-npm.yml`), which stays
responsible only for `@opencoven/cli` and its platform binaries.

1. **Trigger.** Push a signed, annotated tag named `coven-client-v<version>`
   whose version equals `crates/coven-client/Cargo.toml`. The workflow refuses
   lightweight tags, tags GitHub cannot verify, tags whose signer is not in
   the repository's allowed-signers variable, and tags that do not point at a
   commit on `main`.
2. **Gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -D
   warnings`, `cargo test --workspace --locked`, and the secret scan run on
   the tagged commit, exactly as CI runs them.
3. **Package.** `cargo package --locked`, the package verifier, and
   `cargo publish --dry-run` must all pass. The `.crate` file is uploaded as a
   workflow artifact so the bytes that were verified are the bytes reviewed.
4. **Publish.** Only the `publish` job has crates.io authority, through the
   `crates-io-release` environment's `CARGO_REGISTRY_TOKEN`. When that secret
   is absent the job fails closed with an explicit message; a dry run is not
   a release. A `workflow_dispatch` on an existing tag rehearses steps 1–3
   without publishing.

Rollback is by patch-forward: a published crate version is never unpublished
or re-signed. `cargo yank` may mark a version unusable for new resolutions; it
does not rewrite history and does not touch daemon authority state.

## Ownership

The crate release needs crates.io publishing authority for the
`opencoven-coven-client` name, held by the repository owner and configured as
a repository environment secret. Until it exists, the workflow's `publish` job
refuses, and the crate is verified but unpublished.
