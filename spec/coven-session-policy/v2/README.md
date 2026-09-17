# Coven session-policy positive acceptance v2

Contract: `coven.session-policy.v2`.
Owner: Coven Rust. Consumer implementations: OpenCoven SDK and Wand.
Decision authority: BunsDev, through the September 10, 2026 delegation of
cross-repository design and implementation, exercised again on September 17,
2026 ("proceed with recommendations") for the records in this file.
Tracking: OpenCoven/coven#1128 (this revision), OpenCoven/coven#1083 /
#1117 / #1127 (backend), OpenCoven/wand#9 / OpenCoven/wand#10 (consumer gate),
OpenCoven/sdk#199 (typed transport, follows this revision).

## Status, in one paragraph

This revision is an **accepted contract shape with no implementation**.
`coven.session-policy.v1` is unchanged and still refusal-only; there is no
`GET /api/v2/session-policy`, no `POST /api/v2/sessions/restricted`, and the
v1 route keeps refusing the v2 contract literal with `400 invalid_request`
(pinned by `session_policy.rs` tests). The support matrix below carries one
row, and that row is **ineligible** because two of its dimensions are
`unknown`; discovery therefore stays `enforcement: "unavailable"` with an
empty `supportedProfiles`. Nothing here is a grant, a badge, or evidence that
any production launch is confined.

## Decision record (September 17, 2026)

These are the decisions the v1 README and the backend README deferred to the
maintainers. Each is recorded so it can be cited, and each is reversible by
editing this file under the same authority.

| Decision | Record |
| --- | --- |
| G1 — contract acceptance | The shape in *Wire* below is accepted as the v2 revision. Discovery and admission live on new `/api/v2/…` routes; v1 routes, fixtures and refusal correlation are unchanged. Reuse of the automations adoption contracts is **not** taken; this contract stays self-contained. |
| G2 — implementation and enforcement owner | Server implementation owner: Coven Rust (`crates/coven-cli` session-policy module). Enforcement backend owner: `crates/coven-restricted-runtime-macos` (Seatbelt), with `crates/coven-restricted-runtime` as the controller contract. SDK compatibility owner: OpenCoven/sdk, mapping to this revision after it lands. Accountable person for all three: BunsDev (delegated implementer: Cody). |
| Seatbelt decision — loader read exception | **Accepted.** The profile's read-only `/usr/lib`, `/dev/null` and Apple's `dyld-support.sb` rules are a loader closure a Mach-O cannot start without; they are not a configuration or home read. |
| Seatbelt decision — deprecated `sandbox_init` | **Accepted** for this backend. Apple's own `sandbox-exec` and system profiles use the same engine; an App Sandbox helper could not express the zero-write promise. Re-evaluate if Apple removes the interface. |
| Scope decision — what runs under the profile | The `workspace-readonly-no-network.v1` profile confines **one offline sealed worker** (`bin/coven-worker-target` in a caller-owned `0700` workspace). It denies all network. **No inference harness runs under it**: a `codex`/`claude`/`coven-code`/`copilot` session needs a provider connection, and no provider exception is implied or planned for this profile. A network-capable profile is a different backend and a different revision; consumers must not present this profile as confinement for a familiar session. |
| Residual gaps | Carried in the support row's `limitations`; none of the five listed is disqualifying for the offline-worker profile, and two (`denialEvents`, `eventContinuity`) are *unknown* rather than limited, which is what keeps the row ineligible. |

## Wire

Every v2 response body is at most **16,384 UTF-8 bytes**, as in v1, and the
same reading rules apply (enforce while reading, reject oversized or invalid
UTF-8, never truncate).

### Discovery

`GET /api/v2/session-policy` returns the closed object in
[`fixtures/discovery.json`](fixtures/discovery.json):

- `contract`: the literal `coven.session-policy.v2`.
- `enforcement`: `"unavailable"` unless at least one matrix row is `eligible`.
- `supportedProfiles`: the profiles of eligible rows only; empty today.
- `supportMatrix`: one row per backend tuple (see *Support matrix*).
- `reason`: why enforcement is unavailable; `support_row_has_unknown_dimensions`
  today.

Discovery is availability metadata, never permission or daemon identity.

### Restricted launch request

`POST /api/v2/sessions/restricted`, owner-local IPC only, closed object as in
[`fixtures/request.json`](fixtures/request.json):

```json
{
  "contract": "coven.session-policy.v2",
  "requestId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  "invocationId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
  "profile": "workspace-readonly-no-network.v1",
  "expiresAtUnixMs": 1800000000000,
  "worker": { "workspace": "/example/sealed-workspace", "leaseMs": 30000 }
}
```

`worker` replaces v1's `launch`: there is no harness, familiar, prompt or
title, because nothing but the sealed worker target runs. `workspace` is the
caller-owned `0700` directory the backend will `seal` (4,096 UTF-8 bytes max,
no NUL, opaque at admission). `leaseMs` is the finite worker lease, 1,000 to
3,600,000. All v1 envelope rules (UUID canonical form, admission window of
300,000 ms, 1 MiB body, depth 16, no duplicate or unknown keys) carry over.

### Accepted response

HTTP 200 with the closed object in [`fixtures/accepted.json`](fixtures/accepted.json).
It is issued **only after** `seal` succeeded and the controller reached
`Running` with `spawned <pid>` acknowledged; a server that cannot get there
answers with the v2 refusal instead.

