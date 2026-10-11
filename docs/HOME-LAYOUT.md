---
title: "COVEN_HOME layout contract"
description: "Who owns which paths under COVEN_HOME, how backups, temp files, and archives are named, and how a long-lived install stays orderly without deleting state."
source_adjacent_reason: "Describes the COVEN_HOME paths the Rust daemon, Coven Cave, and familiar workspaces create and own; must change when the daemon adds, renames, or retires state files."
---

# `COVEN_HOME` layout contract

`COVEN_HOME` (default `~/.coven`) is written by three different owners into one
directory: the Rust daemon, the Coven Cave desktop app, and the familiar
workspaces. This page says which paths belong to whom, what may be moved, and
how new files are named, so a long-lived install does not drift into ad-hoc
backups, stale locks, and per-workspace conventions.

**Rule zero: nothing under `COVEN_HOME` is deleted by any process this page
describes.** Every action is *classify*, *move with a manifest*, or *relocate
with a SHA-256 manifest*, and moves of shared or cold data need the operator's
explicit approval first. Disk pressure does not change this rule.

See also: [`OPERATIONAL-MODEL.md`](OPERATIONAL-MODEL.md) for the authority
boundary that makes the daemon the only writer of its own state, and
[`SETTINGS.md`](SETTINGS.md) for the configuration files that live here.

## 1. Ownership classes

Every top-level entry is exactly one of:

| Class | Created by | Rule |
|---|---|---|
| **daemon** | the Rust daemon and CLI | Names are fixed in code (`coven.sqlite3*`, `coven.sock`, `daemon.json`, `*.lock`, `daemon*.log`, `familiars.toml`, `repos.toml`, `config.toml`, `privacy.toml`, `adapters/`, `bin/`, `engine/`, `runtime/`, `defaults/`, `mobile/`). Clients and familiars never rename, move, or clean these. A lock or socket whose PID is dead is a **bug report**, not a cleanup task. |
| **cave** | Coven Cave | Everything Cave owns lives under `cave/`. No `cave-*` files at the top level; Cave's own home migration moves legacy ones in. Cave sweeps its own lock and `state.json.*` siblings. |
| **shared** | familiars and the operator | Human-readable knowledge and coordination. Each shared root carries a `README.md` stating owner, scope, layout, conventions, and recovery. |

Anything at the top level that is not in the daemon list, not `cave/`, and not a
documented shared root is **unclassified**. It stays where it is and appears in
the audit until an owner claims it or it is archived.

## 2. Shared roots

| Root | Owning lane | Purpose |
|---|---|---|
| `workspaces/familiars/<id>/` | that familiar | personal workspace (section 4) |
| `research/` | research | shared research tree |
| `knowledge/` | research / memory | knowledge packs served to clients |
| `journal/` | memory | daily journal |
| `memory/` | memory | shared memory stores |
| `skills/` | orchestration | shared skills |
| `library/` | utility | library assets |
| `handoffs/` | orchestration | cross-familiar handoffs, `YYYY-MM-DD-<slug>.md` |
| `repo-coordination/` | code | per-repository coordination state |
| `archives/` | memory | cold storage with manifests (section 3) |
| `backups/` | memory | large point-in-time snapshots: archive plus manifest |
| `logs/` | guardian | non-daemon logs, including `logs/home-audit/YYYY-MM-DD.md` |
| `tmp/` | anyone | the **only** top-level scratch; entries older than 14 days become cold-tier candidates |

A new top-level entry needs an owner and a `README.md` the day it is created.

## 3. Naming

- **In-place backup:** `<name>.bak-<YYYY-MM-DD>-<reason>`. At most two per
  file; older ones move to `archives/`.
- **Temp:** only inside a `tmp/` or `scratch/` directory. Never `*.tmp` or
  `*.<pid>.<hash>` next to the real file.
- **Archive bundle:** `archives/<YYYY-MM>/<source-path-slug>/` holding the
  moved content plus `MANIFEST.json` —
  `{from, to, moved_at, by, sha256: {relpath: hash}}` — and a `RELOCATED.txt`
  pointer left at the original location.
- **External relocation:** same manifest shape; `to` is a path on the archive
  volume. The operator approves each batch.
