---
summary: "HTTP-over-local-IPC contract under /api/v1. Versioned, capability-discovered, structured-error."
read_when:
  - Building a client for the Coven daemon
  - Auditing what the socket exposes
title: "Socket API"
description: "The Coven daemon exposes a small versioned HTTP API over same-user local IPC. Reference for the coven.daemon.v1 contract under the /api/v1 prefix."
---

Coven exposes a small versioned HTTP API over same-user local IPC. On Unix-like
hosts, this is `<COVEN_HOME>/coven.sock`; on Windows, it is an owner-only named
pipe selected by `COVEN_HOME`. Health and `coven daemon status` report the
active endpoint, so clients must not construct a Windows pipe name from the
Unix convention. The current public contract is **`coven.daemon.v1`** served
under the `/api/v1` prefix.

The daemon does not use OAuth, JWTs, bearer tokens, API keys, or browser
cookies. Trust is **same-user local access** to this IPC endpoint. See [Auth
posture](/daemon/auth-posture) before adding a new client, dashboard, remote
bridge, or browser-facing transport.

## Handshake

Always start with `GET /api/v1/health` as the compatibility handshake for the
named `coven.daemon.v1` contract:

```http
GET /api/v1/health
```

```json
{
  "ok": true,
  "apiVersion": "coven.daemon.v1",
  "covenVersion": "0.0.0",
  "capabilities": {
    "sessions": true,
    "events": true,
    "travel": true,
    "scheduler": true,
    "hub": true,
    "executorDispatch": true,
    "eventCursor": "sequence",
    "structuredErrors": true,
    "sessionHandoff": true,
    "afs": true,
    "afsMount": false,
    "afsCommit": true
  },
  "daemon": {
    "pid": 31415,
    "startedAt": "2026-05-15T19:31:02Z",
    "socket": "<local IPC endpoint>"
  }
}
```

`daemon` is `null` when daemon metadata is unavailable. When present,
`daemon.socket` reports the active local IPC endpoint. A `hub` summary may also
be present when the store-backed health summary is available.

Negotiate the named `coven.daemon.v1` contract against this health
`apiVersion`, then check every capability required by the operation before
depending on a response shape. Capabilities advertise availability and never grant permission.
`GET /api/v1/api-version` remains a legacy route-family
diagnostic whose literal `v1` values are not proof of named-contract support.

