# Native OpenCoven Agent File System (AFS) — Design

**Status:** Draft v1 · 2026-08-08
**Companion to:** [RESEARCH.md](./RESEARCH.md) (deep-research brief, 2026-08-03)
**Implements:** RESEARCH.md next steps 3 and 4 — daemon API surface, provenance
extension tables, Cave surfaces, and the frozen interop stance.

This document specifies how the `coven-afs` storage engine becomes a product
surface: what the daemon exposes, how file operations join Coven's existing
session provenance, how a session's delta becomes a git branch for the existing
PR pipeline, and what Cave renders. It does **not** restate the filesystem
semantics — those are AgentFS SPEC v0.4 plus `crates/coven-afs`.

Scope boundaries:

- **In scope:** `coven.daemon.v1` API additions, extension schema, commit
  materialization, Cave read surfaces, interop freeze.
- **Out of scope:** productionizing the experimental mount spike, sandbox
  enforcement design, Windows mounts, cloud sync.

---

## 1. Interop stance (frozen)

Coven databases are **SPEC v0.4-compatible, coven-extended**. Three rules make
that concrete and testable:

**E1 — Spec tables are untouched.** Coven never adds, renames, drops, or
retypes a column in `fs_config`, `fs_inode`, `fs_dentry`, `fs_data`,
`fs_symlink`, `fs_whiteout`, `fs_origin`, `kv_store`, or `tool_calls`. SPEC v0.4
sanctions extension columns on `tool_calls` (session, cost, nesting, tokens);
we decline that allowance so a single rule covers every table.

**E2 — Extensions are additive tables named `afs_*`.** All Coven state lives in
new tables under the `afs_` prefix. `DROP TABLE` on every `afs_*` table must
leave a database whose filesystem semantics are unchanged — extensions carry
provenance and lifecycle metadata, never filesystem truth. Nothing in
`coven-afs`'s read/write path may consult an `afs_*` table to resolve a path,
inode, or byte.

**E3 — Both directions must work.** Upstream `agentfs` tooling opens and mutates
a Coven database correctly (E1 + E2 guarantee it). Coven opens a database
produced by upstream `agentfs` correctly: every `afs_*` table is created lazily
and every column in them is nullable or defaulted, so a foreign database is
simply one with no provenance yet. A missing `afs_session` row means "unbound
session", not an error.

The cost of E1–E3 is that a mutation made by upstream tooling produces no
provenance row. That is accepted and surfaced: see §4.4 (unattributed
mutations), which is why diff is computed from the filesystem tables and not
from the provenance log.

Interop is a **conformance test**, not a claim: CI creates a database with
`coven-afs`, runs the SPEC's consistency rules against it, drops every `afs_*`
table, and re-runs them.

---

## 2. On-disk layout

```
<COVEN_HOME>/afs/
  bases/<base-fingerprint>.db     read-only base snapshots, content-addressed
  sessions/<afs-session-id>.db    writable deltas (one per AFS session)
  mounts/<afs-session-id>/        mount point (macOS NFS / Linux FUSE)
```

A **base** is an immutable snapshot of a project root, identified by
`base-fingerprint`: a hash over the ingest inputs (project root path, git commit
if the root is a repository, ingest filter version). Bases are shared by every
session opened against the same fingerprint, so N concurrent agent sessions on
one repository cost one base plus N small deltas.

A **delta** is one SQLite database per AFS session, holding the writable overlay
and all `afs_*` provenance. Deltas are private to the same-user trust boundary:
`0o600`, inside `COVEN_HOME`, never written to the project root.

Discard deletes the delta file. Bases are reference-counted and swept when no
delta and no open mount refers to them.

---

## 3. Daemon API surface

AFS extends the existing `coven.daemon.v1` contract additively (see
[API versioning](../../docs/daemon/api-versioning.md)). Same-user local IPC only
— like session handoff, these routes must never be proxied to a remote
listener. A remote companion needs a separately paired transport.

### 3.1 Capability flags

`GET /api/v1/health` gains:

```json
{
  "capabilities": {
    "afs": true,
    "afsMount": "nfs",
    "afsCommit": true,
    "afsCommitDryRun": true
  }
}
```

`afs` gates the whole route family. `afsMount` is `"nfs"` (macOS), `"fuse"`
(Linux), or `false` when no mount backend is available — a client must branch on
it rather than assume mounting works, because SDK-only operation is a supported
mode. `afsCommit` is false when the daemon cannot materialize commits (no git,
or a read-only project root). `afsCommitDryRun` separately advertises the
side-effect-free preview contract; clients must not infer it from `afsCommit`
because commit support predates `dryRun`. Capabilities advertise availability
and never grant permission.