- **Dated files:** ISO date first — `YYYY-MM-DD-<slug>.ext` — so listings sort.

## 4. Familiar workspace template

**Required at the workspace root** (identity and runtime contract):
`AGENTS.md SOUL.md IDENTITY.md USER.md TOOLS.md CAPABILITIES.md COMMUNICATION.md
MEMORY.md HEARTBEAT.md DREAMS.md ward.toml ward.toml.v01.bak` plus
`memory/ notes/ skills/ roles/ avatars/`.

**Standard working directories — one name per idea:**

| Directory | Holds | Absorbs |
|---|---|---|
| `artifacts/` | finished deliverables, grouped by topic | `deliverables/ output/ reports/ publications/` |
| `briefs/` | plans, proposals, specs | `plans/ specs/ drafts/` |
| `handoffs/` | dated handoff documents | |
| `self-reports/` | dated self-reports | |
| `evidence/` | screenshots, logs, receipts that prove a claim | `receipts/ shots/ *-evidence/` |
| `media/` | images and video that are *inputs* | image-only `assets/` |
| `tools/` | scripts and helpers the familiar maintains | `scripts/ bin/` |
| `scratch/` | disposable working files; git-ignored; cold after 14 days | `tmp/ tmp-* tmp.* .scratch/` |
| `projects/<name>/` | lane-specific domain work | ad-hoc topic directories at the root |

**Declared directories.** A workspace may keep other top-level directories only
if its `AGENTS.md` has a `## Workspace layout` section listing each one with a
one-line purpose (for example `state/` for tool credentials, `assets/` for data
a skill reads, `repos/` for read-only clones). Undeclared extras are drift.

**Rules**

- **Root budget: at most 20 files** at the workspace root.
- **Ignore baseline:** `.DS_Store`, `scratch/`, `state/`, `repos/`,
  `.worktrees/`, `.env*`, `.secrets/`, `node_modules/`, `vendor/`,
  `__pycache__/`, runtime-owned hidden directories, nested clones with their
  own remotes, and `*.png *.jpg *.jpeg *.mp4 *.mov` except `avatars/**`.
  Before the first commit after adopting, run `git add -n -A | wc -l`; a
  four-digit number means a vendored tree slipped through.
- **Commit at least weekly** so a dirty tree is a signal again.
- **Large binaries** (over 50 MB, not an avatar): `artifacts/` with a sidecar
  `.md`, or `archives/` via manifest.
- **`MEMORY.md` backups:** at most two in place.
- **Moving files:** leave `notes/YYYY-MM-DD-<slug>-index.md` mapping old to new
  paths, so dated memory notes that cite old paths stay resolvable.

## 5. Retention tiers

| Tier | Meaning | Action |
|---|---|---|
| **live** | daemon state, current workspaces, READMEs, memory | never touched |
| **warm** | conversations, research missions under 90 days, verification evidence for open issues | stays |
| **cold** | verification evidence for closed issues, worktree archives, missions over 90 days, `tmp/` over 14 days, trash over 30 days, `.bak-*` beyond the newest two | **proposed** for archive or relocation; operator approves |
| **bug** | dead-PID locks and sockets, orphan `*.tmp`, duplicate `state.json.*` | issue to the code lane; never hand-cleaned |

## 6. Process

1. **Audit** — the guardian lane runs a read-only drift report weekly and writes
   it to `logs/home-audit/YYYY-MM-DD.md`.
2. **Propose** — cold candidates become `handoffs/YYYY-MM-DD-home-cold-tier.md`
   with size and a one-line reason each.
3. **Approve** — the operator, per line or per section.
4. **Move** — the utility lane executes with section 3 manifests. **Verify** —
   the memory lane checks hashes and maintains `archives/INDEX.md`.
5. **Fix in code** — daemon log rotation, Cave sibling sweeps, and an eventual
   `coven home audit` subcommand belong in this repository and Coven Cave, not in
   hand-run cleanup.

## Changelog

- v1 — first contract. Added `tools/`, `projects/`, and the declared-directories
  rule after the first worked example showed every workspace needs them; added
  vendored trees and runtime hidden directories to the ignore baseline after a
  first adoption commit swept in about 2,500 vendored files.
