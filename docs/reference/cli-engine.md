---
summary: "Manage the Coven engine (the interactive agent runtime)."
read_when:
  - Looking up engine
  - Installing or pinning the coven-code engine
title: "coven engine"
description: "Reference for coven engine: update to the latest stable engine with checksum verification; show the resolved engine path, source, version, and pin state; install the pinned engine into ~/.coven/engine; and print the engine binary path for scripts."
source_adjacent_reason: "Tracks the engine-manager selection, verification and activation contract beside its executable regression suite."
---

The engine is the `coven-code` binary that powers the interactive surfaces
(`coven`, `coven chat`) and the engine passthrough commands (`coven auth`,
`coven models`, `coven acp`, `coven code`). Coven pins a known-good engine
version and can manage the install itself.

```sh
coven code update            # update and select the latest stable coding engine
coven code update --version 0.9.0  # select a particular stable release
coven engine status          # resolved path, source, version, pin state
coven engine install         # install the pinned version into ~/.coven/engine
coven engine which           # print the binary path (exit 1 if none)
```

## Update the engine used by the CLI

`coven code update` (also `coven code upgrade` or `coven engine update`) resolves
the latest stable Coven Code release, checks the archive against that release's
`SHA256SUMS`, verifies the candidate's reported version, and selects it in
`~/.coven/engine/<version>/`. When the target is the CLI's bundled pin, its
built-in checksum remains authoritative. Verification finishes before replacing
the selected version pointer. Download and validation failures preserve the
previous selection; separate version directories retain the previous binary.

Use `--version X.Y.Z` for a particular release and `--force` to download and
verify it again. The latest-release path refuses an implicit downgrade. A
version already installed in `~/.coven/engine/<version>/` that reports that
version is re-selected without a download or manifest, which is how to roll
back to a previous engine. Downloading any other release requires its
`SHA256SUMS` manifest; Coven Code publishes one from 0.9.0 onward. For an older
release, use `coven engine install --version X.Y.Z --sha256 HEX`. Checksums
from the same HTTPS release channel establish archive integrity, not an
independently signed build attestation or full engine conformance. Downloads
retry transient failures and abort a transfer that stalls.

The command prints the selected path and version. Existing sessions retain
their running executable; start a new `coven code` session to use the update.
An explicit `COVEN_ENGINE_BIN` override blocks the managed update before any
download. Unset the override to use the managed engine, or update the custom
engine separately.

The CLI and engine have independent versions. `npm install -g
@opencoven/cli@latest` updates the CLI; it does not promise the newest engine.
Installing a standalone `coven-code` also does not replace a managed copy.
Use `coven engine status` to inspect the engine Coven actually selects.

Offline Unix regression recipe, with no provider credentials or real downloads:

```sh
cargo test -p coven-cli --test engine_update --locked
```

The executable fixture suite also runs directly with
`python3 scripts/engine-update-test.py --binary target/debug/coven`.
Windows needs separate native archive/replacement qualification.

## Status

`coven engine status --json`:

```json
{
  "installed": true,
  "path": "/example-user/.coven/engine/0.9.0/coven-code",
  "source": "managed (~/.coven/engine)",
  "version": "0.9.0",
  "pin": "0.9.0"
}
```

When no engine resolves, the JSON is `{ "installed": false }` and the prose
output points at `coven engine install`.

## Install

`coven engine install` downloads the pinned engine release into
`~/.coven/engine`. `--version <v>` installs a specific version instead of the
pin; `--force` reinstalls even when the version is already present.
For an unpinned version, `install` requires `--sha256 HEX` supplied by the caller.
For releases that publish `SHA256SUMS` (0.9.0 and later), use
`coven code update --version X.Y.Z` to obtain the release checksum
automatically. `install` deliberately selects the bundled pin by default;
it is not a latest-release update and may select an older version.

## Which

`coven engine which` prints only the resolved binary path, for scripts and
editor integrations:

```sh
"$(coven engine which)" --version
```

It exits 1 when no engine is installed. `coven doctor` reports the same
resolution together with the minimum supported engine version — see
[cli-doctor](cli-doctor.md).