### 3.2 Operations

The RESEARCH.md operation names are the contract's vocabulary; each maps to one
route in the daemon's REST idiom.

| Operation | Route | Purpose |
|---|---|---|
| `afs.session.create` | `POST /api/v1/afs/sessions` | Ingest/reuse a base, create a delta, bind provenance. |
| `afs.session.list` | `GET /api/v1/afs/sessions` | List AFS sessions with state and change counts. |
| `afs.session.get` | `GET /api/v1/afs/sessions/:id` | Fetch one AFS session. |
| `afs.session.join` | `POST /api/v1/afs/sessions/:id/join` | Attach a second actor to an existing delta. |
| `afs.session.diff` | `GET /api/v1/afs/sessions/:id/diff` | Change set, or one path's unified diff. |
| `afs.session.commit` | `POST /api/v1/afs/sessions/:id/commit` | Materialize the delta into a git branch. |
| `afs.session.discard` | `POST /api/v1/afs/sessions/:id/discard` | Unmount and delete the delta. |
| `afs.mount` | `POST /api/v1/afs/sessions/:id/mount` | Mount the merged view; `DELETE` unmounts. |
| `afs.timeline` | `GET /api/v1/afs/sessions/:id/timeline` | Cursor-paginated provenance + tool calls. |

`discard` is a POST rather than `DELETE /afs/sessions/:id` deliberately: it is a
destructive operation whose body carries an explicit `confirm` and an optional
`retainAudit`, and it must be distinguishable in logs from an idle cleanup.

### 3.3 Shapes

**Create.**

```http
POST /api/v1/afs/sessions
{
  "projectRoot": "/workspace/project",
  "sessionId": "01J...",         // optional: bind to a Coven session
  "beadId": "coven-vhw",         // optional
  "name": "vhw-design"           // optional: joinable handle, unique while open
}
```

```json
{
  "id": "01JAFS...",
  "name": "vhw-design",
  "state": "open",
  "base": { "fingerprint": "sha256:…", "commit": "74d6207", "ingestedAt": "…" },
  "binding": { "sessionId": "01J…", "familiarId": "…", "beadId": "coven-vhw" },
  "mount": null,
  "changes": { "added": 0, "modified": 0, "deleted": 0, "bytes": 0 }
}
```

**Mount.** `POST …/mount` returns the mount point and backend; the response
never includes a listener address a caller could hand to another process.

```json
{ "mountPoint": "<COVEN_HOME>/afs/mounts/01JAFS…", "backend": "nfs", "readOnly": false }
```

**Diff.** `GET …/diff` returns the change set; `GET …/diff?path=src/main.rs`
returns one unified diff, truncated at a documented byte cap with
`"truncated": true` rather than streaming an unbounded body.

```json
{
  "changes": [
    { "path": "src/main.rs", "change": "modified", "bytes": 1841, "ino": 42, "baseIno": 17, "mode": 33188 },
    { "path": "docs/new.md",  "change": "added",    "bytes": 210,  "ino": 43, "baseIno": null, "mode": 33188 },
    { "path": "old.txt",      "change": "deleted",  "bytes": 0,    "ino": null, "baseIno": 9,  "mode": null }
  ],
  "truncated": false
}
```

The path-specific response is:

```json
{
  "path": "/src/main.rs",
  "patch": "--- /src/main.rs\n+++ /src/main.rs\n@@ ...",
  "truncated": false,
  "binary": false
}
```

`patch` is capped at 262,144 bytes during generation and is truncated only on
a UTF-8 boundary. Added and deleted text content uses `/dev/null` for the
missing side. A metadata-only empty-file change has no text hunk, so its patch
is empty and its change kind remains authoritative in the change-set response.
Differing non-UTF-8 content returns the exact marker
`"Binary files differ\n"` with `binary: true`. A missing path returns
`afs.path_not_found`; a directory or symlink returns `afs.path_not_file`.

**Timeline.** Cursor-paginated on `afs_provenance.seq`, matching the daemon's
existing `eventCursor: "sequence"` idiom: `?since=<seq>&limit=<n>`, newest-last,
`nextCursor` echoed in the response. Each provenance row retains
`toolCallId` and includes the linked audit record when present:

