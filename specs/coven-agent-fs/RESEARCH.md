# Native OpenCoven Agent File System (AFS) — Research Brief

Deep-research run, 2026-08-03 (Sage). Ad-hoc mission; ledger inline below.
Confidence: **high** = inspected primary source; **medium** = primary w/ caveats or triangulated; **inference** = our synthesis.

## Executive summary

1. **Turso AgentFS is a small, fully-reimplementable design** — one SQLite file holding a POSIX-ish inode FS (fs_inode/fs_dentry/fs_data 4KB chunks), a CoW overlay (whiteouts + fs_origin), an insert-only tool_calls audit table, and a KV store. MIT, Rust, BETA (0.2.x). [S1][S2][S6] (high)
2. **Copy-up is full-file, not block-level** — first write to a base file copies the whole file into the delta. Fine for code files; bad for huge binaries. [S3][S6] (high)
3. **macOS mounting without kexts is a solved problem**: AgentFS serves NFSv3 in userspace via HuggingFace's `nfsserve` crate and uses macOS's native `mount_nfs`. Linux uses `fuser` (FUSE3). FSKit (macOS 15+) is the strategic future path but immature. [S5][S7][S12] (high)
4. **The design space splits into four families** — SQLite VFS (AgentFS), microVM snapshots (E2B/Firecracker), CoW filesystems (ZFS/btrfs), and convention-only isolation (git worktrees, what OpenCoven uses today). Only the SQLite family gives SQL-queryable audit + single-file portability + cross-platform mounts. [S8–S11] (medium)
5. **git worktrees' known weakness is exactly OpenCoven's pain**: isolation is conventional, not enforced — nothing stops an agent writing outside its worktree (cf. the GitHub Desktop worktree destruction incidents in coven-cave). An AFS overlay closes that hole. [S4][S11] (high fact, inference re: fit)
6. **Recommendation: build native, spec-compatible.** Implement a `coven-afs` crate inside the coven daemon (already Rust + rusqlite 0.40), adopting AgentFS SPEC v0.4's schema as the base contract, extending it with coven-native audit (session/familiar/bead provenance). Reuse `fuser` + `nfsserve` for mounts. Do NOT adopt the Turso SDK as a dependency — the spec is the valuable artifact, the code is beta. (inference)

## Findings per RQ

### RQ1 — How does Turso AgentFS actually work?

- Three interfaces over one SQLite DB: virtual FS, KV store, tool-call audit. Root = ino 1; hard links + symlinks supported; chunked BLOBs (`fs_data`, chunk_size immutable, default 4096). [S6] (high)
- Overlay: delta DB over read-only host base; lookup order delta → whiteout → base; `fs_whiteout(path, parent_path)` indexed for O(1) directory listing; `fs_origin(delta_ino → base_ino)` preserves kernel-cached inode numbers across copy-up — a real bug they hit and solved, worth stealing. [S6] (high)
- Audit: `tool_calls` insert-only (name, params/result JSON, error, timings); MUST NOT be updated/deleted; extension points for session_id, cost, nesting, tokens. [S6] (high)
- Runtime: `agentfs run` = overlay + Linux namespaces (unshare) sandbox; sessions named + joinable; `agentfs diff`/`timeline` for inspection; discard = delete the .db. [S2][S3] (high)
- Naming discrepancy: overlay *guide* shows `fs_block` + whiteout column; SPEC v0.4 (authoritative) uses `fs_data` + `fs_whiteout` table. Follow the SPEC. [S3][S6] (medium)

### RQ2 — What are the alternative designs?

See comparison table. Notables beyond the table: EdenFS/ProjFS prove lazy-materialization daemons scale to monorepos [S13]; Firecracker snapshot-restore is 5–30ms and captures RAM+disk, but is Linux-server-shaped, not desktop-shaped [S9]; Letta memory blocks are a semantic (non-POSIX) alternative — complementary, not competing [S14].

### RQ3 — What do agent harnesses actually need?

From the surveyed products (high, per-source): (a) POSIX view so git/grep/existing tools work with zero integration [S2]; (b) enforced write boundaries, not conventions [S4][S11]; (c) queryable provenance — what changed, which tool call caused it [S6][S15]; (d) cheap branch/discard per session [S3][S16]; (e) portability of a whole agent run as one artifact [S1].

### RQ4 — Right native architecture for OpenCoven? (inference, grounded in above)

