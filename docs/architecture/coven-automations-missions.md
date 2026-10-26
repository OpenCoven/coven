---
title: "Automations missions"
summary: "Approved contract (not implemented) for submitting, observing and cancelling one bounded familiar mission over coven.automations.v1."
read_when:
  - Designing or reviewing the mission commands for issue 1313
  - Building a client (SDK, CLI, Chat) that submits or observes a mission
source_adjacent_reason: "Defines the producer contract that OpenCoven/chat#155 and the SDK binding are built against."
---

# Automations missions

Status: **contract shape approved by Val on 2026-10-18. `mission.submit.v1`,
`mission.get.v1` and `mission.cancel.v1` are implemented; under M0 no mission
launches yet (see *Implementation notes*). The `mission-commands` conformance
suite covers the vectors M0 can reach.** Written for #1313,
which replaces OpenCoven/coven-agents#261. Consumers: OpenCoven/chat#155 (native
submit/observe/cancel) and the SDK client. Builds on
[coven-automations-v1.md](coven-automations-v1.md) and
[coven-automations-runtime-authority.md](coven-automations-runtime-authority.md).

A **mission** is one owner-submitted, one-shot invocation of an exact familiar
revision on one project. The owner submits it once, can watch it from any client
that restarts or reconnects, and can cancel it. It ends with one authoritative
terminal record. A mission is **not** a new lifecycle. It is a thin layer over
one manual occurrence of `coven.automations.v1`, which already has:

