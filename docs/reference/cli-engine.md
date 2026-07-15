---
summary: "Manage the Coven engine (the interactive agent runtime)."
read_when:
  - Looking up engine
  - Installing or pinning the coven-code engine
title: "coven engine"
description: "Reference for coven engine: show the resolved engine path, source, version, and pin state; install the pinned engine into ~/.coven/engine; and print the engine binary path for scripts."
---

The engine is the `coven-code` binary that powers the interactive surfaces
(`coven`, `coven chat`) and the engine passthrough commands (`coven auth`,
`coven models`, `coven acp`, `coven code`). Coven pins a known-good engine
version and can manage the install itself.

```sh
coven engine status          # resolved path, source, version, pin state
coven engine install         # install the pinned version into ~/.coven/engine
coven engine which           # print the binary path (exit 1 if none)
```

## Status

`coven engine status --json`:

```json
{
  "installed": true,
  "path": "/home/alex/.coven/engine/0.6.1/coven-code",
  "source": "managed install",
  "version": "0.6.1",
  "pin": "0.6.1"
}
```

When no engine resolves, the JSON is `{ "installed": false }` and the prose
output points at `coven engine install`.

## Install

`coven engine install` downloads the pinned engine release into
`~/.coven/engine`. `--version <v>` installs a specific version instead of the
pin; `--force` reinstalls even when the version is already present.

## Which

`coven engine which` prints only the resolved binary path, for scripts and
editor integrations:

```sh
"$(coven engine which)" --version
```

It exits 1 when no engine is installed. `coven doctor` reports the same
resolution together with the minimum supported engine version — see
[cli-doctor](cli-doctor.md).