```json
{
  "entries": [{
    "seq": 17,
    "op": "write",
    "path": "/src/main.rs",
    "toolCallId": 42,
    "toolCall": {
      "id": 42,
      "name": "write_file",
      "parameters": "{\"path\":\"/src/main.rs\"}",
      "result": "{\"bytes\":1841}",
      "error": null,
      "startedAt": 1786320000,
      "completedAt": 1786320001,
      "durationMs": 1000
    }
  }],
  "nextCursor": 17,
  "hasMore": false
}
```

The daemon redacts `parameters`, `result`, and `error` before serialization.
Missing and dangling audit references serialize as `toolCall: null` without
dropping the provenance row.

**Commit.**

```http
POST /api/v1/afs/sessions/:id/commit
{ "branch": "afs/vhw-design", "message": "feat: …", "dryRun": false }
```

```json
{
  "branch": "afs/vhw-design",
  "commit": "9f2c…",
  "worktree": "/workspace/project/.worktrees/afs-vhw-design",
  "applied": { "added": 3, "modified": 7, "deleted": 1 },
  "state": "committed"
}
```

### 3.4 Error codes

Dotted codes in the standard envelope (see
[Error envelope](../../docs/daemon/error-envelope.md)):

| Code | Meaning |
|---|---|
| `afs.session_not_found` | Unknown or already-discarded AFS session. |
| `afs.session_not_open` | Operation requires `state = open`. |
| `afs.name_in_use` | Another open session holds that joinable name. |
| `afs.path_not_found` | The requested path exists in neither the base nor merged view. |
| `afs.path_not_file` | The requested path is a directory, symlink, or other non-regular node. |
| `afs.mount_unsupported` | No mount backend on this platform (`afsMount: false`). |
| `afs.mount_busy` | Already mounted, or the mount point is not empty. |
| `afs.base_diverged` | Project root moved off the recorded base commit. |
| `afs.commit_conflict` | Target branch exists with unrelated content. |
| `afs.path_outside_root` | A delta path would materialize outside the repository root. |
| `afs.copy_up_too_large` | A single copy-up exceeds the configured byte cap. |
| `afs.commit_unsigned` | Commit signing is required but unavailable. |
| `afs.unavailable` | The operation failed for a reason with no more specific code. |

`afs.base_diverged` and `afs.path_outside_root` fail closed: neither is
retried, downgraded, or partially applied.

This table is the whole contract. A client branches on these codes, so an
operation that can fail must map onto one of them — `afs.unavailable` is
listed because the daemon does emit it, not because a caller should expect
it. A new failure mode earns a code here before it reaches a client, since
an undocumented code is one no surface handles.

---

## 4. Provenance extensions

### 4.1 `afs_session` — binding and lifecycle

One row per delta database.

```sql
CREATE TABLE IF NOT EXISTS afs_session (
  id                TEXT PRIMARY KEY,
  name              TEXT,
  state             TEXT NOT NULL,          -- open | committing | committed | discarded
  base_fingerprint  TEXT NOT NULL,
  base_commit       TEXT,                   -- git commit of the base, when the root is a repo
  project_root      TEXT NOT NULL,
  coven_session_id  TEXT,                   -- sessions.id in the coven store
  familiar_id       TEXT,                   -- sessions.familiar_id
  bead_id           TEXT,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);
```

### 4.2 `afs_provenance` — file operations joined to Coven identity

```sql
CREATE TABLE IF NOT EXISTS afs_provenance (
  seq               INTEGER PRIMARY KEY AUTOINCREMENT,
  op                TEXT NOT NULL,          -- write|truncate|mkdir|rmdir|unlink|rename|symlink|hardlink|copy_up
  path              TEXT NOT NULL,
  to_path           TEXT,                   -- rename destination
  ino               INTEGER,                -- delta inode after the op
  base_ino          INTEGER,                -- fs_origin base inode when copy-up occurred
  bytes             INTEGER NOT NULL DEFAULT 0,
  at                INTEGER NOT NULL,       -- unixepoch seconds
  at_nsec           INTEGER NOT NULL DEFAULT 0,
  afs_session_id    TEXT NOT NULL,
  coven_session_id  TEXT,
  familiar_id       TEXT,
  bead_id           TEXT,
  turn              INTEGER,                -- events.rowid cursor at the time of the op
  tool_call_id      INTEGER REFERENCES tool_calls(id)
);

CREATE INDEX IF NOT EXISTS idx_afs_provenance_path ON afs_provenance(path, seq);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_session ON afs_provenance(coven_session_id, seq);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_bead ON afs_provenance(bead_id, seq);
CREATE INDEX IF NOT EXISTS idx_afs_provenance_tool_call ON afs_provenance(tool_call_id);
```