- adoption-key idempotency;
- deterministic manual occurrence ids;
- immutable attempts with dispatch fences;
- cancel, retry and recover (#1218–#1221);
- the event changefeed;
- owner grants.

This document defines only what the layer adds.

## Decisions

All decided by Val on 2026-10-18.

| # | Question | Proposal |
| --- | --- | --- |
| 1 | What can the first mission do? | **M0 is a read-only diagnosis report** on the `r0-read.1` envelope, the only one that launches. Running tests waits for an exec envelope with enforced confinement (M1, under #857). This narrows chat#155's "diagnosis/test" to diagnosis until M1. |
| 2 | Is a mission a new resource? | **No.** `mission.submit` atomically records an internal one-shot definition, adopts the owner grant for it, and claims its single manual occurrence. The mission id and the occurrence id are derived from the same adoption key. The definition is never listed, scheduled or revised (see *Implementation notes*). |
| 3 | Who authorizes a mission? | The **owner-local principal** (Decision 1 of the runtime-authority doc), over the owner-only socket or pipe. Submitting is the one-run owner grant, as `occurrence.runNow.v1` on an authority routine already is (decided 2026-10-05). TCP callers are refused before the store opens. |
| 4 | Approvals? | **Reserved, not produced.** M0 launches only R0. An approval would unlock nothing until a writable envelope exists, the same reasoning slice 7 used. `needs_approval` is in the state set so clients render it once it exists. |
| 5 | Where does usage come from? | The session's stream-json **result event**, recorded on the attempt when it settles and summed on the mission. Usage is reported, never estimated. A harness that reports none yields `usage: null`. |

## What a submit binds

Everything below is fixed at submit, and the request digest covers all of it.
Changing any field under the same adoption key is `ADOPTION_REPLAY_MISMATCH`.

| Field | Meaning | Checked |
| --- | --- | --- |
| `principal` | The owner-local principal, `principal:coven-local-owner:<hex>` | At submit, from the transport. Never from the body. |
| `familiar.rootId`, `familiar.revisionId` | The exact revision the client showed the user | At submit **and** at dispatch: the revision must be the ledger's current active revision of that root |
| `envelope` | `{ id: "r0-read.1", descriptorDigest }` | At submit: the daemon must advertise and enforce it. It advertises the list as `missionEnvelopes` on the `coven.automations` entry of `GET /api/v1/capabilities` (`coven missions envelopes --json`). |
| `project.cwd` | An absolute, canonical directory | At submit: it exists, is a directory, and is not inside `COVEN_HOME` |
| `task.statement` | A short title for the mission | 1–200 characters |
| `task.prompt` | What the familiar is asked to do | 1 byte to 32 KiB |
| `limits.timeoutMinutes` | Hard run timeout | 1–60 in M0 |
| `validUntil` | Dispatch must **begin** before this instant | After `requestedAt`, at most 15 minutes later |
| `scopes` (optional) | The project files and folders the familiar may read: 1–32 profile filesystem scopes, each `{ kind: "filesystem", root: "project", path, access: "read", recursive }`. Absent means none. | At submit, by the scoped read service's own rules (`Grants::from_scopes` under the R0 read policy), and each must be a `read` of the `project` root. See *Mission scopes*. |

## Commands

The commands join `CommandName` in `coven.automations.v1`. Each also gets a flat
`coven.automations.mission.*.v1` action on `POST /api/v1/actions`, matching the
existing automations actions. The `coven.automations.` prefix is what puts them
behind the owner-only transport gate (`automation_transport_rejection`) and in
the command matrix. Both forms go through the same command envelope, adoption
and error handling.

```ts
type MissionCommandName = "mission.submit.v1" | "mission.get.v1" | "mission.cancel.v1";

interface MissionSubmitPayload {
  familiar: { rootId: string; revisionId: string };
  envelope: { id: string; descriptorDigest: Digest };
  project: { cwd: string };
  task: { statement: string; prompt: string };
  limits: { timeoutMinutes: number };
  validUntil: Timestamp;
  scopes?: MissionScope[];  // 1–32 when present; absent means none
}

interface MissionScope {
  kind: "filesystem";
  root: "project";
  path: string;             // relative to the project root
  access: "read";
  recursive: boolean;
}

interface MissionSubmitResult {
  missionId: string;      // "mission-" + first 32 hex of sha256(adoptionKey); no ":" (id pattern)
  occurrenceId: string;   // the manual occurrence the mission projects
  mission: MissionView;   // state at commit, "queued" on first commit
}

interface MissionGetPayload { missionId: string }

interface MissionCancelPayload { missionId: string; reason?: string }   // reason ≤ 500 chars, as for other cancels

interface MissionCancelResult {
  cancellation: {
    cancellationId: string;
    requestedAt: Timestamp;
    outcome: "cancelled" | "pending" | "lost_to_completion" | "recovery_required";
  };
  mission: MissionView;
}
```

- **`mission.submit.v1`** commits these in one store transaction: the internal
  definition, the owner grant, the claimed manual occurrence, and the adoption
  record. Launch happens after commit, as for runNow. A replay returns the first
  commit's result with `outcome: "replayed"` and never launches again.
- **`mission.get.v1`** is a read. It requires no adoption key and returns the
  current `MissionView`.
- **`mission.cancel.v1`** maps onto `occurrence.cancel.v1` before dispatch and
  `run.cancel.v1` after it, with the existing stop-fence arbitration. Replaying
  the same adoption key returns the same cancellation. The inner
  `occurrence.cancel.v1` runs under a key derived from the mission cancel's
  (`mc:` and 40 hex of its SHA-256). That key is the `cancellationId`, and the
  view reads the cancellation back from its adoption. A mission that has
  already settled answers `ILLEGAL_TRANSITION`, with the settled view in
  `details.mission`. Under M0, every mission has settled before a cancel can
  arrive.
- **Events**: clients use `events.read.v1` / `events.subscribe.v1` on the
  mission's occurrence stream. Every event on it carries `automationId` equal to
  the `missionId`, because a mission's internal definition has the mission's id.
  Events do not carry the mission state. Each one raises the view's `revision`,
  and the client reads the state with `mission.get`.

## Mission scopes

Decided by Val on 2026-10-20 (coven#857, decision 1): a mission says what the
familiar may read with an optional `scopes` list of profile filesystem scopes
on the project root. Implemented for #1597 as contract and storage only.
Nothing reads the scopes yet, and a mission with scopes still settles
`unavailable` / `authority_unavailable` without a launch.

- **One rule set.** The daemon validates each scope with the scoped read
  service's `Grants::from_scopes` under the R0 read policy, the rules the
  service will enforce. That refuses a malformed or non-closed item, a
  root-wide path (`.`, `/`, or a recursive scope on a single top-level
  folder), an absolute or drive-qualified path, a path containing `..` or
  `*`, and a spelling the service does not interpret.
- **Narrower than the service.** The read service also accepts `workspace`
  roots, `write` access and scope kinds that grant no filesystem access. A
  mission binds only its project, so the submit also requires `kind:
  "filesystem"`, `root: "project"` and `access: "read"`.
- **Bounds.** At most 32 scopes (`maxItems`). An empty list is refused:
  absent already means none, so a present list must name something.
- **Refusals.** Every refusal is `VALIDATION_FAILED`, with a message naming
  the item (`scopes[i]`) and the rule it breaks. As for every submit refusal,
  it is adopted under its key and records no mission.
- **Storage.** The validated list is stored as canonical JSON (RFC 8785) in
  the mission row's `scopes_json` column and returned as `scopes` by every
  mission view. A mission without scopes stores `NULL`, and its view has no
  `scopes` key, exactly as before. The request digest covers the payload, so
  the same adoption key with different scopes is `ADOPTION_REPLAY_MISMATCH`.
- **Compatibility.** `missionSubmit` is a closed object, so a daemon from
  before this field refuses it. SDK and Chat must not send `scopes` until a
  daemon that accepts it is released (coordinated on sdk#80).

## Mission view and states

```ts
type MissionState =
  | "queued" | "running" | "needs_approval" | "cancelling" | "reconciling"
  | "completed" | "failed" | "cancelled" | "rejected" | "revoked" | "unavailable";

interface MissionView {
  missionId: string;
  occurrenceId: string;
  revision: number;                       // increases on every state change
  state: MissionState;
  terminal: boolean;
  reason?: MissionReason;                 // set for failed, rejected, revoked, unavailable, reconciling
  principal: PrincipalRef;
  familiar: { rootId: string; revisionId: string };
  envelope: { id: string; descriptorDigest: Digest };
  project: { cwd: string };
  scopes?: MissionScope[];                // as submitted, canonical; absent when none
  task: { statement: string };            // the prompt is not echoed
  submittedAt: Timestamp;
  validUntil: Timestamp;
  attempts: Array<{ attemptId: string; attemptNumber: number; state: AttemptState; failureClass?: string }>;
  cancellation?: MissionCancelResult["cancellation"];
  result?: MissionResult;                 // terminal states only
}

type MissionReason =
  | "timeout" | "harness_failed" | "familiar_revision_stale" | "familiar_revision_revoked"
  | "grant_revoked" | "envelope_unsupported" | "expired_before_dispatch"
  | "authority_unavailable" | "runtime_unavailable" | "ambiguous_execution";
```

`submitted` and `acknowledged` in chat#155 are **client** states:
- *Submitted* means the request left the client.
- *Acknowledged* is a `committed` or `replayed` response.

The daemon never reports either.

**Projection.** The daemon derives the mission state from the records it already
keeps. The first matching row wins:

| Mission state | When |
| --- | --- |
| `revoked` | The owner grant or the familiar revision was revoked, or the revision was superseded before dispatch. The run is stopped behind a stop fence; anything already done stays recorded. |
| `rejected` | Refused before any launch: validation, envelope, expiry, stale revision at submit, or a no-launch receipt. |
| `unavailable` | The daemon cannot construct the authority or runtime the mission needs (`AUTHORITY_ADAPTER_MISSING`, `AUTHORITY_TRUSTED_STATE_UNAVAILABLE`, `runtime_unavailable`). Not terminal while recovery is possible; terminal once the occurrence settles failed. |
| `cancelling` | A cancellation is `requested` or `stopping`, or `CANCEL_PENDING`. |
| `reconciling` | The occurrence is `recovering` or `recovery_required`, an attempt or run is `ambiguous`, or a cancellation is `recovery_required`. |
| `needs_approval` | Reserved (Decision 4). |
| `running` | The occurrence is `dispatching` or `running`. |
| `queued` | The occurrence is `planned`, `eligible` or `claimed`. |
| `completed` | The run is `succeeded`. |
| `cancelled` | The occurrence or run is `cancelled`. |
| `failed` | The run is `failed` or `timed_out` (reason `timeout`). |

Terminal states never change, and `revision` stops increasing at the terminal
state. A mission enters `reconciling` instead of reporting "nothing happened"
whenever execution may or may not have happened. It leaves `reconciling` only
through recovery evidence or `occurrence.recover.v1`.

## Terminal result

```ts
interface MissionResult {
  outcome: "completed" | "failed" | "cancelled" | "rejected" | "revoked" | "unavailable";
  settledAt: Timestamp;
  resultDigest?: Digest;                  // over the report artifact
  artifacts: Array<{
    kind: "report";                       // M0 produces only a report
    ref: string;                          // stable id, readable by the owner only
    mediaType: "text/markdown";
    bytes: number;                        // ≤ 256 KiB in M0
    digest: Digest;
  }>;
  usage: null | {
    source: "harness_result_event";
    inputTokens?: number; outputTokens?: number;
    cacheReadTokens?: number; cacheWriteTokens?: number;
    costUsd?: number;                     // only if the harness reports it
  };
  observation: "complete" | "partial" | "unknown";   // from the terminal observer (slice 5)
}
```

The report is the final assistant message of the session, bounded and stored as
an owner-only artifact. M0 produces no patch: `outputTarget` stays refused until a
writable envelope exists.

## Fail-closed rules

1. A principal comes only from the transport. A body field never grants
   authority. Over TCP, every mission command is refused before the store opens.
2. An unknown, revoked, superseded or not-yet-active familiar revision refuses
   the submit, or stops the dispatch. The daemon never substitutes the current
   revision.
3. An envelope the daemon does not advertise and enforce refuses the submit. An
   environment override (`COVEN_CLAUDE_BYPASS_PERMISSIONS`) never widens one.
4. `validUntil` passing before dispatch begins settles the mission `rejected`
   with `expired_before_dispatch`. It is never launched late.
5. A mission whose owner grant lapses during the run is stopped and settles
   `revoked`. Reads keep working.
6. Ambiguity is never terminal on its own: `reconciling` until recovery
   evidence settles it.
7. A read of a mission that belongs to another principal answers `NOT_FOUND`,
   so it does not reveal that the mission exists.
8. Limits are enforced at the daemon, not trusted from the client: prompt size,
   timeout, report size, and one active mission per familiar root per project in
   M0 (`CONCURRENCY_LIMIT`).

No new error codes: the v1 vocabulary is frozen, and it already covers every
mission refusal. A familiar revision that is not the current active one is
`REVISION_CONFLICT`, with the current revision in `details`. An envelope the
daemon does not advertise and enforce is `CAPABILITY_UNSUPPORTED`. A
`validUntil` already past at submit is `VALIDATION_FAILED`. Expiry after
submit is not an error: the mission settles `rejected` with reason
`expired_before_dispatch`.

## Vectors

Each negative case in chat#155 maps to at least one vector. Vectors live under
`conformance/automations/runner/` as `mission-*.vectors.json` suites, registered
in `inventory.json` like every other suite, in the existing format: given a store,
a clock and a transport, a command sequence produces exact responses and
`MissionView` states. In the table, "the key" means the mission's adoption key.

The `mission-commands` suite holds the vectors M0 can reach: M-01 to M-06,
M-09, M-11 (a), M-15, M-17 and M-18. M-03 (c) commits a second mission, because M0's first
has already settled. M-07, M-08, M-10, M-12, M-13 and M-14 need a mission that
launches, or one still queued, and wait for Runtime Authority (#857); unit
tests hold M-08 with a mission kept queued. M-11 (b) is a client vector, and
M-16 is held by the transport authority tests.

| # | chat#155 case | Given / when | Then |
| --- | --- | --- | --- |
| M-01 | disconnected before submit | The submit never reaches the daemon; the client asks `mission.get` for the id derived from its key | `NOT_FOUND`. The client may submit with the same key. |
| M-02 | disconnect after submit, before acknowledgement | Commit succeeds, the response is lost, and the client replays the same key and body | `replayed`, the same `missionId` and occurrence, exactly one launch |
| M-03 | duplicate submission | (a) same key, same body; (b) same key, changed body; (c) a new key with the same body | (a) `replayed`. (b) `ADOPTION_REPLAY_MISMATCH` (409), `conflictOutcome: committed`. (c) a second mission; under M0's limit, `CONCURRENCY_LIMIT` while the first is active. |
| M-04 | stale familiar revision | (a) the submitted revision is not the current active one; (b) it is superseded between submit and dispatch | (a) `REVISION_CONFLICT` with the current revision in `details`, nothing created. (b) `revoked` / `familiar_revision_stale`, no launch. |
| M-05 | rejected capability/runtime | The envelope id is unknown, or its descriptor digest differs from the advertised one | `CAPABILITY_UNSUPPORTED` naming the envelope, nothing created |
| M-06 | authority unavailable | Runtime Authority is not constructed, or the trusted state is unreadable at dispatch | `unavailable` with `authority_unavailable`, and no launch. A refusal at dispatch also writes a no-launch receipt; M0 refuses before dispatch, so it has none (see *Implementation notes*). |
| M-07 | cancellation racing completion | A cancel arrives after the harness's conclusive success but before settlement | `completed`; the cancellation outcome is `lost_to_completion` |
| M-08 | cancel before launch | A cancel arrives while `queued` | `cancelled`, no session, and a cancellation receipt |
| M-09 | idempotent cancel | The same cancel key twice, then a new key after the mission is terminal | The same receipt twice; the new key gets `ILLEGAL_TRANSITION` with the terminal view |
| M-10 | ambiguous timeout | The harness times out with its last write unobserved, or the daemon restarts mid-run | `reconciling` / `ambiguous_execution` until recovery settles it. Never `queued`, and never terminal without evidence. |
| M-11 | late response from another context | (a) `mission.get` from a different principal; (b) a response for mission A arriving after the client moved to familiar B | (a) `NOT_FOUND`. (b) The view carries `missionId`, the familiar `rootId`/`revisionId` and the principal; the client drops anything that doesn't match (client vector, run in the SDK and in Chat). |
| M-12 | revoked during observation | The familiar revision is revoked, or the owner grant lapses, while `running` | `cancelling`, then `revoked` / `familiar_revision_revoked` or `grant_revoked`; the session is stopped behind a fence |
| M-13 | expiry | `validUntil` passes while `queued` (for example, the dispatcher is busy) | `rejected` / `expired_before_dispatch`, no launch |
| M-14 | restart recovery | The client restarts during `running` and calls `mission.get` with the stored id | The same view and `revision`; the event cursor resumes or returns `CURSOR_EXPIRED` with a fresh view |
| M-15 | bounds | The prompt is over 32 KiB, the timeout over 60, or `validUntil` more than 15 minutes out | `PAYLOAD_TOO_LARGE` or `VALIDATION_FAILED`, nothing created |
| M-16 | transport | Any mission command over TCP | Refused before the store opens |
| M-17 | read scopes | A submit names project read scopes; the client reads the mission, replays the submit, then reuses the key with other scopes | Committed with the scopes in the view, and `mission.get` returns them on the `unavailable` settlement. The replay is `replayed`; the changed scopes are `ADOPTION_REPLAY_MISMATCH`. |
| M-18 | scope refusals | An empty or over-limit list, or a root-wide, absolute, `..`, glob, `workspace`-root, `write` or malformed scope | `VALIDATION_FAILED`, adopted, nothing created |

## Surfaces

| Surface | M0 shape |
| --- | --- |
| Daemon | `mission.*.v1` in the command envelope, plus flat `coven.automations.mission.*.v1` actions. Owner-local only. |
| CLI | `coven missions submit --json -` reads the payload on stdin and prints the response. It calls the running daemon over its owner socket, never the in-process action path, whose no-op runtime would record a launch that never happened. Also `coven missions get <id> --json`, `coven missions cancel <id> --json`, `coven missions watch <id> --json`, which streams `MissionView` lines until terminal, and `coven missions envelopes --json`, which prints the envelopes the daemon advertises. Chat can call these without moving its pinned `coven-client`. |
| SDK | A `CovenMissionsClient` with `submit`, `get`, `cancel` and `watch`, the projection types, and the client-side vectors (M-01, M-02, M-11, M-14). Designed in the SDK's companion design doc. |
| Chat | chat#155 builds typed native commands on the CLI or SDK surface. It stores the mission's adoption key and id, which needs the storage design its `SECURITY.md` calls for. |

### CLI output and exit status

The `coven missions` commands are hidden from `coven --help`, like `coven automations`,
because they are machine-facing. Each prints one JSON object per line on stdout:

- **`submit --json <path|->`** wraps the payload in a `mission.submit.v1` envelope. The
  envelope records the task statement as its intent and `cli` as its channel.
  `--adoption-key` sets the idempotency key; without it, a UUID is generated. The command
  prints the daemon's command response, `committed`, `replayed` or `rejected`, unchanged.
- **`cancel <id>`** does the same for `mission.cancel.v1`, with an optional `--reason`.
- **`get <id>`** prints the `MissionView`.
- **`watch <id>`** prints each view whose `revision` increased, waiting `--interval-ms`
  (default 1000) between reads, and returns after the first terminal view. A failed read
  ends the watch; running it again resumes from the current view.
- **`envelopes`** prints `{"envelopes":[{"id","descriptorDigest"}]}`, the list the running
  daemon advertises as `missionEnvelopes` in its capability catalog. A daemon from before
  the advertisement has none, and the command fails rather than guess one.

When the daemon refuses a `get` or `watch` read, `--json` prints
`{"missionId":…,"error":{"code":…}}` with the daemon's typed error, so a client can tell
`NOT_FOUND` from a daemon it never reached, which prints nothing on stdout.

Without `--json`, `get`, `cancel`, `watch` and `envelopes` print summary lines instead.

| Exit status | Meaning |
| --- | --- |
| 0 | `committed`, `replayed`, or a view was read |
| 1 | The daemon refused the command or read (with `--json`, its typed error is printed), or nothing was sent: a bad payload, no running daemon, or a failed connection |
| 3 | The request may have been applied and no answer proved it. The command prints `{"outcome":"unknown","command":…,"adoptionKey":…,"missionId":…}`; send it again with the same `--adoption-key` to learn the outcome |

The CLI never resends on its own and never decides a mission's state.

## Milestones

| Milestone | Delivers | Needs first |
| --- | --- | --- |
| **M0** | Local read-only diagnosis report: submit, observe, cancel, terminal report and usage. Vectors M-01 to M-16. | #857 constructs Runtime Authority for R0, or the #1308 scoped read service replaces `r0-read.1`. The familiar issuer gains its production caller. The run-control envelopes work (#1054, #1312). |
| **M1** | Test execution in the project, returning a report, and a patch artifact later | An exec envelope with enforced filesystem and process confinement |
| **M2** | Remote execution (Daytona) | A remote principal and per-attempt credential custody. coven-agents#262's reset-before-reuse finding. |
| **M3** | Paid admission | coven-agents#263 live evidence |

## Not in this contract

- Generic send, generic native request or execute bridges, and browser-direct HTTP.
- Merge, publish, deploy, familiar identity changes, and unbounded shell,
  filesystem or network access.
- Scheduling. A mission never recurs; a routine stays the thing for that.
- Billing and Daytona lifecycle (coven-agents).

## Implementation notes

Found while mapping the code on `origin/main` `753f553f`; they change how, not
what.

- **The internal definition must never be scheduled.** Every v1 definition
  needs an `rrule`, and the planner schedules every active row, while `runNow`
  requires an active one. A mission therefore gets its own claim path: its
  definition is stored `DISABLED`, so the planner, activation and `runNow` pass
  it by, and it carries no marker column. Its `automation_missions` row is what
  marks it: definition list and get, every definition command and `runNow`
  treat a definition with a mission row as absent (`NOT_FOUND`).
- **The owner grant is the mission row.** `automation_owner_grants` allows only
  create, activate and revise commands by CHECK constraint. Rather than widen
  that table with a migration, the immutable `automation_missions` row records
  the grant's evidence: the owner-local principal, the adoption key, the request
  digest and `submittedAt`. Triggers refuse any update or delete of it. Its
  nullable `scopes_json` column is added by an idempotent migration, never by
  the `CREATE TABLE`, so fresh and upgraded stores share one table form.
- **No launch until Runtime Authority exists.** Production dispatch is still the
  base v1 path, which launches with ambient authority. M0 refuses to launch any
  mission until the R0 envelope is enforced (#857). Once the submit has
  committed, the mission's occurrence settles failed with a reason the view
  projects to `unavailable` / `authority_unavailable` (vector M-06). The submit
  answers with the state at commit (`queued`); `mission.get` reports the
  settlement.
- **No receipt in M0.** A receipt binds a run and an attempt: the receipts
  table, `automation-receipt.schema.json` and the commit checks all require
  both. An M0 mission is refused before either exists, so it has no receipt.
  The settled occurrence, its transition event and the mission row are the
  record that nothing launched. Once missions dispatch, a refusal at dispatch
  writes the runner's existing no-launch receipt.
- **Validation order.** `validUntil` (in the future, at most 15 minutes ahead),
  then the envelope (`r0-read.1` and its descriptor digest), then `scopes`
  (see *Mission scopes*), then the project
  directory (exists, canonical, not inside `COVEN_HOME`), then the familiar
  (unknown root `NOT_FOUND`; a revision that is not the root's active head
  `REVISION_CONFLICT` with `details.currentRevisionId`), then concurrency.
  Every refusal is adopted under its key, as for `runNow`. The request digest
  covers `{command, payload}`, so the flat action and the command envelope are
  the same request, and `origin` / `intentId` never change it.
- **Events need no schema change.** Event payloads are closed schemas. The
  mission id already reaches every event as its `automationId`, and the state
  comes from `mission.get`, so the transition payload is unchanged. The SDK's
  `watch` already works this way: it yields a fresh view whenever `revision`
  rises.

## Also decided (2026-10-18)

1. **One-shot only.** `mission.submit` creates its own internal definition and
   never accepts an existing routine. Running a routine stays `occurrence.runNow`.
2. **Report retention follows session logs.** The owner-only report artifact is
   kept for the period the daemon already applies to session logs; there is no
   mission-specific `RetentionClass`.
3. **Concurrency per familiar root and project.** M0 allows one active mission per
   familiar root in each project (fail-closed rule 8).
