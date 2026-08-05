---
summary: "Versioned, read-only configuration path diagnostic."
description: "Reference for coven config paths --json, including its stable schema and side-effect-free guarantees."
read_when:
  - Auditing an isolated Coven profile
  - Building a sandbox or test harness
  - Locating Coven-managed state without opening it
title: "Configuration path diagnostic"
---

`coven config paths --json` prints the versioned, machine-readable list of
filesystem locations that the current process would use. It is intended for
profile isolation checks and diagnostics, not for reading configuration or
state.

```sh
COVEN_HOME=/tmp/coven-profile coven config paths --json
```

The command does not create directories or files, open the SQLite store,
inspect familiar workspace contents, start or contact the daemon, download an
engine, or make network requests. It reads `familiars.toml` to resolve declared
workspace paths. Each run writes exactly one JSON document to standard output.

## Schema

Schema version 1 has this shape:

```json
{
  "schema": "coven.config.paths",
  "version": 1,
  "surfaces": [
    {
      "id": "coven.home",
      "status": "resolved",
      "path": "/absolute/path/to/.coven",
      "source": "environment",
      "access": "read_only"
    }
  ]
}
```

Every surface has a stable `id`, a `status`, `source`, and `access`. A
`resolved` surface has one absolute `path`, or a `paths` array for the
environment-provided adapter search roots and configured familiar workspaces.
Terminal `not_applicable`, `unsupported`, and `unresolved` surfaces
intentionally omit `path`.

`source` is `environment` when an applicable environment override selected
the location, `configuration` when `familiars.toml` selected familiar
workspaces, and `default` otherwise. `access` is always `read_only`: it
describes this diagnostic invocation, not whether another Coven command may
later write the location.

## Reported surfaces

The report covers the Coven home, session ledger, repository registry,
privacy policy, trusted and external adapter roots, settings, managed and
resolved engine locations, mobile and daemon state, familiar state, skills,
call, executor, export, memory and memory-migration state, proposals,
research, reset-backup, and travel state, the profile and daemon coordination
locks, the pending reset marker, the daemon recovery log, and the Chat
dashboard state. Redacted events and encrypted artifacts are both stored in
the session ledger, so their distinct IDs can legitimately point to the same
SQLite file.

The optional Memory dashboard is a separately installed process without a
Coven-owned state-path contract. It is reported as `unsupported` rather than
guessing a location.

## Isolation caveat

`COVEN_HOME` selects the Coven store root, but it does not move every
per-user location. Settings follow `XDG_CONFIG_HOME` or `$HOME/.config`, and
the managed engine cache follows the user home directory by design. Isolated
runners should set `COVEN_HOME`, `XDG_CONFIG_HOME`, and any user-home override
honored by their platform, then verify the resulting report. On Windows, the
native profile resolver can continue to select the OS account profile, so
`COVEN_HOME` alone does not isolate the managed cache.

The command reports paths only. It never serializes non-path contents from
`familiars.toml`, settings, privacy, adapter manifests, or the session ledger.

## Related

- [CLI reference](cli.md)
- [Environment variables](/help/environment)
- [Daemon configuration](/daemon/configuration)