`tool_call_id` is a real foreign key: `tool_calls` lives in the same delta
database. `coven_session_id`, `familiar_id`, `bead_id`, and `turn` are
**by-value** references into the daemon store (`<COVEN_HOME>/coven.db`) — a
different SQLite file, so no constraint can enforce them. That is deliberate:
the delta must stay self-describing when copied off the host, and a delta whose
originating session was pruned must still read.

**Why the identity columns repeat per row instead of living only in
`afs_session`:** `afs.session.join` exists, so more than one actor can write to
one delta. The acting session, familiar, bead, and turn are properties of the
*operation*, not of the delta. `afs_session` records who opened it;
`afs_provenance` records who did each thing.

`turn` is the session's event cursor (`MAX(events.rowid)` for that session) at
the moment of the operation — the same cursor session handoff fences on, so a
timeline entry can be lined up against a transcript position without a second
clock.

### 4.3 `afs_commit` — materialization record

```sql
CREATE TABLE IF NOT EXISTS afs_commit (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  branch                 TEXT NOT NULL,
  commit_sha             TEXT,
  worktree_path          TEXT,
  provenance_high_water  INTEGER NOT NULL,  -- afs_provenance.seq covered by this commit
  state                  TEXT NOT NULL,     -- planned | committed | failed
  created_at             INTEGER NOT NULL
);
```

`provenance_high_water` makes a commit answerable: every operation at or below
that `seq` is represented in that commit, and a later commit on the same delta
covers the open interval above it.

### 4.4 Unattributed mutations

Writes that arrive without a bound actor — upstream `agentfs` tooling, a manual
`sqlite3` edit, a mount write from a process the daemon did not launch — produce
either no provenance row or a row with null identity columns. Diff therefore
never reads `afs_provenance`; it is computed from `fs_dentry`, `fs_origin`, and
`fs_whiteout`, which cannot lie about what the filesystem contains. The timeline
reports its own completeness by comparing the change set against provenance
coverage and marking uncovered paths `attribution: "unknown"`. A UI that showed
a clean timeline while the diff carried unexplained files would be worse than
no timeline.

---

## 5. Commit materialization

Commit turns a delta into an ordinary git branch. It adds no new review
machinery: the branch enters the existing PR pipeline described in
[AGENTS.md](../../AGENTS.md).

1. **Quiesce.** `state → committing`; reject writes; unmount if mounted.
2. **Verify the base.** The project root's HEAD must still be `base_commit`;
   otherwise `afs.base_diverged`. The delta is preserved, not discarded — the
   operator rebases the base or commits onto a fresh worktree.
3. **Create the worktree.** `git worktree add -b <branch> <path> <base_commit>`,
   following the repo's worktree convention. Branch defaults to
   `afs/<session-name-or-id>`.
4. **Apply the change set.** Added and modified paths are written from the
   delta; whiteouts become removals. Only the executable bit of `mode` is
   carried across — AFS mode bits are not a git concept.
5. **Refuse escapes.** Any path that normalizes outside the repository root,
   any write under `.git/`, and any symlink whose target escapes the root fail
   the whole commit with `afs.path_outside_root`. Materialization is
   all-or-nothing; a partially applied delta is never left on a branch.
6. **Commit, signed.** The repo requires verified commits, so materialization
   signs (`-S`) using the operator's existing git signing configuration and
   fails with `afs.commit_unsigned` if signing is unavailable or fails. Coven
   never disables signing to make a commit land.
7. **Attribute.** Trailers carry the provenance the branch would otherwise
   lose: `Coven-Session`, `Coven-Familiar`, `Coven-Bead`, `Coven-Afs-Session`,
   plus `Co-authored-by:` lines in the numeric-id no-reply form required by
   AGENTS.md.
8. **Record.** Insert `afs_commit`; `state → committed`. The delta survives
   commit — it is the audit record — until an explicit discard.

Commit does not push, open a PR, or run CI. Those stay where they are.

---

## 6. Cave surfaces

Cave reads AFS exclusively through the daemon routes above. It holds no SQLite
handle on a delta and reimplements no diff or overlay logic; the Rust authority
boundary applies unchanged.

