---
summary: "What lives under COVEN_HOME and how to relocate it."
read_when:
  - Customizing where Coven keeps state
title: "COVEN_HOME layout"
description: "How to lay out COVEN_HOME on a fresh install: the SQLite ledger, append-only event log, sockets, and per-session directories the daemon expects."
---

`COVEN_HOME` is Coven's local state directory. If you do not set it, Coven uses `<home>/.coven`.

Coven resolves `<home>` from the normal platform home directory. On Windows this includes `USERPROFILE` and `HOMEDRIVE` + `HOMEPATH`, so `coven doctor` should not require a Unix-style `HOME` variable.

The directory contains:

- `coven.sqlite3` — the local session ledger;
- `daemon.json` and daemon sockets/pipes — local daemon metadata;
- `sessions/` and event logs — per-session artifacts;
- `familiars.toml` — optional local familiar declarations;
- `adapters/` — trusted local harness adapter manifests, including recipes created by `coven adapter install <id>`.

Override it only when you want Coven state somewhere else:

```sh
export COVEN_HOME="$HOME/.coven"
coven doctor
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven doctor
```

See [Install overview](/install/index) for the broader install flow.