- **`coven-afs` crate** in the coven workspace, owned by the daemon. rusqlite 0.40 already bundled — zero new storage deps.
- **Schema: AgentFS SPEC v0.4 as baseline** (fs_config/inode/dentry/data/symlink, whiteouts, origin, kv, tool_calls), plus coven extension tables: `afs_provenance(op → session_id, familiar, bead_id, turn)` joining file mutations to coven's existing session events. Spec-compatibility keeps `agentfs` CLI/tooling usable against our DBs for free.
- **Mounts**: `fuser` on Linux, `nfsserve` on 127.0.0.1 for macOS (same as AgentFS — proven, kext-free). Windows: SDK-only initially (AgentFS also has no Windows mount).
- **API surface**: extend `coven.daemon.v1` socket API — `afs.session.create/join/diff/discard/commit`, `afs.mount`, `afs.timeline`. Cave desktop renders diff/timeline as a UI surface.
- **Workflow fit**: AFS session per agent run layered over the project root; "commit" materializes the delta into a real git worktree/branch for the existing PR pipeline — AFS replaces the risky live-worktree phase, not git itself.
- **Block-level CoW deferrable**: full-file copy-up (Turso's choice) is acceptable v1; chunked fs_data already bounds the write amplification for appends.

### RQ5 — Build vs adapt?

- **Adopt the spec, not the code.** SPEC v0.4 is ~22KB, MIT, precise enough to reimplement in days; the CLI/SDK are BETA (0.2.x, repo created 2025-10). Depending on Turso's crate couples us to their Turso-DB direction and cloud sync. (medium/inference)
- Reuse verbatim: `fuser`, `nfsserve` crates (both battle-tested; AgentFS itself vendors their licenses). (high)
- Effort estimate: core FS + overlay ≈ the SPEC's operation list, all plain SQL; the daemon integration and Cave UI are the larger halves. (inference)

## Comparison table

| Dimension | AgentFS-style SQLite VFS | Firecracker/E2B | ZFS/btrfs | git worktrees (today) |
|---|---|---|---|---|
| CoW branch/discard | delta DB + whiteouts | VM snapshot clone | subvolume snapshot | branch only, no FS enforcement |
| Audit | insert-only SQL, per-op | none native | none native | commit-level only |
| Enforced isolation | overlay + sandbox | hardware VM | no | **no** |
| macOS desktop fit | ✅ userspace NFS | ⚠️ heavy | ❌ | ✅ |
| Portability | single .db | VM image | host FS bound | git remote |
| Fits coven daemon (Rust+SQLite) | ✅ native | ❌ | ❌ | current state |

## Open questions & conflicts

- **C1**: overlay guide (`fs_block`, whiteout column) vs SPEC (`fs_data`, whiteout table) — resolved in favor of SPEC, but confirm against CLI source before freezing our schema.
- Turso cloud-sync semantics (conflict resolution, audit preservation) unfetched — irrelevant if we build native, relevant if we ever interop.
- Windows mount story: nothing exists upstream; ProjFS is the plausible native route, unscoped.
- Performance of NFS-localhost vs FUSE for big `pnpm install`-scale writes: unmeasured anywhere; needs a spike benchmark.

## Recommended next steps

1. Bead: spike `coven-afs` — SPEC v0.4 schema in rusqlite + read/write/overlay ops + unit tests (no mount yet).
2. Bead: mount spike — `nfsserve` on macOS against the spike DB; benchmark a real repo checkout + build.
3. Design doc in the coven repo: daemon API surface + provenance extension tables + Cave diff/timeline UI.
4. Decide interop stance: freeze on "SPEC-compatible, coven-extended" and document the extension tables.

## Source ledger

- S1 Turso AgentFS introduction — https://docs.turso.tech/agentfs/introduction (fetched, full)
- S2 Announcement + FUSE blogs — https://turso.tech/blog/agentfs, https://turso.tech/blog/agentfs-fuse (fetched)
- S3 Overlay guide + blog — https://docs.turso.tech/agentfs/guides/overlay, https://turso.tech/blog/agentfs-overlay (fetched)
- S4 AgentFS README + FAQ — https://github.com/tursodatabase/agentfs (fetched; MIT, Rust, 3.3k★, created 2025-10-24, pushed 2026-06-03)
- S5 NFS guide — https://docs.turso.tech/agentfs/guides/nfs (fetched, full; NFSv3 on 127.0.0.1:11111)
- S6 SPEC.md v0.4 — https://github.com/tursodatabase/agentfs/blob/main/SPEC.md (fetched, full)
- S7 nfsserve — https://github.com/huggingface/nfsserve (triangulated; vendored license in AgentFS confirms use)
- S8 E2B — https://docs.e2b.dev, https://github.com/e2b-dev (search-synthesized)
- S9 Firecracker snapshots — https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md (search-synthesized, path verified)
- S10 btrfs/ZFS branching — https://btrfs.readthedocs.io + practitioner gists (search-synthesized)
- S11 git worktree agent patterns — Augment/MindStudio guides (search-synthesized)
- S12 macFUSE/FSKit/Fuse-T — https://github.com/macfuse/macfuse/wiki (FUSE Backends page), FSKit samples (search-synthesized)
- S13 EdenFS/VFSForGit — https://github.com/facebook/sapling, https://github.com/microsoft/VFSForGit (partly fetched)
- S14 Letta memory blocks — https://docs.letta.com (search-synthesized)
- S15 PROV-AGENT — https://arxiv.org/abs/2508.02866 (search-synthesized, URL verified)
- S16 Sandbox forking pattern — https://agentpatterns.ai/patterns/agent-design/sandbox-forking/ (search-synthesized)