**Filesystem pane** (per session, behind the `afs` capability):

- **Changes** — the `afs.session.diff` change set as a tree, with per-file
  added/modified/deleted badges and byte counts. Selecting a file fetches that
  path's unified diff on demand; the truncation flag renders as an explicit
  "diff truncated" affordance, never as a silently short diff.
- **Timeline** — `afs.timeline` merged into one ordered list: file operations
  and the tool calls that caused them, grouped by turn, each entry linking back
  to its session event. Rows whose `attribution` is `"unknown"` are marked, not
  hidden.
- **Commit** — branch name, the change counts that would be applied, and a
  `dryRun` preview. Commit is an explicit operator action; Cave never
  materializes automatically.

Degraded states are first-class. `afs: false` hides the pane; `afsMount: false`
shows changes and timeline but disables mount controls; `afsCommit: false`
disables the commit action with the reason from the capability payload.

---

## 7. Operational semantics

**Copy-up cost.** RESEARCH.md flags full-file copy-up as the acceptance risk. A
configurable per-file cap (`afs.copy_up_max_bytes`) fails the write with
`afs.copy_up_too_large` rather than silently absorbing a multi-gigabyte artifact
into a delta. Ingest filters exclude the obvious offenders (`target/`,
`node_modules/`, `.git/objects/`) from the base by default. The cap's default
value is set from the `coven-110` mount benchmark, not guessed here.

**Orphan recovery.** A delta whose daemon died stays `open` with a stale mount.
Startup sweeps `<COVEN_HOME>/afs/sessions/`, unmounts anything mounted by a dead
pid, and leaves the delta intact — the same posture as
[orphan recovery](../../docs/daemon/orphan-recovery.md) for sessions. Deltas are
never auto-discarded; unreviewed work is not garbage.

**Loopback NFS exposure (decided 2026-08-09, `coven-75e`).** The macOS backend
serves NFSv3 on loopback. Any local user can reach a loopback port, so a bare
port grants a second account on the machine read/write access to a session's
files.

Two findings changed the mitigation set this section originally specified, and
both are worth recording because each defeats the obvious design:

1. **An export-path token cannot live in the MOUNT protocol.**
   `MOUNTPROC3_EXPORT` returns the export path to any caller with no
   authentication, so a token carried there is readable by exactly the
   attacker it is meant to stop.
2. **The token was not the weak link.** `nfsserve`'s default file handle is
   `[server start time in ms ‖ file id]`. The start time is readable from
   `ps` and `fs_inode.ino` is a sequential `AUTOINCREMENT` from `ROOT_INO`, so
   handles are forgeable. A forged handle needs no MOUNT call at all, which
   bypasses every path-based check.

What ships instead:

- **Authenticated file handles.** `[file id ‖ HMAC-SHA256(key, file id)]`
  truncated to 128 bits, under a 256-bit per-export key from OS entropy. A
  forged or stale handle is `NFS3ERR_BADHANDLE`; the two are deliberately
  indistinguishable, because separating them would confirm which file ids
  exist. The per-export key also gives restart invalidation without a
  guessable generation number.
- **A VFS token gate.** The export roots at a synthetic gate directory rather
  than at the filesystem. Its `readdir` returns nothing and its `lookup`
  succeeds only on a constant-time match against a 128-bit token, so the token
  lives where no NFS procedure enumerates it. Clients mount
  `localhost:/<token>`.
- **Loopback-only bind**, refused in library code rather than by convention,
  on an **ephemeral port** per session.
- **Token rotation after mount.** `mount_nfs` takes the export path as a
  positional argument and offers no stdin, environment, or config alternative
  (`man mount_nfs` has neither an ENVIRONMENT nor a FILES section), so the
  token is `ps`-visible for the duration of that one call. The daemon rotates
  the gate the moment the mount returns, which makes a scraped value useless.
  Established mounts are unaffected: the client holds an authenticated file
  handle, handles are keyed on the file id under a key rotation does not
  touch, and only a *new* traversal of the gate needs the current token.

**Decision.** This is sufficient to keep an unprivileged local account out by
default, and it is *not* sufficient to survive disclosure of the token. An
attacker can still find the port, learn the export path, and mount the gate —
they arrive at an empty directory and must produce 128 bits to go further.
There is no second factor behind the token: NFSv3 `AUTH_UNIX` authenticates
nothing. Mount therefore remains **opt-in and off by default**; `afsMount`
advertises a backend only where these mitigations are active.

