# coven-afs mount spike — results and go/no-go

**Status:** Spike complete · 2026-08-08 · bead `coven-110`
**Follows:** [RESEARCH.md](./RESEARCH.md) next step 2 · [DESIGN.md](./DESIGN.md) §3, §7
**Code:** `crates/coven-afs/src/ino.rs`, `src/nfs.rs`, `examples/afs_serve.rs`,
`examples/afs_bench.rs`

RESEARCH.md called the throughput question unmeasured anywhere and said it
gates the architecture. This spike measures it, and exercises the kext-free
macOS mount path end to end.

**Verdict: GO on the storage engine, conditional on write-ahead logging.
The macOS loopback NFS path is validated for a consent-enabled,
human-operated Terminal/client.**

---

## 1. What was built

- **Inode-addressed operations** (`src/ino.rs`). The existing API is
  path-addressed; NFS and FUSE both address objects by file id and
  `(parent, name)`. Adds lookup, readdir with inode-ordered pagination,
  offset read/write, truncate, setattr, create/mkdir/symlink/remove/rename.
  No schema change — same SPEC v0.4 tables, different query shape.
- **`write_ino_at`**, the operation a mount stands or falls on. A client
  writes a file in ~64 KiB pieces, so rewriting the whole file per write would
  make an N-byte file cost O(N²). Only the touched chunks are rewritten; holes
  are zero-filled because reads reconstruct a file by chunk index.
- **NFSv3 export** (`src/nfs.rs`, feature `mount`) over `nfsserve` 0.11
  (BSD-3-Clause, HuggingFace — the crate AgentFS itself uses).
- **`AgentFs::enable_wal`**, opt-in write-ahead logging. §3 explains why this
  turned out to be the whole ballgame.
- **Benchmarks** (`examples/afs_bench.rs`) and a serving/import harness
  (`examples/afs_serve.rs`, `AFS_IMPORT=<dir>`).

## 2. Storage throughput

macOS 15 / arm64, release build, 2000 files per shape, one run.
`afs (wal)` is the same workload with `enable_wal()`; ratios are against the
host filesystem, lower is better.

### Checkout shape — 2000 files, 13.0 MiB, source-tree sizes

| Operation | host | afs (default) | afs (wal) |
|---|---|---|---|
| write | 241.1 ms (53.8 MiB/s) | 2130.3 ms — **8.84x** | 311.3 ms (41.7 MiB/s) — **1.29x** |
| read | 180.3 ms (72.0 MiB/s) | 1180.4 ms — **6.55x** | — |
| stat | 6.0 ms | 56.5 ms — **9.37x** | — |

Database is 15.2 MiB for 13.0 MiB of payload (**1.17x**).

### Package-tree shape — 2000 files, 1.0 MiB, `node_modules` sizes

| Operation | host | afs (default) | afs (wal) |
|---|---|---|---|
| write | 244.1 ms | 1669.1 ms — **6.84x** | 197.2 ms — **0.81x** |
| read | 914.6 ms | 831.6 ms — **0.91x** | — |
| stat | 6.5 ms | 66.6 ms — **10.29x** | — |

Database is 1.4 MiB for 1.0 MiB of payload (**1.51x**).

With WAL, the `pnpm install`-shaped workload is **faster than the host
filesystem** on both write (0.81x) and read (0.91x). Thousands of tiny files
are one SQLite transaction stream instead of thousands of create/write/close
syscall triples, and that trade favours the database.

### Large file — 64 MiB written in 64 KiB pieces (WAL)

| host | afs |
|---|---|
| 49.1 ms (1302 MiB/s) | 359.3 ms (178 MiB/s) — **7.31x** |

178 MiB/s is the chunked write path a mount client drives. Adequate; it is
also the number to re-check if `chunk_size` is ever tuned away from 4096.

### Copy-up — first write to a file that lives in the read-only base (WAL)

| Base file size | First-write cost |
|---|---|
| 1 MiB | 3.3 ms |
| 8 MiB | 57.1 ms |
| 32 MiB | 238.0 ms |

Linear, ~135–300 MiB/s. RESEARCH.md's concern is confirmed and bounded:
irrelevant for source files, painful for large binaries. **This validates the
`afs.copy_up_max_bytes` cap and the default ingest excludes in DESIGN.md §7.**
A cap in the low tens of MiB keeps worst-case first-write latency under
~250 ms; `target/`, `node_modules/`, and `.git/objects/` should stay out of
the base regardless.

Directory ingest, for scale: 138 files / 4.8 MB imported in 181 ms.

## 3. Write-ahead logging is not optional

The engine as merged uses SQLite's default rollback journal with
`synchronous=FULL`, so every `write_file` is its own fully-synced
transaction. That is why the default column reads 6.8–8.8x. The host baseline
does buffered writes and does not fsync per file — which is also what `git
checkout` and `pnpm install` actually do, so it is the honest comparison.

