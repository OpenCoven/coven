---
title: "Automations missions"
summary: "Approved contract (not implemented) for submitting, observing and cancelling one bounded familiar mission over coven.automations.v1."
read_when:
  - Designing or reviewing the mission commands for issue 1313
  - Building a client (SDK, CLI, Chat) that submits or observes a mission
source_adjacent_reason: "Defines the producer contract that OpenCoven/chat#155 and the SDK binding are built against."
---

# Automations missions

Status: **contract shape approved by Val on 2026-10-18; not implemented**. Written for #1313,
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
| `envelope` | `{ id: "r0-read.1", descriptorDigest }` | At submit: the daemon must advertise and enforce it |
| `project.cwd` | An absolute, canonical directory | At submit: it exists, is a directory, and is not inside `COVEN_HOME` |
| `task.statement` | A short title for the mission | 1–200 characters |
| `task.prompt` | What the familiar is asked to do | 1 byte to 32 KiB |
| `limits.timeoutMinutes` | Hard run timeout | 1–60 in M0 |
| `validUntil` | Dispatch must **begin** before this instant | After `requestedAt`, at most 15 minutes later |

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
}

interface MissionSubmitResult {
  missionId: string;      // "mission-" + first 32 hex of sha256(adoptionKey); no ":" (id pattern)
  occurrenceId: string;   // the manual occurrence the mission projects
  mission: MissionView;   // state at commit, "queued" on first commit
}

interface MissionGetPayload { missionId: string }

interface MissionCancelPayload { missionId: string; reason?: string }   // reason ≤ 200 chars

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
  the same adoption key returns the same cancellation.
- **Events**: clients use `events.read.v1` / `events.subscribe.v1` on the
  mission's occurrence stream. Every event that changes the projected state also
  carries `missionId` and the new `mission.state`.

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

New error codes: `FAMILIAR_REVISION_STALE`, `MISSION_EXPIRED`,
`ENVELOPE_UNSUPPORTED`. Every other refusal reuses the existing `ErrorCode`
set.

## Vectors

Each negative case in chat#155 maps to at least one vector. Vectors live under
`conformance/automations/runner/` as `mission-*.vectors.json` suites, registered
in `inventory.json` like every other suite, in the existing format: given a store,
a clock and a transport, a command sequence produces exact responses and
`MissionView` states. In the table, "the key" means the mission's adoption key.

| # | chat#155 case | Given / when | Then |
| --- | --- | --- | --- |
| M-01 | disconnected before submit | The submit never reaches the daemon; the client asks `mission.get` for the id derived from its key | `NOT_FOUND`. The client may submit with the same key. |
| M-02 | disconnect after submit, before acknowledgement | Commit succeeds, the response is lost, and the client replays the same key and body | `replayed`, the same `missionId` and occurrence, exactly one launch |
| M-03 | duplicate submission | (a) same key, same body; (b) same key, changed body; (c) a new key with the same body | (a) `replayed`. (b) `ADOPTION_REPLAY_MISMATCH` (409), `conflictOutcome: committed`. (c) a second mission; under M0's limit, `CONCURRENCY_LIMIT` while the first is active. |
| M-04 | stale familiar revision | (a) the submitted revision is not the current active one; (b) it is superseded between submit and dispatch | (a) `FAMILIAR_REVISION_STALE`, nothing created. (b) `revoked` / `familiar_revision_stale`, no launch. |
| M-05 | rejected capability/runtime | The envelope id is unknown, or its descriptor digest differs from the advertised one | `ENVELOPE_UNSUPPORTED`, nothing created |
| M-06 | authority unavailable | Runtime Authority is not constructed, or the trusted state is unreadable at dispatch | `unavailable` with `authority_unavailable`, no launch, and a no-launch receipt |
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

## Surfaces

| Surface | M0 shape |
| --- | --- |
| Daemon | `mission.*.v1` in the command envelope, plus flat `coven.automations.mission.*.v1` actions. Owner-local only. |
| CLI | `coven missions submit --json -` reads the payload on stdin and prints the response. It calls the running daemon over its owner socket, never the in-process action path, whose no-op runtime would record a launch that never happened. Also `coven missions get <id> --json`, `coven missions cancel <id> --json`, and `coven missions watch <id> --json`, which streams `MissionView` lines until terminal. Chat can call these without moving its pinned `coven-client`. |
| SDK | A `CovenMissionsClient` with `submit`, `get`, `cancel` and `watch`, the projection types, and the client-side vectors (M-01, M-02, M-11, M-14). Designed in the SDK's companion design doc. |
| Chat | chat#155 builds typed native commands on the CLI or SDK surface. It stores the mission's adoption key and id, which needs the storage design its `SECURITY.md` calls for. |

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
  requires an active one. A mission therefore gets its own claim path: the
  definition is stored with a mission marker the planner skips, and is never
  active in the scheduling sense.
- **The owner grant needs a migration.** `automation_owner_grants` allows only
  create, activate and revise commands by CHECK constraint, so `mission.submit`
  is added to it, with release-upgrade fixture coverage.
- **No launch until Runtime Authority exists.** Production dispatch is still the
  base v1 path, which launches with ambient authority. M0 refuses to launch any
  mission until the R0 envelope is enforced (#857), and settles it
  `unavailable` with a no-launch receipt (vector M-06).
- **Events.** Event payloads are closed schemas. `missionId` and the projected
  state are added as optional fields of the transition payload, a minor-profile
  change while the profile is `proposed`.

## Also decided (2026-10-18)

1. **One-shot only.** `mission.submit` creates its own internal definition and
   never accepts an existing routine. Running a routine stays `occurrence.runNow`.
2. **Report retention follows session logs.** The owner-only report artifact is
   kept for the period the daemon already applies to session logs; there is no
   mission-specific `RetentionClass`.
3. **Concurrency per familiar root and project.** M0 allows one active mission per
   familiar root in each project (fail-closed rule 8).