> **Invariant: the export token is never logged, printed, or displayed.**
> Token secrecy is the entire access-control boundary, so anything that
> records it — a log line, an error message, a `ps`-visible mount command, a
> Cave surface, a bug report attachment — grants full read/write to the
> session's files. Treat it like a credential, because it is one. Code that
> needs to show a mount target shows the port and a redacted path.

On 2026-08-09, a human-operated consent-enabled Terminal mounted the
experimental loopback NFS export and completed mounted create/write/read-back.
The earlier automated-agent EPERM remains meaningful process-level macOS
privacy/TCC behavior. Access and consent must be assessed for each
client/harness or deployment; this does not claim universal requirement, and
it does not say Full Disk Access guarantees access. This result does not
change default-off status or resolve loopback NFS access control, sandboxing,
recovery, concurrency, or Linux/FUSE gates.

Also on 2026-08-09, the same human-operated Terminal confirmed rotation on a
live mount: with the export rotated out from under an established mount, reads,
directory listings, and writes through that mount continued to work, while a
fresh `mount_nfs` with the pre-rotation token was refused and one with the
current token succeeded. macOS's NFSv3 client does not re-resolve the export
path after mount. The server-side half of that property — a captured handle
outliving rotation — is covered by a unit test, so a regression fails CI rather
than waiting on a manual check.

On 2026-08-10, the `scripts/afs-mount-smoke.sh` probe run from an automated
agent shell on a maintainer's machine completed every stage: mount, readdir,
read, and write through the mount. That is the operation MOUNT-SPIKE.md §4
recorded as `EPERM` for an automated agent shell, so it does **not** overturn
that finding — the likelier reading is that consent is a property of the
process tree, and this shell inherited a terminal that had since been granted
it. What it does establish is that nothing about being automated is
disqualifying on its own.

The genuinely unconsented data point arrived the same day from the `AFS mount
backend (macOS)` CI job, on a stock GitHub runner with no consent granted to
anything: **every stage passed — mount, readdir, read, and write.** So the
`EPERM` MOUNT-SPIKE.md §4 recorded was specific to that harness or that macOS
version; it is not a blanket privacy restriction on network volumes, and it is
not a barrier a design may lean on.

That matters for §7's threat model rather than for convenience. The mitigation
list here is the whole of the access control: an unprivileged local process
that reaches the port and knows the token reads and writes the session's files,
and nothing in the operating system stops it afterwards. Any future reasoning
that treats macOS consent as a second factor is reasoning from a finding this
job disproved.

**Enforcement is not free.** As RESEARCH.md notes, the mount alone does not stop
an agent writing to an absolute path outside it. AFS bounds the blast radius of
writes that go *through* it; the OS sandbox that forces all writes through the
mount is separate work and is not claimed by this design.

---

## 8. Open questions

- **Sandbox pairing** (from RESEARCH.md): Linux namespaces are the proven path;
  macOS needs a validated strategy. Unscoped here.
- **Windows**: no mount backend upstream or here. SDK-only; ProjFS is the
  plausible native route and remains unscoped.
- **Copy-up cap default**: the mount spike establishes a cap in the low tens
  of MiB as the policy target, but the configurable cap is not implemented
  yet.
- **Loopback NFS access control**: decided in §7 (`coven-75e`) — authenticated
  file handles plus a VFS token gate on an ephemeral loopback port. Mount stays
  opt-in because token secrecy is the whole boundary; enabling it by default
  needs a second factor, not a stronger token.
- **Base ingest filters**: the mount spike validates the workload shape, but
  the default exclude set still needs an implementation and broader validation.

## 9. Delivery sequencing

| Step | Bead | State |
|---|---|---|
| Storage engine + overlay | `coven-d4p` | merged — PR #658, `e2da654` |
| macOS mount + benchmark | `coven-110` | merged experimental spike — PR #680, Terminal mounted-I/O confirmation passed; macOS network-volume/privacy access must be assessed per client/harness or deployment |
| This design | `coven-vhw` | — |
| Daemon API + extension tables | not yet filed | ready to file |
| Cave surfaces | not yet filed | needs the daemon API |

The storage engine and an experimental feature-gated NFSv3 backend exist.
Nothing in §3–§6 does: daemon API routes, provenance extensions, commit
materialization, and Cave surfaces remain this document's contract, not a
claim that they are built. Linux/FUSE remains unstarted, and loopback NFS
access control still blocks mount-on-by-default.