Enabling WAL with `synchronous=NORMAL` closes almost all of that gap
(8.84x → 1.29x; 6.84x → 0.81x). A mounted session overlay is scratch state
whose durability story is "discard or commit", so relaxed sync is the right
trade there.

It is **opt-in rather than the default** because WAL costs a property this
design values: a WAL database is three files while open (`-wal`, `-shm`)
rather than the single portable file that makes a delta copyable. They are
checkpointed away on clean close. The daemon should call `enable_wal()` when
it opens a session delta; a database being handed to someone else should not.

## 4. The macOS mount: correct, and now manually confirmed

What works, verified against a live server with RPC tracing:

- An **unprivileged** `mount_nfs` succeeds — no `sudo`, no kext.
  `mount` reports `localhost:/ on … (nfs, nodev, nosuid, mounted by buns)`.
- The kernel client drives the export correctly: MOUNT, LOOKUP, ACCESS,
  GETATTR, READDIRPLUS and FSSTAT all succeed. ACCESS returns `63` (all
  rights). `stat` through the mount returns real imported data — correct
  size, correct owner.
- Spotlight walks the mounted tree without trouble, which is the clearest
  proof the export is well-formed.

What does not work for an automated agent shell: **`open()` returns `EPERM`** —
for files (`cat`) and directories (`ls`) alike — while `stat` on the same path
succeeds and the server logs no error. Metadata passes, `open` is refused, and
the refusal happens client-side without an RPC.

That pattern is a macOS privacy/TCC restriction on network volumes for the
calling process, not an NFS or permission fault:

- it is not privilege — reads and writes fail identically, and the mount
  itself succeeded unprivileged;
- it is not ownership — inodes are presented as owned by the serving user
  (SPEC defaults `uid`/`gid` to 0, which made the client reject everything
  until `src/nfs.rs` mapped unowned inodes to the serving user; that fix is
  in and `ls -ldn` now shows the correct owner);
- it is not the server — the same operations succeed for Spotlight, and
  ACCESS grants every right.

The manual confirmation path passed from a consent-enabled, human-operated Terminal/client, with `afs_serve` listening on `127.0.0.1:12049`:

```sh
cargo run -p coven-afs --features mount --example afs_serve -- /tmp/afs.db 12049 &
mkdir -p /tmp/afsmnt
mount_nfs -o vers=3,tcp,port=12049,mountport=12049,nolock,soft localhost:/ /tmp/afsmnt
mkdir /tmp/afsmnt/hello && echo written > /tmp/afsmnt/hello/file.txt && cat /tmp/afsmnt/hello/file.txt
umount -f /tmp/afsmnt
```

The `mount_nfs` client mounted `localhost:/` at `/private/tmp/afsmnt` and
`mount` reported NFS with `nodev` and `nosuid`. `mkdir /tmp/afsmnt/hello`, the
write to `file.txt`, and the read-back of `written` all succeeded. The earlier
`Connection refused` was just startup timing: Cargo/server startup had not
completed yet. The later local write happened while unmounted and is not NFS
evidence.

This is consistent with a per-process macOS privacy/TCC requirement such as
Network Volumes or Full Disk Access for the client or agent process. Do not
read that as universal sufficiency in every deployment context; treat it as a
process-consent requirement to document and re-check per harness.

Linux/FUSE was **not** evaluated: this spike ran on macOS, and `fuser` needs a
Linux host to mean anything. The inode API in §1 is the layer a `fuser`
backend would bind to, so that work is unblocked but unstarted.

## 5. Go/no-go

**GO on the storage engine.** The throughput question that gated the
architecture is answered: with WAL, an agent-shaped workload runs at
0.81–1.29x the host filesystem for whole-file writes, faster than the host for
dependency-tree shapes, at ~1.2–1.5x on-disk overhead. Copy-up is linear and
capped by policy. Nothing here argues against the design in DESIGN.md.

**Consent-enabled mount validation may proceed.** The NFS export is
protocol-correct and the human-operated Terminal/client path now has a manual
write/read confirmation. Keep the SDK-only path (`afsMount: false` in
DESIGN.md §3.1) as the safe default until the remaining gates below close.

**Two limits to remember.** The export serializes all RPCs behind one mutex
over a single SQLite connection, so nothing here measures parallel clients.
And loopback NFS is still unauthenticated — DESIGN.md §7's access-control
question is untouched by this spike and still gates enabling mounts by
default.

**Remaining gates.** Unauthenticated loopback access control, one SQLite
connection/mutex and untested parallel clients, recovery/default-enable
policy, sandboxing, and Linux/FUSE remain unresolved.

## 6. Reproducing

```sh
cargo run --release -p coven-afs --example afs_bench -- --scale 2000 --json bench.json
cargo test -p coven-afs --features mount --locked
AFS_IMPORT=<dir> cargo run -p coven-afs --features mount --example afs_serve -- <db> <port>
```
