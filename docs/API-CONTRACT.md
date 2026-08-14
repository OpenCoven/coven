---
title: "Coven local API contract (coven.daemon.v1)"
summary: "The versioned coven.daemon.v1 contract under /api/v1: health negotiation, capability discovery, error envelopes, and additive compatibility rules."
read_when:
  - Pinning a client to the daemon contract
  - Checking stable API shapes and error envelopes
description: "The versioned coven.daemon.v1 contract under /api/v1: health negotiation, capability discovery, error envelopes, and additive compatibility rules."
---

# Coven local API contract

> **See also:** the condensed [API contract reference](reference/api-contract.md), which summarizes versioning and links per-topic pages under `docs/daemon/`. This page is the fuller single-page contract.

The Coven daemon API is a public compatibility boundary for comux and external
clients such as external OpenClaw bridge plugin. It travels over same-user local
IPC: `<COVEN_HOME>/coven.sock` on Unix-like hosts, or an owner-only named pipe
selected by `COVEN_HOME` on Windows. Health and `coven daemon status` report
the active endpoint; clients must not construct a Windows pipe name from the
Unix convention.

## Current stable version

Clients negotiate compatibility with `GET /api/v1/health`. Its `apiVersion`
field is the named contract `coven.daemon.v1`; clients must then check every
capability required by the operation before sending a dependent request.
Capabilities advertise availability and never grant permission.

`GET /api/v1/api-version` is a legacy route-family diagnostic. Its existing
`apiVersion: "v1"` and `supportedApiVersions: ["v1"]` values identify the
`/api/v1/*` route namespace, not the named compatibility contract. Existing
values remain wire-compatible, but new clients must not use this response as
proof of `coven.daemon.v1` support.

- `GET /api/v1/health` exposes `apiVersion: "coven.daemon.v1"`, `covenVersion`, and a machine-readable `capabilities` object.
- Clients should read `/api/v1/health` before assuming any response shape from other endpoints.
- Legacy unversioned routes such as `GET /health` remain early-MVP aliases; new clients should use `/api/v1`.
- Control-plane clients should discover capabilities before sending action ids.
- All API failures are returned as structured `{ "error": { "code", "message", "details" } }` envelopes.
- Events include a monotonic `seq` cursor for incremental reads.
- Event payloads are redacted by default before API display.

## `GET /api/v1/health`

`GET /api/v1/health` returns daemon reachability, the named contract version, coven version, and machine-readable capabilities:

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
    "sessionLaunchPolicy": true,
    "afs": true,
    "afsMount": false,
    "afsCommit": true,
    "afsCommitDryRun": true,
    "executionBindingContracts": ["psyche.execution_binding.v1"]
  },
  "daemon": {
    "pid": 12345,
    "startedAt": "2026-05-09T06:43:00Z",
    "socket": "<local IPC endpoint>"
  },
  "eventWriter": {
    "state": "healthy",
    "queuedBytes": 0,
    "capacityBytes": 2097152,
    "droppedOutputEvents": 0,
    "droppedOutputBytes": 0,
    "connectionOpens": 1,
    "transactions": 42,
    "committedEvents": 513
  },
  "hub": {
    "role": "hub",
    "hubId": "hub_01J...",
    "nodesTotal": 2,
    "nodesAvailable": 1
  }
}
```

If the daemon metadata is unavailable, `daemon` may be `null`. When present,
`daemon.socket` reports the active local IPC endpoint. The `hub` block reports
the daemon's control-plane role and node availability summary; full node detail
lives at `GET /api/v1/hub/status`.

`eventWriter` is present for the daemon-owned live-session runtime. Its
`state` is `healthy`, `pressured`, or `failed`. A pressured writer reports raw
output that could not enter the byte-bounded queue; lifecycle, tool, error, and
exit events reserve capacity and are not dropped for pressure. Raw output is
the only lossy class. Each contiguous pressure episode produces one ordered
`output_truncated` event for the affected session, inserted immediately before
that session's next accepted event; the marker's `created_at` is the first
rejected chunk timestamp. Global writer counters remain in the health payload.
A failed writer includes `lastError`; clients should surface it as degraded
persistence rather than treating the daemon's liveness as successful event
durability.

### Capability fields

| Field             | Type    | Description                                                       |
|-------------------|---------|-------------------------------------------------------------------|
| `sessions`        | boolean | Sessions API (`/sessions`, `/sessions/:id`) is available.        |
| `events`          | boolean | Events API (`/events`) is available.                             |
| `travel`          | boolean | Travel profile, delta, and state APIs are available.             |
| `scheduler`       | boolean | Scheduler decision and recovery APIs are available.              |
| `hub`             | boolean | Hub control-plane APIs (node registry, routing, queues) are available. |
| `executorDispatch`| boolean | Hub-outbound executor poll/dispatch APIs are available.          |
| `eventCursor`     | string  | Cursor type supported; `"sequence"` means `afterSeq` is stable.  |
| `structuredErrors`| boolean | All errors use the `{ error: { code, message, details } }` shape.|
| `sessionHandoff` | boolean | Durable generation-fenced session handoff routes are available. |
| `sessionLaunchPolicy` | boolean | Owner-gated local IPC accepts the exact unattended Codex launch policy. Always `false` over TCP. |
| `afs` | boolean | The AFS route family is available. |
| `afsMount` | string or `false` | Active mount backend, or `false` when mount-backed access is unavailable. |
| `afsCommit` | boolean | AFS deltas can be materialized into a Git branch. |
| `afsCommitDryRun` | boolean | AFS commit accepts the side-effect-free `dryRun` contract. |
| `executionBindingContracts` | string array | Psyche execution-binding contract names this daemon accepts. Currently `["psyche.execution_binding.v1"]`. A client that requires binding must confirm `"psyche.execution_binding.v1"` is present before sending a bound launch; an unknown or missing required value fails before any dependent request, per the existing fail-closed negotiation rule. See [Psyche execution binding contract (`v1`)](#psyche-execution-binding-contract-v1). |

## Structured error envelope

```mermaid
flowchart TD
  Req[Incoming request] --> Parse{Parse + version check}
  Parse -- bad shape --> ErrInvalid["400 invalid_request"]
  Parse -- unknown version --> ErrInvalid
  Parse -- ok --> Route{Route exists?}
  Route -- no --> ErrNotFound["404 not_found"]
  Route -- yes --> Validate{Field validation}
  Validate -- cwd outside root --> ErrInvalid
  Validate -- unknown harness/action --> ErrInvalid
  Validate -- ok --> Action{Resource lookup}
  Action -- session missing --> ErrSession["404 session_not_found"]
  Action -- session not live --> ErrLive["409 session_not_live"]
  Action -- launch (PTY/pipe spawn, init write, harness startup) fails --> ErrLaunch["500 launch_failed"]
  Action -- send_input fails --> ErrSend["500 send_input_failed"]
  Action -- kill_session fails --> ErrKill["500 kill_failed"]
  Action -- runtime down --> ErrRuntime["503 runtime_unavailable"]
  Action -- internal panic --> ErrInternal["500 internal_error"]
  Action -- ok --> Success[Documented success shape]

  ErrInvalid & ErrNotFound & ErrSession & ErrLive & ErrLaunch & ErrSend & ErrKill & ErrRuntime & ErrInternal -->|"{ error: { code, message, details } }"| Client[Client branches on code]