## Endpoints

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/api-version` | Read the legacy route-family token. |
| `GET /api/v1/health` | Check daemon health and metadata. |
| `GET /api/v1/capabilities` | Discover routable capabilities and owning adapters. |
| `POST /api/v1/actions` | Send a known intent through the control plane. |
| `GET /api/v1/sessions` | List sessions. |
| `POST /api/v1/sessions` | Launch a session. |
| `GET /api/v1/sessions/:id` | Fetch one session. |
| `GET /api/v1/events?sessionId=...` | Read session events. |
| `GET /api/v1/memory` | List familiar memory summaries with opaque ids. |
| `GET /api/v1/memory/overview` | Read memory totals and capability state. |
| `GET /api/v1/memory/:id` | Read validated markdown content for a list id. |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session. |
| `POST /api/v1/sessions/:id/kill` | Kill a live session. |
| `POST /api/v1/sessions/:id/handoffs` | Offer a redacted, generation-fenced handoff. |
| `GET /api/v1/sessions/:id/handoffs` | Read stored handoff state. |
| `POST /api/v1/sessions/:id/handoffs/:handoffId/claim` | Claim a handoff generation. |
| `POST /api/v1/sessions/:id/handoffs/:handoffId/ack` | Acknowledge a quiesced source. |
| `POST /api/v1/sessions/:id/handoffs/:handoffId/continuations` | Record continuation import. |
| `POST /api/v1/store/vacuum` | Rebuild the event FTS index and compact the SQLite store. |
| `POST /api/v1/afs/sessions` | Create an agent-filesystem session over a project root. |
| `GET /api/v1/afs/sessions` | List agent-filesystem sessions. |
| `GET /api/v1/afs/sessions/:id` | Fetch one agent-filesystem session. |
| `POST /api/v1/afs/sessions/:id/join` | Attach another actor to an existing session. |
| `GET /api/v1/afs/sessions/:id/diff` | Read the change set, or add `?path=` for one file's unified diff. |
| `GET /api/v1/afs/sessions/:id/timeline` | Read file operations with linked, redacted tool-call context. |
| `POST /api/v1/afs/sessions/:id/commit` | Materialize the session's delta into a signed git branch. |
| `POST /api/v1/afs/sessions/:id/discard` | Discard a session (requires `"confirm": true`). |
| `POST /api/v1/afs/sessions/:id/mount` | Mount the session's filesystem; returns `mountPoint`, `backend`, `readOnly`. |
| `DELETE /api/v1/afs/sessions/:id/mount` | Unmount. Idempotent. |

Detailed shapes live in the [API reference](/reference/api).

Agent-filesystem routes are gated by the `afs` capability, and are **same-user
local IPC** operations for the same reason session handoff is: they expose a
session's working tree. `afsCommit` reports whether the daemon can materialize
a delta into a git branch and is now `true`.

`GET /api/v1/afs/sessions/:id/diff` returns the change list. Supplying a
percent-encoded regular-file path returns the daemon-owned review patch
instead:

```json
{
  "path": "/src/main.rs",
  "patch": "--- /src/main.rs\n+++ /src/main.rs\n@@ ...",
  "truncated": false,
  "binary": false
}
```

Text patches are capped at 262,144 bytes and remain valid UTF-8 when
`truncated` is true. Added and deleted text content uses `/dev/null` on the
missing side. Metadata-only empty-file changes have an empty patch and retain
their added/deleted kind in the change-list response. Differing binary content
returns `binary: true` and the stable patch `"Binary files differ\n"`.
Missing paths return `afs.path_not_found`; directories and symlinks return
`afs.path_not_file`.

Timeline pages remain cursor-paginated on provenance `seq`. Each operation
keeps `toolCallId` and adds `toolCall` when that audit row still exists:

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

Tool-call parameters, results, and errors pass through the daemon privacy
filters before serialization. A missing or dangling audit reference produces
`toolCall: null` without dropping the filesystem operation.

`afsMount` reports the mount backend or `false`. It is currently `false` on all
platforms: the macOS NFS backend cannot pass its export credential to
`mount_nfs` without exposing it in the process list, and the Linux FUSE backend
is not built. `POST .../mount` returns `501` with `afs.mount_unsupported`
wherever no backend is reported.

The daemon spawns a per-mount export process serving the merged base+delta
view, mounts it on loopback, rotates the export token as soon as the mount
returns, and unmounts orphans left by a dead daemon at startup. Mounting
requires an open session; `DELETE` does not, so a session committed while
mounted can still be taken down, and unmounting something that is not mounted
succeeds rather than erroring.

A reported backend means the daemon can mount, not that every process can read
through the mount: macOS refuses `open()` on network volumes for processes
without the right privacy consent. Capabilities advertise availability and
never grant permission.

Commit creates a git worktree at the session's `base_commit`, applies the
change set, and produces a **signed** commit carrying `Coven-Session`,
`Coven-Familiar`, `Coven-Bead`, and `Coven-Afs-Session` trailers. It does not
push, open a PR, or run CI. Materialization is all-or-nothing: if the project
root has moved off `base_commit` (`afs.base_diverged`), a path would escape the
repository or write under `.git/` (`afs.path_outside_root`), a file exceeds the
copy-up cap (`afs.copy_up_too_large`), the branch or worktree is taken
(`afs.commit_conflict`), or signing is unavailable (`afs.commit_unsigned`), the
attempt is rolled back and the delta is preserved unchanged. The delta survives
a successful commit too — it is the audit record until an explicit discard.

Passing `"dryRun": true` runs every one of those refusal checks and reports
what would happen, with **no side effects**: no `committing` transition, no
worktree, no branch, and no `afs_commit` row. A preview that would succeed
returns `{ branch, worktreePath, counts, files, provenanceHighWater,
"dryRun": true, "wouldCommit": true }`; a preview that would be refused
returns the same dotted error a real commit would raise, so clients read one
contract rather than two. Preview and commit share their validation, so the
two cannot disagree.
Design: [`specs/coven-agent-fs/DESIGN.md`](https://github.com/OpenCoven/coven/blob/main/specs/coven-agent-fs/DESIGN.md).

Session handoff is a **same-user local IPC** operation. A companion must use a
separately paired authenticated transport; exposing these endpoints directly
to a remote listener would bypass this IPC endpoint's same-user trust boundary. See
[Session handoff](/daemon/session-handoff).

Memory path entries must be UTF-8 regular `.md` files; invalid names,
symlinks, Windows reparse points, non-files/non-directories, and entries that
disappear during scanning are excluded. Unexpected enumeration, directory-open,
or metadata errors fail the request rather than returning partial data.
Overview reads metadata only. List omits the excerpt for an unreadable,
invalid-UTF-8, or over-4-MiB body by returning its metadata-valid row with an
empty `excerpt`. Detail reads only the selected validated handle and returns
`413 memory_content_too_large` above 4 MiB or `422 memory_content_invalid` for
invalid UTF-8. A missing or unsafe target at open time returns
`404 memory_not_found`; permission failures, unexpected open failures, and
post-open metadata/read failures return `503 memory_content_unavailable`.
Those path-safe errors include only `memoryId` in `details`, never a filesystem
path or raw I/O error.

## Error envelope

All error responses use:

```json
{
  "error": {
    "code": "session.cwd_outside_root",
    "message": "cwd must canonicalize inside project root",
    "details": {
      "projectRoot": "/workspace/project",
      "cwd": "/tmp/wander"
    }
  }
}
```

See [Error envelope](/daemon/error-envelope) for the full code list.

## Versioning

The health response's `apiVersion` field is the named contract clients pin against. Coven follows additive compatibility: new fields and new capabilities are added under existing versions; breaking changes require a new version. See [API versioning](/daemon/api-versioning).

## Calling the socket

The following curl, Node `socketPath`, and Rust `UnixStream` examples apply
only on Unix-like hosts. On Windows, use a named-pipe-capable local IPC client
against the daemon-reported endpoint; do not derive a pipe name from
`<COVEN_HOME>/coven.sock`.

<Tabs>
  <Tab title="curl">
    ```bash
    curl --unix-socket "$HOME/.coven/coven.sock" \
      http://localhost/api/v1/health
    ```
  </Tab>
  <Tab title="Node">
    ```js
    import http from "node:http";
    http.get(
      { socketPath: `${process.env.HOME}/.coven/coven.sock`, path: "/api/v1/health" },
      (res) => res.pipe(process.stdout)
    );
    ```
  </Tab>
  <Tab title="Rust">
    ```rust
    let home = std::env::var("HOME")?;
    let socket = std::path::PathBuf::from(home).join(".coven/coven.sock");
    let stream = tokio::net::UnixStream::connect(socket).await?;
    // wrap in hyper or your preferred HTTP client
    ```
  </Tab>
</Tabs>

## Related

- [API contract](/reference/api-contract)
- [Capabilities handshake](/daemon/capabilities-handshake)
- [Error envelope](/daemon/error-envelope)
- [Auth posture](/daemon/auth-posture)