| Field | Meaning |
| --- | --- |
| `requestDigest` | Lowercase `sha256:` of the exact transmitted request-body bytes, identical to v1's rule. An acceptance that does not correlate is echo, not a grant. |
| `decision` | `"accepted"`. |
| `enforced` | `true` only when `backend` matches an *eligible* matrix row byte-for-byte; otherwise `false`. A client treats `false` as no authority. |
| `backend` | `{ crate, revision, platform, profileDigest }`. `profileDigest` is `sha256:` of the exact single-line Seatbelt profile the backend installed (the manifest carries the text for the fixture workspace). |
| `grant` | `{ sessionId, workspaceIdentity, leaseMs, policyVersion, issuedAtUnixMs, expiresAtUnixMs }`. `workspaceIdentity` is the `dev:<n>,ino:<n>` pair the backend holds; `expiresAtUnixMs − issuedAtUnixMs == leaseMs`. |
| `receipts` | Where the lifecycle receipts arrive (`stream: "events"`, the session's event stream), the `required` kinds every accepted session must emit in order, the `terminal` kinds exactly one of which ends it, and the `pending` kind that may precede `restricted.terminated`. |

### Refusal

HTTP 409 with [`fixtures/refusal.json`](fixtures/refusal.json): v1's refusal
shape with the v2 contract literal. Codes: `enforcement_unavailable` (no
eligible row), `unsupported_backend` (row not eligible for this tuple),
`seal_refused` (the backend refused the workspace; `admission` is
`not_started`), `start_refused` (controller refused before `Running`;
`admission` is `not_started`). A refusal after `spawned` is impossible by
construction: that state is reported through receipts, not through admission.

### Receipts

[`fixtures/receipts.json`](fixtures/receipts.json) is one page of the
existing events wire (`seq`, `session_id`, `kind`, `payload_json`) showing the
required order: `restricted.sealed` (request digest, workspace identity,
closure manifest), `restricted.spawned` (`pid`), `restricted.lease`
(`remainingMs`, at least once), then `restricted.terminated` (`cause`,
`groupEmpty`, `leaderReaped`). `restricted.refused` is the other terminal;
`restricted.cleanup_pending` may appear before `restricted.terminated` and
means *possibly live*.

### Client obligations (refusal-first)

- An `accepted` without `restricted.spawned` in its receipts is **unknown
  start**, never "did not run".
- `restricted.cleanup_pending` is possibly live until `restricted.terminated`.
- A missing or expired lease is revoked authority.
- An unknown `backend` tuple, `enforced: false`, or a `requestDigest` that
  does not match the bytes sent is no authority.
- Never fall back to the legacy launch route for a request that carried a
  confinement requirement.

## Support matrix

One row today, carried verbatim in `fixtures/discovery.json`:

| Tuple | `coven-restricted-runtime-macos` at `b5ffbde5a3977395edd5f43e34c5d49c4a4f1d41`, `macos-26.6`, profile digest `sha256:dc8069fc…34fd7d` (see manifest) |
| --- | --- |
| Profile | `workspace-readonly-no-network.v1` (offline sealed worker) |
| targets | **supported** — reads confined to the sealed workspace subpath; inodes and closure manifest revalidated before exec |
| process | **supported** — `process-fork` denied, `process-exec` pinned to one literal path, leader fenced by pid and group |
| network | **supported** — `(deny default)` with no network allowance; enforcement of *denial*, not availability |
| deadline | **supported** — guardian-owned monotonic lease, handoff latency charged to the lease |
| revocation | **supported** — explicit cancel, owner death, lease expiry; all `SIGKILL` the group and leader |
| restart | **rejected** — the guardian survives controller and driver loss, but there is no daemon-restart re-attachment; a restarted daemon must treat the session as unknown |
| denialEvents | **unknown** — the kernel denies, the worker's probes report, but no `restricted.*` denial event is emitted on the wire yet |
| eventContinuity | **unknown** — receipts are not yet produced by any server; the ordering above is contract, not observed behaviour |
| Evidence | #1083 (6 cases), #1117 (boundary re-read), #1127 (pid fence, closure manifest; 9 cases); `Restricted runtime (macOS)` CI job on #1127; verified September 17, 2026 |
| Validity | while the crate revision is `b5ffbde5`; any crate, profile, platform or configuration change withdraws the row until re-verified |
| Limitations | no `fexecve` on macOS; guardian handoff not kernel-atomic; direct owner pid only; Seatbelt does not deny `setsid`/`setpgid` themselves; manifest proves no change after seal, not review |
| Eligible | **false** — two dimensions are unknown |

Unknown is ineligible. The row becomes eligible, and discovery may advertise
the profile, only when a server implementation emits the receipts and a
conformance run observes denial events and event continuity for this exact
tuple. That run needs its own authorization (v1's G5) before any `enforced:
true` is ever sent.

## Shared fixtures

[`fixtures/manifest.json`](fixtures/manifest.json) pins the admission time to
`1799999700000`, the Seatbelt profile text and digest for the fixture
workspace, and for every fixture its byte length and `sha256:` digest. Files
are UTF-8 JSON with a single trailing LF; digests include that LF, and the
`requestDigest` in `accepted.json`, `refusal.json` and the `restricted.sealed`
receipt is the digest of `request.json`'s exact 288 bytes:
`sha256:bed802052e0717763e35243bf80354efd1ea358b9374c64c2fe81d8da312a6b8`.
These are synthetic offline vectors, not live launch inputs or evidence of
enforcement.

## What this revision does not do

It does not add a server route, touch `SessionRuntime`, change v1, make any
matrix row eligible, or authorize a live conformance run. Wand's W08 stays
blocked on eligibility, and even an eligible row confines an offline worker,
not a familiar session.