```

All API errors use the following stable envelope. Clients must branch on `error.code`, not `error.message`:

```json
{
  "error": {
    "code": "session_not_found",
    "message": "Session was not found.",
    "details": {
      "sessionId": "abc-123"
    }
  }
}
```

`details` is optional and included when extra context is useful.

### Stable error codes

| Code                   | HTTP status | Description                                      |
|------------------------|-------------|--------------------------------------------------|
| `not_found`            | 404         | Generic route not found.                         |
| `invalid_request`      | 400 or 404  | Malformed request, unknown harness id, missing required field, or unsupported API version. |
| `forbidden`            | 403         | The request asks TCP to exercise an owner-local-IPC-only capability such as `launchPolicy`. |
| `session_not_found`    | 404         | Session id does not exist.                       |
| `harness_not_found`    | 404         | `GET /capabilities/:harnessId`: harness id is not a known capability scan target. |
| `session_not_live`     | 409         | Session exists but is not running.               |
| `project_root_violation`| 400        | Reserved. Cwd-outside-root currently emits `invalid_request` with the violation message in the body; promoting to its own code would let clients branch without parsing prose. |
| `pty_spawn_failed`     | 500         | Reserved. PTY spawn failures currently emit `launch_failed`; promoting to its own code would let clients distinguish "the PTY couldn't open" (likely a host issue) from "the harness CLI errored at startup" (likely an auth/config issue). |
| `launch_failed`        | 500         | Daemon accepted the launch payload but the runtime (PTY/pipe spawn, initial-message write, harness CLI startup) failed. `details.sessionId` is the row that was inserted and marked `failed`. |
| `maintenance_locked`   | 423         | A valid repository maintenance owner is draining or holds the common-directory gate. `details.owner` carries its fenced generation and deadline. |
| `maintenance_state_invalid` | 423  | The repository maintenance protocol contains malformed or ambiguous state. Coven fails closed rather than launching a writer. |
| `maintenance_gate_unavailable` | 423 | Coven could not establish a repository maintenance writer intent. |
| `send_input_failed`    | 500         | Daemon accepted the input payload but the runtime write failed (closed pipe, killed process, IO error). `details.sessionId` is the affected session. |
| `kill_failed`          | 500         | Daemon accepted the kill request but the runtime signal/kill call failed (permission, missing process, IO error). `details.sessionId` is the affected session. |
| `runtime_unavailable`  | 503         | The session runtime is unavailable.              |
| `internal_error`       | 500         | Unexpected internal error.                       |
| `raw_artifacts_disabled` | 403       | Raw artifact retrieval was requested without explicit raw artifact persistence enabled. |
| `raw_artifact_requires_raw_flag` | 400 | Raw artifact retrieval omitted the required `raw=1` query flag. |
| `artifact_not_found`   | 404         | Sensitive artifact id does not exist for the session. |
| `travel_profile_not_found` | 404     | Travel profile id does not exist.                |
| `travel_profile_expired` | 409       | Travel profile is expired and cannot accept deltas. |
| `source_hub_mismatch`  | 409         | Delta source hub does not match the travel profile source hub. |
| `no_scheduler_target`  | 409         | No available scheduler node matches the requested capabilities and policy. |
| `scheduler_decision_not_found` | 404 | Scheduler decision id does not exist.            |
| `scheduler_loop_not_found` | 404     | Scheduler loop state does not exist.             |
| `node_not_found`       | 404         | Node id does not exist in the hub registry.      |
| `node_unavailable`     | 409         | Requested assignment target node is not available. |
| `node_missing_capabilities` | 409    | Requested assignment target node lacks required capabilities. |
| `job_not_found`        | 404         | Job id does not exist in the hub queue.          |
| `job_already_queued`   | 409         | A job with the same id already exists in the global queue. |
| `job_not_assignable`   | 409         | Job has already reached a terminal state.        |
| `no_available_node`    | 409         | No available registered node satisfies the job's required capabilities. |
| `session_id_conflict`  | 409         | `POST /sessions/external`: a daemon-managed (non-external) session with the supplied id already exists. |
| `not_external_session` | 422         | `POST /sessions/:id/complete`: the session exists but is not an external session. Use `POST /sessions/:id/kill` for daemon-managed sessions. |
| `external_session_not_killable` | 422 | `POST /sessions/:id/kill`: the session is external and not managed by the daemon; use `POST /sessions/:id/complete` instead. |
| `execution_binding_invalid` | 400   | `executionBinding` (or its nested `parent`) is malformed, missing a required member, or carries an unknown/extra member; a launch cross-field rule (root/child, canonical-familiar) fails; or an external-session registration request supplies `executionBinding` at all. See [Psyche execution binding contract (`v1`)](#psyche-execution-binding-contract-v1). |
| `execution_binding_unsupported` | 400 | `executionBinding.contract` is present but is not `psyche.execution_binding.v1`. |
| `execution_binding_required` | 400 | A bound session's `POST /sessions/:id/input` or `POST /sessions/:id/kill` omits `executionBinding` or supplies an incomplete proof. |
| `execution_binding_expired` | 409  | Launch, or bound input, references an `executionBinding.expiresAt` that has already elapsed. Bound kill is explicitly exempt from this check. |
| `execution_binding_mismatch` | 409 | A bound request's `executionBinding` proof, once parsed, byte-differs from the session's stored binding on at least one field, including parent correlation. `details.fields` names only the first mismatched field path, never a value. |

## Capability catalog shape (`v1`)

`GET /api/v1/capabilities` returns the daemon/control-plane capability catalog. This is the intended intake-client discovery surface for deciding which actions to show or route through Coven after compatibility negotiation.

```json
{
  "capabilities": [
    {
      "id": "coven.control.actions",
      "label": "Coven control-plane action router",
      "adapter": "coven-daemon",
      "status": "available",
      "policy": "allow",
      "actions": ["coven.capabilities.refresh"]
    },
    {
      "id": "coven.travel",
      "label": "Travel profiles and offline delta reconciliation",
      "adapter": "coven-daemon",
      "status": "available",
      "policy": "allow",
      "actions": []
    },
    {
      "id": "coven.scheduler",
      "label": "Multi-host scheduler decisions and recovery",
      "adapter": "coven-daemon",
      "status": "available",
      "policy": "allow",
      "actions": []
    },
    {
      "id": "desktop.automation",
      "label": "Desktop automation adapters",
      "adapter": "desktop-use",
      "status": "planned",
      "policy": "requiresApproval",
      "actions": []
    }
  ]
}
```

Known enum values in `v1`:

- `status`: `available`, `planned`
- `policy`: `allow`, `requiresApproval`

Clients should ignore unknown future capability ids and action ids unless they explicitly support them.

## Harness capability manifests (`v1`)

Distinct from the control-plane catalog above, `GET /api/v1/capabilities/harnesses` returns what each installed harness brings (global instructions, skills, plugins) plus Coven-owned skills, and `GET /api/v1/capabilities/:harnessId` returns a single harness's manifest. Both accept `?refresh=1` to invalidate the 5-minute scan cache. This surface keeps the snake_case field names pinned by `specs/coven-harness-capabilities/`:

```json
{
  "coven_skills": [],
  "harness_capabilities": [
    {
      "harness_id": "codex",
      "scanned_at": "2026-07-15T12:00:00Z",
      "global_instructions": { "present": false },
      "skills": [],
      "plugins": [],
      "warnings": []
    }
  ],
  "scanned_at": "2026-07-15T12:00:00Z"
}
```

Uninstalled harnesses return empty manifests, never errors. Unknown harness ids on `/capabilities/:harnessId` return `404 harness_not_found`. `harnesses` is a reserved path segment, never a harness id. See [Capabilities endpoint](reference/api-capabilities.md) for the full reference.

## Memory dashboard read shapes (`v1`)

The memory read surface is additive to the existing observability list:

- `GET /api/v1/memory` returns summary rows;
- `GET /api/v1/memory/overview` returns counts and capability state;
- `GET /api/v1/memory/:id` returns one validated detail row.

The list preserves its original `familiar_id`, `title`, `path`, `updated_at`,
and `excerpt` fields, and adds the same authoritative `source` object returned
by detail. `path` is relative to the memory root and exists for CLI
compatibility; it is never absolute. The opaque UUID `id` is stable while the
relative file identity is stable. Browser-facing adapters should omit `path`
from their DTOs.

Memory enumeration is metadata-only. It accepts UTF-8 familiar directory names
and UTF-8 `.md` file names whose directory entries are regular files. Confirmed
non-UTF-8 names, symlinks, Windows reparse points, non-files/non-directories,
and entries that disappear during enumeration are excluded. Unexpected
iterator, directory-open, or entry-metadata errors fail the request instead of
returning a partial or empty success. If two accepted entries ever produce the
same opaque id, the request fails closed instead of returning an ambiguous id.
The list then reads each accepted file through a no-follow,
directory-relative handle to build its excerpt. If that body is unavailable,
invalid UTF-8, or larger than 4 MiB, the metadata-valid row remains in the list
with an empty `excerpt`; other valid rows are still returned.

```json
[
  {
    "id": "d251bc66-3e45-5d03-8d78-1e76919642f9",
    "familiar_id": "sage",
    "title": "notes",
    "path": "sage/notes.md",
    "updated_at": "4m ago",
    "updated_at_iso": "2026-07-26T09:56:00Z",
    "excerpt": "Durable fact.",
    "source": {
      "kind": "coven-origin",
      "label": "Coven origin"
    },
    "privacy_classification": null,
    "reveal_required": null,
    "verification_state": "unknown"
  }
]
```

The overview uses the same metadata-only enumeration and does not read any file
bodies. A structurally valid entry is therefore counted even when its body is
invalid UTF-8 or too large for list/detail reads. The overview does not
translate unavailable metadata into zero or healthy:

```json
{
  "generated_at": "2026-07-26T10:00:00Z",
  "totals": {
    "entries": 1,
    "familiars": 1,
    "verified": 0,
    "needs_review": 0,
    "unknown": 1
  },
  "last_updated_at": "2026-07-26T09:56:00Z",
  "capabilities": {
    "detail": true,
    "verification": false,
    "attestation_metadata": false,
    "supersession_history": false,
    "mutations": false
  },
  "verification": {
    "state": "unavailable",
    "checked_at": "2026-07-26T10:00:00Z",
    "manifest": null,
    "index": null,
    "issues": []
  }
}
```

Detail accepts only a UUID returned by the list. It enumerates metadata, opens
only the matching entry through a no-follow directory-relative handle,
validates that exact handle as a regular file, and reads from that same handle.
It returns content without a path:

```json
{
  "id": "d251bc66-3e45-5d03-8d78-1e76919642f9",
  "familiar_id": "sage",
  "title": "notes",
  "updated_at": "2026-07-26T09:56:00Z",
  "source": {
    "kind": "coven-origin",
    "label": "Coven origin"
  },
  "content": "Durable fact.",
  "content_format": "markdown",
  "privacy": {
    "classification": null,
    "reveal_required": null,
    "reason": "privacy taxonomy unavailable"
  },
  "verification": {
    "state": "unknown",
    "reason": "verification metadata unavailable"
  },
  "attestation": null,
  "supersession": {
    "supersedes": null,
    "superseded_by": null
  }
}
```

Detail content must be UTF-8 and no larger than 4 MiB (4,194,304 bytes).
Malformed ids return `400 invalid_request`; well-formed ids that do not resolve,
or entries that disappear or become an unsafe target before the validated
open, return `404 memory_not_found`. Permission failures, unexpected open
failures, and metadata/read failures on an already-opened handle return
`503 memory_content_unavailable`. Its error details contain only `memoryId`;
filesystem errors and paths are never exposed. Oversize content returns
`413 memory_content_too_large` with `details.maxBytes`; invalid UTF-8 returns
`422 memory_content_invalid`. Until the promotion privacy contract provides a
classification, clients must treat `classification: null` and
`reveal_required: null` as requiring explicit reveal.

## Control action shape (`v1`)

`POST /api/v1/actions` accepts a policy-shaped action envelope. The daemon validates the action id before any adapter work is allowed.

```json
{
  "action": "coven.capabilities.refresh",
  "origin": "external-client",
  "intentId": "intent-1",
  "args": {}
}
```

Immediately completed safe actions return `200`:

```json
{
  "ok": true,
  "accepted": true,
  "action": "coven.capabilities.refresh",
  "status": "completed",
  "event": {
    "kind": "capabilities.refreshed",
    "action": "coven.capabilities.refresh",
    "origin": "external-client",
    "intentId": "intent-1",
    "payload": { "capabilities": 5 }
  }
}
```

Unknown action ids return `400` and fail closed:

```json
{
  "ok": false,
  "accepted": false,
  "action": "desktop.deleteEverything",
  "status": "rejected",
  "reason": "unknown action `desktop.deleteEverything`"
}
```

## `POST /api/v1/sessions`

Launches a daemon-managed harness session. `model` is optional; when present,
the daemon forwards the provider-qualified id through the selected harness
adapter's declared `strip_provider` or `preserve` transform. Clients that omit
it retain the harness's own default model.

Clients must observe `capabilities.sessionLaunchPolicy === true` before sending
`launchPolicy`. The only supported policy is an exact, explicit Codex
`nonInteractive` contract: approval `never`, sandbox `workspace-write`, and
optional absolute additional directories. Every `addDirs` entry must be a
non-empty, existing directory; entries are canonicalized and deduplicated
before they reach Codex. This supports an explicitly granted mission workspace
outside the research context root without granting any implicit parent or
sibling path. Unknown fields or values, other harnesses or modes, relative,
missing, or non-directory paths fail with `400 invalid_request` before a
session row or process is created. Omitting `launchPolicy` preserves the
harness default.
This policy is owner-local-IPC-only: health advertises
`sessionLaunchPolicy: false` over TCP, and a TCP request that nevertheless
includes the field fails with `403 forbidden` before a session row or process
is created. Host and Origin allowlists do not grant this authority.
Capabilities advertise availability; the owner-gated local IPC boundary,
exact requested write set, and Rust validation remain the authority boundary.

```json
{
  "projectRoot": "/repo",
  "harness": "codex",
  "model": "openai/gpt-5.6-sol",
  "launchMode": "nonInteractive",
  "launchPolicy": {
    "approval": "never",
    "sandbox": "workspace-write",
    "addDirs": []
  },
  "prompt": "Fix the tests"
}
```

A launch may additionally carry a top-level `executionBinding` object (and, for
a delegated launch, `callerFamiliarId`) to bind the session to an opaque
Psyche-owned `psyche.execution_binding.v1` identity. This is entirely optional
and additive: a launch that omits `executionBinding` behaves exactly as it
does today. See [Psyche execution binding contract (`v1`)](#psyche-execution-binding-contract-v1)
for the full request/response shape, validation, and error rules.

## Session record shape (`v1`)

In `v1`, session responses stay as raw JSON objects using the Rust daemon's snake_case field names.

Endpoints that return this shape:

- `GET /api/v1/sessions` → `SessionRecord[]`
- `POST /api/v1/sessions` → `SessionRecord`
- `GET /api/v1/sessions/:id` → `SessionRecord`
- `POST /api/v1/sessions/external` → `SessionRecord`
- `POST /api/v1/sessions/:id/complete` → `SessionRecord`

```json
{
  "id": "session-1",
  "project_root": "/repo",
  "harness": "codex",
  "title": "Fix the tests",
  "status": "running",
  "exit_code": null,
  "archived_at": null,
  "created_at": "2026-05-09T06:43:00Z",
  "updated_at": "2026-05-09T06:43:05Z",
  "conversation_id": null,
  "familiar_id": null,
  "execution_binding": null,
  "labels": [],
  "visibility": "private",
  "external": false,
  "transcript_path": null
}
```

`execution_binding` is `null` for every session launched without a Psyche
binding; it is never omitted from the payload. A session launched with a
bound `executionBinding` request field serializes the full stored
`psyche.execution_binding.v1` object here instead, unchanged by archive
state, cursor position, or lifecycle status. See
[Psyche execution binding contract (`v1`)](#psyche-execution-binding-contract-v1).

The `external` field is `true` for sessions registered via `POST /api/v1/sessions/external`; it is `false` for all daemon-launched sessions. The `transcript_path` field carries the absolute path to the external session's transcript file when provided at registration; it is `null` for daemon-launched sessions and for external sessions where no path was supplied.

Classify the row kind before interpreting `status`. Synthetic `active` rows can appear in raw store or list output, but `active` is not a harness-session state.

| Harness-session status | Terminal? | Meaning |
|---|---|---|
| `created` | No | Ledger row exists before runtime ownership. Stale unowned `created` rows recover to `failed`. |
| `running` | No | Reported live state. Inspect `external` to determine whether Coven owns and supervises the runtime. |
| `idle` | No | Reusable conversational session is waiting for more work. |
| `completed` | Yes | Harness session completed successfully. |
| `failed` | Yes | Launch or execution failed. |
| `killed` | Yes | Terminal in the current ledger. This status is not proof that process termination was acknowledged. |
| `orphaned` | Yes | Runtime ownership was lost and the outcome remains unresolved. |

Archive is not a session status. It is stored separately in `archived_at`; archive and summon preserve the existing lifecycle status of every non-running session, including `created` and `idle`.

External `running` sessions are not daemon-control targets: `POST /api/v1/sessions/:id/input` returns `409 session_not_live` because Coven has no owned live runtime, and `POST /api/v1/sessions/:id/kill` returns `422 external_session_not_killable` as documented below.

## `POST /api/v1/sessions/external`

Registers a session that is already running outside the daemon (for example, the engine's interactive TUI). The daemon creates a ledger row with `external: true` and does not own the PTY or lifecycle.

### Request body

```json
{
  "id": "sess-engine-abc",
  "projectRoot": "/repo",
  "harness": "coven-code",
  "title": "coven-code session",
  "transcriptPath": "/repo/.claude/sessions/sess-engine-abc.jsonl"
}
```

| Field            | Type   | Required | Description                                                                 |
|------------------|--------|----------|-----------------------------------------------------------------------------|
| `id`             | string | Yes      | Session id. Must be non-empty after trimming whitespace.                    |
| `projectRoot`    | string | Yes      | Absolute path to the project root. Must be non-empty after trimming.       |
| `harness`        | string | Yes      | Harness identifier (e.g. `"coven-code"`). Must be non-empty after trimming.|
| `title`          | string | No       | Display title. Defaults to `"External session"` when absent or empty.      |
| `transcriptPath` | string | No       | Absolute path to the external session's transcript file. Stored as-is; the daemon does not read or validate the path. |

### Responses

| Status | Condition                                                                                          |
|--------|----------------------------------------------------------------------------------------------------|
| `201`  | Session did not exist; row created. Body: the new `SessionRecord`.                                 |
| `200`  | An external session with this id was already registered (idempotent re-register). Body: the existing `SessionRecord`. |
| `409`  | `session_id_conflict` — a daemon-managed (non-external) session with this id already exists. The daemon refuses to alias it. |
| `400`  | `invalid_request` — malformed JSON or a required field is missing or blank.                        |
| `400`  | `execution_binding_invalid` — the request supplies `executionBinding` at all. Coven does not supervise an externally registered runtime and cannot honor bound-operation guarantees for it; this is checked before any other field is read. `details.fields` is `["executionBinding"]`. |

On success the response body is the full `SessionRecord` as described in [Session record shape (`v1`)](#session-record-shape-v1), with `external: true` and `status: "running"`.

## `POST /api/v1/sessions/:id/complete`

Marks an externally-registered session finished. The daemon updates the session status based on `exitCode` and returns the updated `SessionRecord`.

### Request body

```json
{
  "exitCode": 0
}
```

| Field      | Type    | Required | Description                                                                                   |
|------------|---------|----------|-----------------------------------------------------------------------------------------------|
| `exitCode` | integer | No       | Process exit code. Absent, `null`, or `0` → status becomes `"completed"`. Any nonzero value → status becomes `"failed"`. |

### Responses

| Status | Condition                                                                                  |
|--------|--------------------------------------------------------------------------------------------|
| `200`  | Session updated. Body: the updated `SessionRecord` with the new `status` and `exit_code`.  |
| `404`  | `session_not_found` — no session with this id exists.                                      |
| `422`  | `not_external_session` — the session exists but was not registered as external. For daemon-managed sessions use `POST /api/v1/sessions/:id/kill`. |

### Kill on an external session

`POST /api/v1/sessions/:id/kill` returns `422 external_session_not_killable` when the target session has `external: true`. The kill endpoint is only valid for daemon-managed sessions.

## Psyche execution binding contract (`v1`)

Coven binds a session at launch to an immutable, opaque `psyche.execution_binding.v1`
tuple that Psyche defines and Coven never interprets beyond syntax, contract
identity, and expiry. Coven persists the tuple unchanged and exact-compares
it, byte for byte, on every subsequent bound mutating request (input, kill)
that must prove it. This is a mismatch-correlation guarantee only — it
detects a proof drawn from, or matching, a different attempt's tuple. It is
**not** authentication, and it is **not** a uniqueness or replay guarantee:
two sessions may be launched with byte-identical `executionBinding` objects
and both succeed. See [Non-goals](#non-goals) below and the normative design,
`specs/psyche/O2_CONTRACT_DESIGN.md`.

### Contract identity and field naming

The named contract is `psyche.execution_binding.v1`. Requests carry it under
the camelCase field `executionBinding`, matching request conventions. The
persisted `SessionRecord` response exposes it under the snake_case field
`execution_binding` (see [Session record shape (`v1`)](#session-record-shape-v1)),
matching response conventions. The binding object itself always carries its
own `contract` member, so a stored or returned object is self-describing
independent of the wrapper field name. The health capability array is named
`executionBindingContracts` (see [Capability fields](#capability-fields)) —
distinct from the request/response field name, so it never collides by name
or type with the `executionBinding` object itself.

### JSON shape

Root (non-delegated) launch — the object contains exactly these 13 members,
no more, no fewer:

```json
{
  "familiarId": "sage",
  "executionBinding": {
    "contract": "psyche.execution_binding.v1",
    "principalRef": "principal:operator",
    "familiarId": "sage",
    "familiarSnapshotDigest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "projectDigest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "graphId": "graph-1",
    "nodeId": "node-1",
    "attemptId": "attempt-1",
    "requestDigest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "policyRevision": "policy:7",
    "expiresAt": "2099-01-01T00:00:00Z",
    "parent": null,
    "delegationDigest": null
  }
}
```

Child (delegated) launch requires the top-level `callerFamiliarId` and a
complete, non-null `parent` object (exactly these 4 members) and
`delegationDigest`:

```json
{
  "familiarId": "sage",
  "callerFamiliarId": "cody",
  "executionBinding": {
    "contract": "psyche.execution_binding.v1",
    "principalRef": "principal:operator",
    "familiarId": "sage",
    "familiarSnapshotDigest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "projectDigest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "graphId": "graph-2",
    "nodeId": "node-2",
    "attemptId": "attempt-2",
    "requestDigest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    "policyRevision": "policy:7",
    "expiresAt": "2099-01-01T00:00:00Z",
    "parent": {
      "sessionId": "parent-1",
      "graphId": "graph-1",
      "nodeId": "node-1",
      "attemptId": "attempt-1"
    },
    "delegationDigest": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
  }
}
```

`GET /api/v1/sessions/:id` and any session-listing route return the same
typed field values under `execution_binding`: `null` for an unbound session
(never omitted), the full typed object for a bound one.

### Field semantics

Every field is opaque to Coven except syntax, contract identity, and expiry,
which Coven validates. Coven never interprets principal, familiar, graph,
node, attempt, policy, or delegation meaning.

| Field | Nullable | Coven's obligation |
|---|---:|---|
| `contract` | No | Must equal `psyche.execution_binding.v1`; rejected otherwise. |
| `principalRef` | No | Opaque ref syntax; store and exact-compare. |
| `familiarId` | No | Opaque ref syntax; at launch, must exact-match the canonical `FamiliarContext.id` resolved from top-level `familiarId` (not merely the raw alias). |
| `familiarSnapshotDigest` | No | Digest syntax; store and exact-compare. |
| `projectDigest` | No | Digest syntax; store and exact-compare. Independent of, and never derived from or checked against, the Coven-canonical `project_root`. |
| `graphId` | No | Opaque ID syntax; store and exact-compare. |
| `nodeId` | No | Opaque ID syntax; store and exact-compare. |
| `attemptId` | No | Opaque ID syntax; store and exact-compare. |
| `requestDigest` | No | Digest syntax; store and exact-compare on bound input/kill. No uniqueness or conflict detection over this field. |
| `policyRevision` | No | Opaque revision syntax; store and exact-compare. Coven never evaluates policy. |
| `expiresAt` | No | Canonical UTC RFC 3339 whole-second timestamp; Coven checks syntax and, at launch and for bound input, that it has not already elapsed. |
| `parent` | Yes | `null` for a root binding; a complete 4-field object for a child binding. Coven checks existence and exact-match against the referenced parent session's stored fields; it never infers graph topology. |
| `delegationDigest` | Yes | `null` for a root binding; a digest for a child binding. Store and exact-compare. Coven never authorizes delegation. |

### Exact-object membership and no normalization

`executionBinding` and its nested `parent` are each a closed, exact set of
members — there is no open/extensible schema at either level:

- `executionBinding` must contain exactly the 13 members above — no more, no
  fewer, no additional ones.
- A non-null `parent` must contain exactly `sessionId`, `graphId`, `nodeId`,
  `attemptId` — no more, no fewer, no additional ones.
- Any unrecognized member key at either level is rejected with
  `execution_binding_invalid` before any other validation runs. This applies
  identically at launch and to the proof on bound input/kill.
- Coven performs **no normalization**: no trimming, no case folding, no
  Unicode normalization, no other reformatting. A value is checked against
  the syntax rule below and then stored/compared exactly as received, byte
  for byte. A same-after-normalization value is not a match — it is a syntax
  failure (`execution_binding_invalid`) if it fails raw syntax, or a mismatch
  (`execution_binding_mismatch`) on bound input/kill if it is syntactically
  valid but byte-differs from the stored value. For example, a `graphId`
  differing only in letter case from the stored value is rejected as a
  mismatch, not silently accepted.

### Shape validation

| Value class | Applies to | Rule |
|---|---|---|
| Opaque ref/ID/policy-revision | `principalRef`, `familiarId` (both locations), `graphId`, `nodeId`, `attemptId`, `policyRevision`, `parent.sessionId`, `parent.graphId`, `parent.nodeId`, `parent.attemptId` | 1 to 255 ASCII bytes, matching `[A-Za-z0-9._:/-]` only. |
| Digest | `familiarSnapshotDigest`, `projectDigest`, `requestDigest`, `delegationDigest` (when present) | Exactly `sha256:` followed by 64 lowercase hexadecimal characters (71 bytes total). |
| Timestamp | `expiresAt` | Canonical UTC RFC 3339 whole-second: `YYYY-MM-DDTHH:MM:SSZ`. No fractional seconds, no non-`Z` offset. Coven validates by parsing the value as RFC 3339 and re-serializing the parsed instant through the same canonical whole-second formatter, accepting the value only if the two are byte-identical. This check does not special-case a leap second: `SS` may be the RFC 3339 leap-second value `60` in addition to `00`-`59`, because a leap-second instant round-trips unchanged through that same parse/format pair — `SS` is not restricted to `00`-`59` only. |
| Contract | `contract` | Must equal `psyche.execution_binding.v1` exactly. |

### Launch correlation rules

- The top-level, Coven-resolved canonical `projectRoot` remains Coven
  authority and is persisted unchanged as `project_root`. `projectDigest` is
  Psyche-owned, independently persisted, and never derived from or checked
  against `project_root`.
- A bound launch requires top-level `familiarId`; its absence is
  `400 execution_binding_invalid` (`details.fields: ["familiarId"]`). Coven
  runs its existing `resolve_familiar` resolution and
  `executionBinding.familiarId` must exact-match the resolved
  `FamiliarContext.id` — not merely the raw alias supplied. A mismatch is
  `409 execution_binding_mismatch` (`details.fields: ["executionBinding.familiarId"]`)
  and no session row is created.
- **Root binding:** `parent` must be `null`, `delegationDigest` must be
  `null`, and `callerFamiliarId` must be absent from the top-level request.
  Any other combination of these three is rejected with
  `400 execution_binding_invalid`, naming the single field responsible
  (`executionBinding.parent`, `executionBinding.delegationDigest`, or
  `callerFamiliarId`). A present `callerFamiliarId` that is `null`,
  non-string, empty, or carries leading/trailing whitespace always fails at
  `callerFamiliarId`, independent of `parent`/`delegationDigest` — it is
  never collapsed into "absent".
- **Child binding:** `parent` must be a complete object, `delegationDigest`
  must be present, and `callerFamiliarId` is required. The session named by
  `parent.sessionId` must exist (`404 session_not_found`,
  `details.fields: ["parent.sessionId"]`, if it does not) and must itself
  carry a stored, non-null `execution_binding`; if it exists but is unbound,
  the response is `409 execution_binding_mismatch` naming only
  `parent.sessionId`, since no stored binding fields exist to compare. That
  parent's stored `familiar_id` must exact-match the request's
  `callerFamiliarId` (mismatch: `execution_binding_mismatch`,
  `details.fields: ["callerFamiliarId"]`), and the parent's stored
  `graphId`/`nodeId`/`attemptId` must exact-match the request's
  `parent.graphId`/`parent.nodeId`/`parent.attemptId` respectively (mismatch:
  `execution_binding_mismatch`, `details.fields` naming the bare
  `parent.graphId`/`parent.nodeId`/`parent.attemptId` path, one test per
  field).
- Parent correlation is existence and exact-match only. Coven never
  authorizes delegation policy or infers graph topology beyond the single
  parent reference given, and it does not resolve or enforce Coven Calls
  delegation authority from `callerFamiliarId`.

### Field path conventions in error `details`

`details.fields` always names exactly one static field path, never a value
or digest. Two distinct conventions apply, matching the actual daemon
behavior:

- **Shape/contract/cross-field violations** (`execution_binding_invalid`,
  `execution_binding_unsupported`, `execution_binding_required`) name the
  fully-qualified path from the request root, e.g. `executionBinding.contract`,
  `executionBinding.parent`, `executionBinding.parent.sessionId`,
  `executionBinding.delegationDigest`. `callerFamiliarId` is named bare
  because it is a top-level launch field, not a member of `executionBinding`.
- **Exact-match mismatches** (`execution_binding_mismatch`) name a top-level
  `executionBinding` field with its full path (e.g.
  `executionBinding.familiarId`, `executionBinding.graphId`,
  `executionBinding.delegationDigest`), but name a nested `parent`
  correlation mismatch bare — `parent`, `parent.sessionId`, `parent.graphId`,
  `parent.nodeId`, `parent.attemptId` — never
  `executionBinding.parent.sessionId`. `callerFamiliarId` mismatches are
  likewise named bare. This bare-`parent.*`/`callerFamiliarId` convention is
  normative for mismatch details and applies identically to launch parent
  correlation and to bound input/kill proof comparison.

### Persistence and API behavior

- The immutable tuple is the complete `executionBinding` object plus the
  session row's own Coven-canonical `project_root` and assigned session id.
  Nothing else is added to it.
- The binding is persisted atomically with session-row creation in a
  nullable `execution_binding_json TEXT` column on the session row; there is
  no separate binding table.
- No route may update any field of an existing session's `execution_binding`
  after creation. A stored binding round-trips deterministically, byte for
  byte, across a daemon restart.
- A `NULL` stored `execution_binding_json` is the only representation of an
  unbound session. If a non-null stored value fails to parse as valid JSON,
  or its `contract` does not equal `psyche.execution_binding.v1`, reading
  that row is a store error — it is never silently treated as unbound.
- `GET /api/v1/sessions/:id` and any listing route return the same typed
  `execution_binding` value unchanged by archive state, cursor position, or
  lifecycle status.

### Bound input and kill

`POST /api/v1/sessions/:id/input` on a bound session requires the complete,
exact `executionBinding` object alongside the existing `data` payload:

```json
{
  "data": "existing input payload, unchanged shape",
  "executionBinding": { "...": "complete object, matching the JSON shape above" }
}
```

`POST /api/v1/sessions/:id/kill` on a bound session, which today carries no
body, gains a JSON body carrying only the binding:

```json
{
  "executionBinding": { "...": "complete object, matching the JSON shape above" }
}
```

For both routes:

- A missing or incomplete proof fails closed: `400 execution_binding_required`,
  `details.fields: ["executionBinding"]` (or the specific missing member's
  path).
- A malformed proof shape fails as `400 execution_binding_invalid`; an
  unrecognized `contract` value fails as `400 execution_binding_unsupported`.
- A present, well-formed proof that byte-differs from the stored binding on
  any field fails as `409 execution_binding_mismatch`, naming only the first
  mismatched field path per the conventions above.
- **Input** additionally rejects an expired binding: `409 execution_binding_expired`,
  `details.fields: ["executionBinding.expiresAt"]`. Input never proceeds
  against an expired binding.
- **Kill is explicitly exempt from the expiry check.** An exact-matching
  proof whose `expiresAt` has already elapsed still succeeds, because kill
  only narrows authority (stops a running attempt) and preserves operator
  safety. Kill still requires an exact match on every other field.
- Read/list/events endpoints (`GET /api/v1/sessions/:id`,
  `GET /api/v1/sessions`, event/cursor reads) require no binding proof; they
  return the stored `execution_binding` field as-is. Coven defines
  correlation here, not authentication — read access is unchanged from
  today.

Once the proof is required, an unbound session's input/kill precedence is
completely unaffected: no proof check runs, and the existing status/liveness
gate, body shape, and response are unchanged from before O2. Legacy unbound
launches, inputs, and kills that never mention `executionBinding` behave
identically to their pre-O2 shape.

#### Operation precedence

**Launch** (`POST /api/v1/sessions`):

1. Existing JSON body parsing, `projectRoot`/`cwd` resolution, and harness
   validation, unchanged from today.
2. `executionBinding` contract identity, shape, expiry, and root/child
   cross-field validation, if `executionBinding` is present.
3. Existing familiar resolution (`resolve_familiar`), unchanged from today.
4. Canonical familiar-equality check and, for a child binding, parent lookup
   and exact correlation.
5. Existing maintenance-gate check, unchanged from today.
6. Atomic session-row insert, including `execution_binding_json` if present.

No session row is created, and no existing session state is mutated, unless
every step through 5 succeeds. For an unbound launch, steps 2 and 4 do not
apply and the remaining precedence is unchanged from today.

**Input and kill** (`POST /api/v1/sessions/:id/input`,
`POST /api/v1/sessions/:id/kill`):

1. Existing session lookup by id (`404 session_not_found` if absent),
   unchanged from today. A missing session wins over a malformed proof: an
   unparseable `executionBinding` against a nonexistent session still
   reports `session_not_found`.
2. If the session is bound, require and parse the request's
   `executionBinding` (`execution_binding_required` if missing/incomplete,
   `execution_binding_invalid` if malformed, or `execution_binding_unsupported`
   if its contract is unknown).
3. Exact comparison of the parsed binding against the stored binding
   (`execution_binding_mismatch` on any field difference — this wins even
   over a not-live or external-session response that would otherwise apply
   later).
4. For input only, expiry check (`execution_binding_expired`); kill has no
   expiry check, per its explicit exception above.
5. Existing status/liveness and external-session checks, unchanged from
   today.
6. The runtime action itself (deliver input, or send kill).

For an unbound session, steps 2-4 are skipped entirely and existing
precedence and behavior (steps 1, 5, 6) are unchanged. No runtime action
occurs unless every required prior step succeeds.

#### Metadata isolation

`executionBinding` is proof metadata consumed entirely by the API layer; it
never reaches the harness/runtime or a recorded event, on any code path,
including error paths:

- **Input:** only the existing `data` field reaches the session runtime's
  input call; `executionBinding` is stripped from the parsed body first. The
  persisted input event is built from `data` only — its pre-O2 shape,
  containing no `executionBinding` key.
- **Kill:** the binding proof exists solely to satisfy the exact-match and
  (non-)expiry checks above. It is never passed to the runtime's kill call,
  which continues to take only the session id, and the persisted kill event
  remains the pre-O2 shape — a bare `{"status": "killed"}` marker, no binding
  fields.

### Health negotiation

`GET /api/v1/health` advertises `capabilities.executionBindingContracts`
additively (see [Capability fields](#capability-fields)):

```json
{
  "capabilities": {
    "executionBindingContracts": ["psyche.execution_binding.v1"]
  }
}
```

A client requiring execution binding must confirm
`"psyche.execution_binding.v1"` is present before sending a bound launch. An
unknown or missing required contract value fails before any dependent
request, per the existing fail-closed rule. Legacy and unbound sessions
remain fully compatible: a launch that omits `executionBinding` behaves
exactly as it does today.

Externally registered (non-Coven-owned) sessions must reject any
`executionBinding` supplied at registration time (see
[`POST /api/v1/sessions/external`](#post-apiv1sessionsexternal)), because
Coven does not supervise that runtime and cannot honor bound-operation
guarantees for it.

### Error matrix

| Code | Status | Condition |
|---|---:|---|
| `execution_binding_invalid` | 400 | Malformed, missing a required field, or contains an unknown/extra member in `executionBinding` or its nested `parent`; fails a root/child cross-field or canonical-familiar-presence rule at launch; malformed binding proof (including an unknown/extra member) on bound input/kill; or an externally registered session's registration request supplies `executionBinding` at all. |
| `execution_binding_unsupported` | 400 | `contract` is not `psyche.execution_binding.v1`. |
| `execution_binding_required` | 400 | Bound input or kill omits or supplies incomplete binding proof. |
| `execution_binding_expired` | 409 | Launch or input references a binding whose `expiresAt` has elapsed. Kill is exempt (see above). |
| `execution_binding_mismatch` | 409 | Any exact-match check fails, including parent correlation, canonical-familiar correlation, or a bound input/kill proof that byte-differs from the stored binding. This includes a child launch whose `parent.sessionId` exists but carries a `null` stored `execution_binding` — details name only `parent.sessionId` in that case. |
| `session_not_found` | 404 | The current session, or a child launch's referenced `parent.sessionId`, does not exist at all. Unchanged from existing behavior (see [Stable error codes](#stable-error-codes)). |

`details.fields` names only the mismatched/invalid field path (e.g.
`executionBinding.graphId`, `parent.attemptId`); it never includes field
values or digests. No broader denial taxonomy is introduced by this
contract.

### Non-goals

This contract defines only the immutable launch/correlation core:

- No adoption key, uniqueness index, single-use/replay protection, or
  lookup-by-binding route. Two sessions may be launched with byte-identical
  `executionBinding` objects, including identical `requestDigest` values,
  and both succeed — a repeated valid proof is indistinguishable from a
  replay or duplicate adoption under this contract.
- No return-or-fence lookup semantics and no cancellation acknowledgement.
- No content-addressed artifact binding and no crash-matrix recovery proofs
  beyond deterministic persistence and restart round-trip.
- No broader structured-denial taxonomy beyond the six error codes above.
- No interpretation of `graphId`/`nodeId`/`attemptId` topology, descendant
  enumeration, or delegation authorization — `callerFamiliarId` is
  correlation metadata only, never a delegation-authority decision.
- No production child/subagent dispatch.

## Event record shape and cursor pagination (`v1`)

`GET /api/v1/events` returns a paginated envelope with monotonic `seq` cursors. `GET /api/v1/sessions/:id/events` is the session-scoped alias with the same response shape and cursor query parameters except that `sessionId` comes from the path.

### Query parameters

| Parameter     | Required | Description                                             |
|---------------|----------|---------------------------------------------------------|
| `sessionId`   | Yes      | Session to fetch events for.                           |
| `afterSeq`    | No       | Return only events with `seq > afterSeq` (preferred).  |
| `afterEventId`| No       | Compatibility cursor — resolves to a sequence position.|
| `limit`       | No       | Maximum number of events to return (daemon-enforced, max 1000). |

### Response envelope

```json
{
  "events": [
    {
      "seq": 42,
      "id": "event-uuid-a",
      "session_id": "session-uuid",
      "kind": "output_truncated",
      "payload_json": "{\"droppedEvents\":3,\"droppedBytes\":128}",
      "created_at": "2026-05-09T06:43:09Z"
    },
    {
      "seq": 43,
      "id": "event-uuid-b",
      "session_id": "session-uuid",
      "kind": "output",
      "payload_json": "{\"data\":\"hello\"}",
      "created_at": "2026-05-09T06:43:10Z"
    }
  ],
  "nextCursor": {
    "afterSeq": 43
  },
  "hasMore": false
}
```

`nextCursor` is `null` when there are no events. `hasMore` is `true` when a `limit` was applied and more events may exist.

`payload_json` is the redacted preview payload used by clients. Raw sensitive artifacts are never included in this envelope. `output_truncated` is additive and ordered: it appears in the session event stream before the next accepted event for the same session, and it uses `{"droppedEvents": <u64>, "droppedBytes": <u64>}` with `droppedBytes` counting rejected UTF-8 payload bytes only.

Adjacent accepted output callbacks for the same session may coalesce into one `output` event, and that event's `created_at` is the first accepted callback timestamp.

## Log preview shape (`v1`)

`GET /api/v1/sessions/:id/log` currently returns the full redacted log preview for the session as an unbounded array:

```json
[
  {
    "ts": "2026-05-09T06:43:10Z",
    "level": "info",
    "message": "> hello"
  }
]
```

## Travel mode profile and delta shapes (`v1`)

Travel mode lets a same-user local client export a bounded, read-only working profile for laptop/offline work and later reconcile appended results back into the hub store. It is additive to sessions/events: uploaded offline events are persisted as ordinary redacted event-log entries on a reconciliation session.

### `POST /api/v1/travel/profiles`

Request:

```json
{
  "familiarId": "sage",
  "workspaceId": "workspace-1",
  "expiresInSeconds": 604800,
  "staleAfterSeconds": 172800
}
```

`familiarId` is required. `workspaceId` defaults to `"default"`. Expiry values must be positive when supplied; defaults are 7 days for `expiresInSeconds` and 2 days for `staleAfterSeconds`, capped at the expiry.

Response `201`:

```json
{
  "profileId": "travel_...",
  "version": "0.1",
  "generatedAt": "2026-07-04T12:00:00Z",
  "expiresAt": "2026-07-11T12:00:00Z",
  "staleAfter": "2026-07-06T12:00:00Z",
  "sourceHub": {
    "hubId": "hub_...",
    "displayName": "Coven hub"
  },
  "scope": {
    "familiarId": "sage",
    "workspaceId": "workspace-1"
  },
  "sourceRevision": {
    "memoryRevision": "mem_...",
    "loopRevision": "loop_..."
  },
  "permissions": {
    "mode": "travel-read-only",
    "allowedLocalAgents": ["lightweight"],
    "allowMemoryOverwrite": false,
    "allowHeavyweightLocalWork": false
  },
  "encoding": "gzip+base64",
  "contentHash": "sha256:...",
  "profileBlob": "..."
}
```

The daemon also writes a gzip profile artifact under `<covenHome>/travel/profiles/` and marks it read-only. The profile payload may include familiar memory context for the requested familiar; clients must treat it as a snapshot, not a write target.

### `POST /api/v1/travel/deltas`

Request:

```json
{
  "profileId": "travel_...",
  "sourceHubId": "hub_...",
  "sourceRevision": {
    "memoryRevision": "mem_...",
    "loopRevision": "loop_..."
  },
  "clientId": "laptop-1",
  "events": [
    { "id": "local-event-1", "kind": "assistant", "text": "offline result" }
  ],
  "artifacts": [
    { "id": "artifact-1", "kind": "summary" }
  ],
  "proposedMemoryAdditions": [
    { "path": "MEMORY.md", "text": "append this" }
  ]
}
```

`profileId`, `sourceHubId`, and `clientId` are required. Query `state` may be `handoff_pending`, `syncing_delta`, or `hub_resumed`; omitted state defaults to `hub_resumed`. Query `defer=1` is a compatibility alias for `state=handoff_pending`.

Response `202`:

```json
{
  "deltaId": "delta_...",
  "state": "hub_resumed",
  "acceptedEvents": 1,
  "acceptedArtifacts": 1,
  "memoryReviewState": "queued",
  "canonicalMemoryOverwriteApplied": false,
  "reconciliationSessionId": "travel-delta_...",
  "hubRevision": {
    "memoryRevision": "mem_...",
    "loopRevision": "loop_..."
  }
}
```

The daemon appends offline events as `travel.offline_event` and offline artifacts as `travel.offline_artifact` entries on the reconciliation session. Proposed memory additions are queued for review; canonical memory overwrite is never applied by this endpoint.

### `GET /api/v1/travel/state`

Query parameters:

| Parameter   | Required | Description                                      |
|-------------|----------|--------------------------------------------------|
| `clientId`  | Yes      | Client whose latest travel delta state is read. |
| `profileId` | No      | Profile to evaluate before any delta exists.    |

Response `200`:

```json
{
  "state": "travel_local",
  "profileId": "travel_...",
  "pendingDeltaBytes": 0,
  "lastSyncError": null,
  "hubReachable": false,
  "profileFreshness": "fresh",
  "travelExecutionAllowed": true,
  "validStates": [
    "hub_active",
    "travel_local",
    "travel_stale",
    "handoff_pending",
    "syncing_delta",
    "hub_resumed"
  ]
}
```

`profileFreshness` is `fresh`, `stale`, `expired`, `none`, or `unknown`. Expired profiles return `travelExecutionAllowed: false`; local clients should fail closed when that flag is false.

## Scheduler decision and recovery shapes (`v1`)

The scheduler routes multi-host work across local laptop, stationary, hub, and compute executor roles. Decisions are stored so clients can inspect prior routing and recover loop state after daemon restart.

### `POST /api/v1/scheduler/decisions`

Request:

```json
{
  "jobId": "job-gpu-loop",
  "requiredCapabilities": ["gpu", "long-running-loop"],
  "taskWeight": "heavyweight",
  "travelState": "hub_active",
  "allowHeavyweightLocalWork": false,
  "nodes": [
    {
      "nodeId": "node-compute-idle",
      "role": "compute_executor",
      "available": true,
      "capabilities": ["gpu", "long-running-loop"],
      "queuePressure": 1
    }
  ]
}
```

Response `201`:

```json
{
  "decisionId": "sched_...",
  "jobId": "job-gpu-loop",
  "target": {
    "role": "compute_executor",
    "nodeId": "node-compute-idle"
  },
  "reason": "compute_executor has required capability set and low queue pressure",
  "inputs": {
    "requiredCapabilities": ["gpu", "long-running-loop"],
    "queuePressure": "low",
    "travelState": "hub_active",
    "taskWeight": "heavyweight",
    "nodesSource": "request_snapshot"
  },
  "createdAt": "2026-07-04T12:00:00Z"
}
```

The daemon filters unavailable nodes, required capability misses, low-battery `laptop_local` nodes during travel, and heavyweight laptop-local work while `travelState` is `travel_local` or `travel_stale` unless explicitly allowed.

`nodes` is optional. When it is omitted or empty, candidates are loaded from the persistent hub node registry instead (`inputs.nodesSource` is `"hub_registry"`); supplying a `nodes` snapshot keeps the request fully deterministic for failure simulations. An empty snapshot with an empty registry returns `409 no_scheduler_target`.

### `GET /api/v1/scheduler/decisions/:id`

Returns the same shape as `POST /api/v1/scheduler/decisions` for a persisted decision, or `404 scheduler_decision_not_found`.

### `POST /api/v1/scheduler/redispatch`

Request:

```json
{
  "loopId": "loop-gpu",
  "jobId": "job-gpu-loop",
  "currentNodeId": "compute-primary",
  "requiredCapabilities": ["gpu", "long-running-loop"],
  "loopResumable": true,
  "nodes": [
    {
      "nodeId": "compute-primary",
      "role": "compute_executor",
      "available": false,
      "capabilities": ["gpu", "long-running-loop"],
      "queuePressure": 3,
      "queuedJobIds": ["job-gpu-loop"]
    },
    {
      "nodeId": "compute-fallback",
      "role": "compute_executor",
      "available": true,
      "capabilities": ["gpu", "long-running-loop"],
      "queuePressure": 1
    }
  ]
}
```

Response `202`:

```json
{
  "decisionId": "sched_...",
  "state": "redispatched",
  "loopId": "loop-gpu",
  "jobId": "job-gpu-loop",
  "target": {
    "role": "compute_executor",
    "nodeId": "compute-fallback"
  },
  "reason": "compute-primary went offline; redispatched resumable loop to compute-fallback",
  "preservedSubqueue": {
    "nodeId": "compute-primary",
    "jobIds": ["job-gpu-loop"]
  },
  "nodeAvailability": [
    {
      "nodeId": "compute-primary",
      "role": "compute_executor",
      "available": false,
      "queuePressure": "medium"
    }
  ],
  "hubJobSynced": true,
  "createdAt": "2026-07-04T12:00:00Z"
}
```

If the loop is not resumable or no alternate node matches, `state` is `paused` and `target` is `{ "role": "paused", "nodeId": null }`. In both cases, the failed node subqueue is preserved.

`nodes` is optional. When omitted or empty, both the failed node and the redispatch candidates are resolved from the persistent hub node registry, with subqueue contents taken from the persistent per-executor queues (`inputs.nodesSource` on the persisted decision is `"hub_registry"`). If `currentNodeId` is not in the registry either, the call fails with `400 invalid_request`.

`hubJobSynced` reports whether the job is tracked in the hub's persistent global queue. When `true`, the redispatch also updated hub state so the outcome is visible at `GET /api/v1/hub/jobs/:jobId` and `GET /api/v1/hub/status`:

- `redispatched` — the job becomes `assigned` to the new node, the routing table points at it, and both nodes' subqueues are rebuilt.
- `paused` — the job becomes `held` on its current node without leaving that node's subqueue.

Snapshot-only jobs (not enqueued via `POST /api/v1/hub/jobs`) leave hub state untouched (`hubJobSynced: false`), which keeps deterministic failure-simulation fixtures independent of the registry.

### `GET /api/v1/scheduler/loops/:loopId`

Returns the persisted redispatch/pause state with the same fields as `POST /api/v1/scheduler/redispatch`, plus `updatedAt`, or `404 scheduler_loop_not_found`.

## Hub control-plane shapes (`v1`)

The hub control plane is the durable multi-host state described in `specs/coven-multi-host-daemon`: a persistent node registry, a routing table, a global job queue, and per-executor subqueues. All hub state persists in the daemon SQLite store and reloads after a daemon restart. Hub job assignment routes against the persistent registry; the `POST /api/v1/scheduler/*` routes also fall back to the registry whenever a request omits its `nodes` snapshot.

### `POST /api/v1/hub/nodes`

Registers a node or re-registers an existing one (updating role, transport, capabilities, and availability). Returns `201` for a new node and `200` for a re-registration.

Request:

```json
{
  "nodeId": "compute-primary",
  "role": "compute_executor",
  "transport": "ssh",
  "transportConfig": {
    "kind": "ssh",
    "host": "compute-primary.internal",
    "user": "coven",
    "port": 22,
    "identityFile": "/var/lib/coven/keys/id_ed25519"
  },
  "capabilities": ["gpu", "long-running-loop"],
  "available": true
}
```

Response:

```json
{
  "nodeId": "compute-primary",
  "role": "compute_executor",
  "transport": "ssh",
  "transportConfig": { "kind": "ssh", "host": "compute-primary.internal", "user": "coven", "port": 22, "identityFile": "/var/lib/coven/keys/id_ed25519" },
  "capabilities": ["gpu", "long-running-loop"],
  "available": true,
  "queuePressure": 0,
  "lastHealthAt": "2026-07-06T12:00:00Z",
  "lastError": null,
  "registeredAt": "2026-07-06T12:00:00Z",
  "updatedAt": "2026-07-06T12:00:00Z"
}
```

`transport` defaults to `"ssh"`. `queuePressure` is hub-computed from the node's persistent subqueue and cannot be set by the caller. `transportConfig` is the structured hub-outbound dispatch link (`kind: "ssh"` or `kind: "local"` for private-network/same-host process dispatch); it is validated at registration, required before the hub can poll or dispatch to the node, and preserved when a re-registration omits it.

### `GET /api/v1/hub/nodes` and `GET /api/v1/hub/nodes/:nodeId`

List all registered nodes (`{ "nodes": [ ... ] }`) or fetch one node record. Unknown ids return `404 node_not_found`.

### `POST /api/v1/hub/nodes/:nodeId/health`

Records an executor health report and updates `lastHealthAt`:

```json
{
  "available": false,
  "capabilities": ["gpu", "long-running-loop"]
}
```

Availability transitions move the node's jobs between `assigned` and `held` without removing them from the node's persistent subqueue:

- `available: false` — every `assigned` job on the node becomes `held`; the subqueue and loop ids are preserved.
- `available: true` — every `held` job on the node returns to `assigned`.

Response:

```json
{
  "node": { "nodeId": "compute-primary", "available": false, "queuePressure": 1 },
  "heldSubqueue": { "nodeId": "compute-primary", "jobIds": ["job_01J..."] },
  "transitionedJobs": { "from": "assigned", "to": "held", "jobIds": ["job_01J..."] }
}
```

### `POST /api/v1/hub/nodes/:nodeId/poll`

Hub-initiated availability poll for the stateless executor protocol (`coven.executor.v1`). The hub connects **outbound** over the node's registered `transportConfig` (SSH batch mode with pinned host keys, or a local/private-network process launch), runs `coven executor probe`, and records the advertised capabilities plus last-known availability. Executors never push registration or heartbeats to the hub.

The response always returns `200` with the poll outcome; failures are recorded on the node (`available: false`, `lastError`), never fatal:

```json
{
  "nodeId": "compute-primary",
  "ok": true,
  "probe": {
    "protocolVersion": "coven.executor.v1",
    "role": "compute_executor",
    "capabilities": ["shell", "gpu"],
    "available": true,
    "queuePressure": 0,
    "covenVersion": "0.0.0",
    "probedAt": "2026-07-06T12:00:00Z"
  },
  "heldSubqueue": { "nodeId": "compute-primary", "jobIds": [] },
  "node": { "nodeId": "compute-primary", "available": true }
}
```

Availability transitions from a poll move the node's jobs between `assigned` and `held` exactly like a health report. A probe that advertises a role different from the registered one fails closed (`ok: false`, node unavailable). Nodes registered without a `transportConfig` return `409 node_transport_not_configured`.

### `POST /api/v1/hub/nodes/:nodeId/dispatch`

Hub-outbound job dispatch. The hub sends a full-context job spec (argv, cwd, env, stdin payload, timeout, opaque `context` blob) to `coven executor run-job` on the node, so the stateless executor needs no local durable authority.

Request:

```json
{
  "jobId": "job_01J...",
  "command": ["sh", "-c", "…"],
  "cwd": "/work/checkout",
  "env": { "KEY": "value" },
  "stdin": "optional payload",
  "timeoutSeconds": 300,
  "requiredCapabilities": ["gpu"],
  "context": { "workspaceId": "workspace_01J..." }
}
```

`jobId` is optional (the hub generates `job_<uuid>` when omitted). Required capabilities are checked against the node's last-known capability metadata (`409 executor_capability_mismatch`). The executor replies with a normalized result envelope, persisted with the dispatch record:

```json
{
  "jobId": "job_01J...",
  "nodeId": "compute-primary",
  "createdAt": "2026-07-06T12:00:00Z",
  "envelope": {
    "protocolVersion": "coven.executor.v1",
    "jobId": "job_01J...",
    "status": "completed",
    "exitCode": 0,
    "stdout": "…",
    "stderr": "…",
    "startedAt": "2026-07-06T12:00:00Z",
    "finishedAt": "2026-07-06T12:00:05Z",
    "durationMs": 5000,
    "error": null
  }
}
```

Envelope `status` is one of `completed`, `failed`, `timeout`, `rejected`, or `transport_error` (synthesized by the hub-side dispatcher when the node is unreachable or replies with a malformed envelope, returned as `502 executor_unreachable` with the envelope in `details`). A dispatch doubles as an availability observation, and when `jobId` names a job on the hub queue, that job's state advances from the envelope (`completed`, or `failed` for `failed`/`timeout`/`rejected`); a transport error leaves the queued job held so no work is lost.

### `GET /api/v1/hub/dispatches/:jobId`

Returns the persisted dispatch record — full job spec, normalized result envelope (or `null` while in flight), status, node id, and timestamps — or `404 executor_job_not_found`.

### `POST /api/v1/hub/jobs`

Enqueues a job on the persistent global queue with state `queued`. `jobId` is optional (the hub generates `job_<uuid>` when omitted). Duplicate ids return `409 job_already_queued`.

```json
{
  "jobId": "job_01J...",
  "requiredCapabilities": ["gpu"],
  "priority": 5,
  "loopId": "loop_01J...",
  "payload": { "kind": "loop-run" }
}
```

Job states: `queued`, `assigned`, `held`, and the terminal states `completed`, `failed`, `cancelled`.

### `GET /api/v1/hub/jobs?state=...` and `GET /api/v1/hub/jobs/:jobId`

List queued jobs (optionally filtered by `state`, ordered by priority then age) or fetch one job. The single-job response includes the job's routing-table entry under `route` (or `null` when unrouted).

### `POST /api/v1/hub/jobs/:jobId/assign`

Assigns the job to an executor from the persistent node registry. With an empty body, the hub picks the best available node by capability match, then lowest queue pressure, then role rank (`compute_executor` before `stationary_executor` before `hub` before `laptop_local`). Passing `{ "nodeId": "..." }` forces a specific registered node (`409 node_unavailable` / `409 node_missing_capabilities` when it cannot take the job).

On success the hub persists, in one pass:

- the job's state (`assigned`) and `assignedNodeId`;
- a routing-table entry mapping the job to the node;
- a scheduler decision record (readable at `GET /api/v1/scheduler/decisions/:id`); and
- the target node's rebuilt subqueue and queue pressure.

If no registered node qualifies, the call returns `409 no_available_node` and the job stays `queued`.

### `POST /api/v1/hub/jobs/:jobId/complete`

Marks a job terminal (`{ "state": "completed" | "failed" | "cancelled" }`, defaulting to `completed`), removes it from its executor subqueue, and refreshes the node's queue pressure. Terminal jobs cannot be reassigned (`409 job_not_assignable`).

### `GET /api/v1/hub/routing`

Returns the persistent routing table:

```json
{
  "routes": [
    {
      "jobId": "job_01J...",
      "nodeId": "compute-primary",
      "decisionId": "sched_01J...",
      "reason": "compute-primary selected from hub registry by capability match and queue pressure",
      "createdAt": "2026-07-06T12:00:00Z",
      "updatedAt": "2026-07-06T12:00:00Z"
    }
  ]
}
```

### `GET /api/v1/hub/status`

Returns the hub role, identity, node availability, and queue depths:

```json
{
  "role": "hub",
  "hubId": "hub_01J...",
  "nodes": [ { "nodeId": "compute-primary", "available": true, "queuePressure": 1 } ],
  "nodesTotal": 1,
  "nodesAvailable": 1,
  "globalQueue": { "queued": 0, "assigned": 1, "held": 0, "total": 1 },
  "executorQueues": [ { "nodeId": "compute-primary", "jobIds": ["job_01J..."], "updatedAt": "2026-07-06T12:00:00Z" } ]
}
```

`hubId` is the same stable identity embedded as `sourceHub.hubId` in generated travel profiles.

Restart and supervision guidance for hub daemons lives in [`HUB-OPERATIONS.md`](HUB-OPERATIONS.md).

## Raw artifact access (`v1`)

`GET /api/v1/sessions/:id/artifacts/:artifactId?raw=1` is intentionally narrow. It is unavailable unless raw artifact persistence is explicitly enabled in local privacy settings. Disabled installs return:

```json
{
  "error": {
    "code": "raw_artifacts_disabled",
    "message": "Raw artifact persistence is not enabled.",
    "details": {
      "sessionId": "session-1",
      "artifactId": "event-1"
    }
  }
}
```

### Incremental read pattern

1. Poll `GET /events?sessionId=<id>` to get all events (with optional `limit`).
2. Use `nextCursor.afterSeq` in subsequent requests: `GET /events?sessionId=<id>&afterSeq=<seq>`.
3. Repeat until `hasMore` is `false`.

This gives clients stable incremental reads. Exactly-once delivery also requires client-side checkpointing and idempotency.

```mermaid
sequenceDiagram
  participant Client
  participant Daemon as /api/v1/events

  Client->>Daemon: GET ?sessionId=S1
  Daemon-->>Client: { events: [seq 1..50], nextCursor: { afterSeq: 50 }, hasMore: true }
  Client->>Client: persist last seq = 50
  Client->>Daemon: GET ?sessionId=S1&afterSeq=50
  Daemon-->>Client: { events: [seq 51..78], nextCursor: { afterSeq: 78 }, hasMore: false }
  Client->>Client: persist last seq = 78

  note over Client,Daemon: Client crash + restart
  Client->>Daemon: GET ?sessionId=S1&afterSeq=78
  Daemon-->>Client: { events: [seq 79..82], nextCursor: { afterSeq: 82 }, hasMore: false }
```

Persisting `afterSeq` survives daemon restarts: events are append-only and seq numbers are monotonic, so a resumed poll always picks up where it stopped.

## Live control response shapes (`v1`)

Both live-control endpoints return the same accepted response shape on success:

- `POST /api/v1/sessions/:id/input`
- `POST /api/v1/sessions/:id/kill`

```json
{
  "ok": true,
  "accepted": true
}
```

Shared non-success responses use the structured error envelope:

- `404` when the session does not exist:

```json
{
  "error": {
    "code": "session_not_found",
    "message": "Session was not found.",
    "details": { "sessionId": "session-1" }
  }
}
```

- `409` when the session exists but is not live:

```json
{
  "error": {
    "code": "session_not_live",
    "message": "Session is not live.",
    "details": { "sessionId": "session-1" }
  }
}
```

A bound session (one launched with `executionBinding`) additionally requires
a complete, exact-matching `executionBinding` proof in the request body
before any of the above; see
[Psyche execution binding contract (`v1`)](#psyche-execution-binding-contract-v1)
for the request shape, precedence, and the additional
`execution_binding_required`/`execution_binding_invalid`/
`execution_binding_unsupported`/`execution_binding_expired`/
`execution_binding_mismatch` error responses that apply only to bound
sessions.

## comux and OpenClaw bridge compatibility

- comux reads the `capabilities` object from `/api/v1/health` to decide which features to use.
- The external OpenClaw bridge plugin (`packages/openclaw-coven`) is updated in this repo alongside the daemon and uses `apiVersion === "coven.daemon.v1"` as its contract guard.
- Client updates to use `afterSeq` cursors and paginated event envelopes may happen independently of the daemon update; the daemon-enforced shape is the source of truth.
- The `supportedApiVersions` field has been removed from the health response in `coven.daemon.v1`; clients should check `apiVersion` directly.

## Compatibility and migration policy

- `coven.daemon.v1` clients may rely on the documented field names and top-level response shapes above.
- Additive fields are backward compatible. Clients should ignore unknown fields when safe.
- Any incompatible change must ship under a new `apiVersion` value exposed by `GET /api/v1/health` or its successor route.
- Before a client switches to a new major contract, the Coven repo should publish updated contract docs and a migration note that maps the old shape to the new one.

## Recommended client handshake

1. Call `GET /api/v1/health`.
2. Verify `apiVersion === "coven.daemon.v1"` and `capabilities.structuredErrors === true`.
3. Verify `capabilities.sessions === true` before session requests and
   `capabilities.events === true` before event requests.
4. Check `capabilities.eventCursor === "sequence"` before using `afterSeq` pagination.
5. Check `capabilities.sessionLaunchPolicy === true` before sending
   `launchPolicy`; a missing, false, or malformed value means unsupported.
6. Check `capabilities.executionBindingContracts` includes
   `"psyche.execution_binding.v1"` before sending a bound `executionBinding`
   launch, input, or kill.
7. Only then depend on the documented `v1` sessions/events shapes.

## Scope boundary

The `coven.daemon.v1` contract covers daemon health, capability discovery, action routing, sessions, events, live input, live kill, travel-mode profile/delta reconciliation, scheduler decision/recovery routes, and the Psyche execution binding contract described above. Do not treat route names outside this document as reserved API until they are implemented and documented here.
