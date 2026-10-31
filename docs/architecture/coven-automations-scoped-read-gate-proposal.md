---
title: "Automations scoped read gate proposal"
summary: "Proposal for the production gate that admits and commits each scoped read under the daemon's revocation state, and for wiring r0-read.2 into the daemon launch."
read_when:
  - Implementing the production scoped read Gate
  - Wiring trusted_launch::prepare into the daemon launch path
  - Deciding when routine definitions may declare authority
source_adjacent_reason: "Proposes the gate and launch wiring left open by the scoped read service design for issue 1603."
---

# Automations scoped read gate proposal

Status: Approved as written and implemented (#1604). Val approved it on #857
(comment 6446181451), with two decisions: the `workspace` root stays refused
for v1, and the execution binding carries no scope digest.

Tracks: #1603 (draft PR #1604), #857. Builds on the
[scoped read service design](coven-automations-scoped-read-service.md) and the
[Runtime Authority decisions](coven-automations-runtime-authority.md). Line
references are to `crates/coven-cli/src/` at `7d396e7f`. This proposal changes
no code. It adds no certification evidence and does not advertise Runtime
Authority.

## What exists today

- `Gate` has two steps (`automations/scoped_read/service.rs:47-56`):
  - `admit()` runs before an operation;
  - `commit(tool, digest)` runs after the result is built and before any byte
    is written. A refused commit withholds the result and stops the service
    (`service.rs:226-262`).
- Only the test `Open` gate exists (`service.rs:459-472`). The channel wraps
  whatever gate it is given in `Closing`, which refuses with `attempt_closed`
  once the channel closes (`scoped_read/channel.rs:200-219`).
- `trusted_launch::prepare` (`automations/trusted_launch.rs:209`) checks the
  binding's signature, the projection, the project, and the decision's
  validity and consumption, through `launch_scopes`
  (`trusted_launch.rs:112-205`). It then calls scoped read admission
  (`scoped_read/admission.rs:155`).
  `Prepared::finish` returns the `Operation` list (`admission.rs:145`), but
  nothing persists it.
- The daemon launches an authority projection only through `for_projection`
  over `LAUNCHABLE` (`daemon.rs:1091-1121`,
  `automations/runtime_envelope.rs:196, 276-297`). `r0-read.2` is kept out by
  a guard test (`runtime_envelope.rs:354-361`). Definition validation refuses
  an `authority` block in production (`automations/definition.rs:467-503`).

**There is no single "revocation fence" in the code.** What can withdraw a
running attempt's authority lives in five places:

| Source | Where it is written |
| --- | --- |
| Role key revocation | `UPDATE automation_authority_keys SET revoked_at` (`automations/authority_keys.rs:352-370`) |
| Familiar revision revocation | `familiar_ledger_revisions.status = 'revoked'` (`familiar_ledger.rs:106-133, 933`) |
| Revision no longer current: revise, pause, disable or tombstone | `owner_grants::authorize_scheduled_dispatch` predicate (`automations/owner_grants.rs:230`) |
| Cancellation, timeout and recovery stop | `automation_stop_fences`, claimed in `BEGIN IMMEDIATE` (`automations/runner.rs:3135-3200`, `automations/cancellation.rs:100-107`) |
| Attempt or run settled | `automation_attempts.state`, terminal rows immutable (`automations/runs.rs:56-121`) |

All five are SQLite writes, and SQLite takes one writer at a time. That makes
the store's write lock the one serialization point the design asks for
("share the daemon's authority/fence serialization point").

## Recommendation

### 1. Admit and commit

The production gate, `StoreGate`, holds its own store connection and what
`prepare` already verified, fixed for the attempt: the attempt, run and
session ids; the pinned binding's digest; its `identityRevisionId` and
definition revision; and the ids of the `dispatch-authority`,
`threads-decision` and `owner-principal` keys that signed the binding and
decision.

**The predicate, P.** Both steps evaluate it:

1. **The attempt is live and owned.** The attempt, run, occurrence and session
   rows meet the same conditions `claim_stop_fence` requires
   (`runner.rs:3150-3170`):
   - attempt state `dispatching`, `started` or `observing`, with this
     session;
   - run status `running`;
   - session status `created`, `running` or `orphaned`.
2. **The binding is unchanged.** The attempt's `authority_extension_json`
   still has the digest verified at `prepare`.
3. **No stop is pending.** There is no `automation_stop_fences` row for the
   run, and no cancellation in `requested` or `stopping`. This is stricter
   than the stop fence alone: a requested cancellation already ends serving.
4. **No signing key is revoked.** Each of the held key ids has
   `revoked_at IS NULL`. This is the same fact that `threads_decisions::recorded`
   and `trusted_keys` act on (`threads_decisions.rs:527-545`,
   `authority_keys.rs:422-447`), read by primary key rather than by verifying
   the signatures again on every call.
5. **The familiar revision is not revoked.** Whether `retired` also stops
   reads is a maintainer choice; this proposal says yes. `superseded` does
   not, because the decision is bound to the revision it named.
6. **The definition revision is still current and active.** The predicate is
   `authorize_scheduled_dispatch`'s. A pause, disable, revise or tombstone
   therefore stops further reads.
7. **The run's `timeout_at` has not passed.** The service also checks its
   own deadline. `commit()` reads the time once it holds the write lock, so
   a deadline that passes while it waits for the lock withholds the result.

**`admit()`** evaluates P in a read transaction and records nothing. It is an
early refusal, not the authority.

**`commit()`** is the authority. In one `BEGIN IMMEDIATE` transaction, it
evaluates P again and inserts one commitment row. When the transaction
commits, it returns `Ok`. When P fails, it inserts a `withheld` row in the
same transaction and refuses with `attempt_closed`. That covers revocation
as well as a settled or stopping attempt, because the harness needs no more
detail than that, and the row's `refusal` keeps the same code.

A busy or unreadable store refuses (`attempt_closed`), and so fails closed.
The busy timeout is bounded by the operation's remaining time.

**The commitment table.** Its shape follows the immutable, retained event
tables in `threads_decisions.rs:64-85`:

```sql
CREATE TABLE IF NOT EXISTS automation_scoped_read_commitments (
    attempt_id TEXT NOT NULL REFERENCES automation_attempts (id),
    channel_id TEXT NOT NULL,          -- random per channel; a restart gets a new one
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    tool TEXT CHECK (tool IS NULL OR tool IN ('read_file', 'list_directory', 'search')),
    outcome TEXT NOT NULL CHECK (outcome IN ('served', 'refused', 'withheld')),
    refusal TEXT,                      -- a stable Refusal code, never free text
    result_digest TEXT CHECK (result_digest IS NULL OR length(result_digest) = 64),
    result_bytes INTEGER,
    committed_at TEXT NOT NULL,
    PRIMARY KEY (attempt_id, channel_id, sequence),
    CHECK ((outcome = 'withheld') = (result_digest IS NULL))
);
-- BEFORE UPDATE and BEFORE DELETE triggers: RAISE(ABORT, ...), as for Threads decisions.
```

**What is stored and what is not.**

- Stored: the SHA-256 of the exact JCS text the harness receives, which is
  already the `Operation` digest (`service.rs:80-85`), and its length.
- Never stored: file content, the requested path, the search pattern and
  entry names. `Response::to_json` carries the path and content
  (`scoped_read/open.rs:129-145`). A digest of a short, guessable result can
  confirm a guess, which is the same exposure the `Operation` record already
  has. Correlating a digest with a resource, through a request digest or a
  privacy-classified reference, belongs to the receipts step, which stays
  open.

**One trait change is needed.** `Gate::commit` receives only the tool and the
digest, so it cannot record `served` or `refused`, the refusal code or the
sequence. The proposal is to pass an `&Operation`-shaped value: sequence,
tool, outcome class and digest, with no content. This is a small change to
`service.rs:55` and its one caller.

### 2. Atomicity and crashes

Two orders are possible:

- **Revocation commits first.** The `commit` transaction sees P fail. A
  `withheld` row is written, and no byte of the result is written. Queued
  results are never built, because the service stops (`service.rs:243-244`).
- **Commitment commits first.** Those exact bytes are authorized, are
  written, and cannot be recalled. The next `admit` refuses. The write
  lock's order is the order; clock timestamps are only supporting evidence.

Crashes:

- **Before the commit transaction commits:** there is no row and no byte.
- **After it commits but before the write:** the row records that the bytes
  were *authorized*, not that they were *delivered*. Receipts must read the
  committed set as an upper bound on disclosure.
- **On restart:** the in-process listener and `Closing` are gone, so no old
  channel is adopted. The attempt follows the existing recovery semantics.
  The daemon should also sweep stale `<COVEN_HOME>/runtime/scoped-read/*`
  directories at startup, each of which holds a 0600 `mcp.json` with a token.
  No such sweep exists today.

At settlement, comparing `Prepared::finish`'s `Operation` list with the
committed rows is cheap. A mismatch makes the evidence incomplete, not
successful.

### 3. Launch wiring

- **Keep `r0-read.2` out of `LAUNCHABLE`, and leave `for_projection`
  unchanged.** BaseV1, which passes no projection, and `r0-read.1` missions
  keep their current path (`daemon.rs:1100-1121`).
- **Add a separate typed path for the scoped envelope.**
  `launch_session_with_envelope` takes
  `enum EnvelopeLaunch { Args(&'static RuntimeEnvelope), Scoped(Prepared) }`.
  Only `admission::prepare` constructs a `Prepared`, so `r0-read.2` can never
  start from its arguments alone. When `by_descriptor_digest` finds an
  envelope with `service.is_some()`, the daemon:
    1. opens its own store connection, following `daemon.rs:2806`;
    2. builds `ScopedLaunch` from the launch;
    3. calls `trusted_launch::prepare(conn, launch, Box::new(StoreGate), now)`.

  Any refusal fails before spawn, with the existing
  `RUNTIME_AUTHORITY_PROJECTION_UNSUPPORTED` handling.
- **The scoped path needs a new spawn builder.**
  - `build_envelope_harness_command` runs the harness in `launch.cwd`, the
    project, and inherits the daemon's environment with overrides
    (`pty_runner.rs:374-401, 296-318`).
  - `Prepared` requires its empty `cwd` and exactly its `environment`. The
    builder must clear the environment, then apply the containment receipt
    variable consistently with the five names the descriptor binds.
- **The `Prepared` value lives with the session.** The live session registry
  owns it, and `finish()` runs when the session exits, is killed or is
  stopped by shutdown. Dropping it closes the channel and removes the launch
  folder.
- **Refusals after consumption.** The decision is consumed before `prepare`
  runs, so a launch refused for an unadmitted harness version or a missing
  key burns that attempt's decision. The refusal must be recorded as
  `launch_refused` with its profile code. Retrying a Runtime Authority run is
  not implemented yet; when it is, it must decide afresh.
- **One gap in `launch_scopes`.** It does not check attempt state, stop
  fences, the familiar's status or the definition's currency
  (`trusted_launch.rs:112-205`). `prepare` should evaluate P once before
  admission.

### 4. Exit criteria for lifting definition opt-in

Remove the production refusal (`definition.rs:467-503`) and replace
`declaring_authority_is_refused_until_the_launch_gate_lands`
(`automations/trusted_authority.rs:1094`) only when every test below has been
seen failing first and then passing. Pause points use test hooks, like
`cancellation.rs:28-62`.

1. **Revocation first.** The reader returns, the test pauses before `commit`,
   then revokes the `threads-decision` key. The result is withheld, a
   `withheld` row is written, no bytes are written, and the service stops.
   Repeat with a familiar revocation, a definition pause and a stop fence
   claim.
2. **Commitment first.** Commit, then revoke. The bytes are delivered and the
   row exists. The next call is refused `attempt_closed`.
3. **A wrong attempt or session**, or a changed `authority_extension_json`, is
   refused at admit and at commit.
4. **A busy store** (another connection holding `BEGIN IMMEDIATE`) withholds
   the result. It is never served unrecorded.
5. **Immutability.** An update or delete on the commitments table aborts.
6. **Privacy.** With sentinel content and a sentinel path, no column of any
   row contains either string.
7. **Crash.** Abort after commit and before the write. The row is present.
   After restart, the channel directory is swept and the attempt is held for
   recovery.
8. **Daemon end to end.** An opt-in test drives `LiveSessionRuntime` with the
   stand-in claude from `trusted_launch::test_support`:
   - the granted note is served and the sibling is refused, both committed;
   - the cwd is empty and the environment is exactly the descriptor's names;
   - the channel and the launch folder are gone at session end.
9. **Launch separation.** An `r0-read.2` projection that reaches the args-only
   path is refused. The `LAUNCHABLE` guard test still passes.
10. **No regressions.** The BaseV1 and `r0-read.1` dispatch tests are
    unchanged, and `runtime_authority_stays_unadvertised` still passes.

Lifting opt-in does not advertise Runtime Authority or certify containment.
Those stay with #858 and #854.

## As implemented

The code follows the recommendation. Where the proposal left a detail open,
the implementation chose:

- `store_gate::authorized` is P. The launch, `admit` and `commit` evaluate
  it, and nothing else decides whether an attempt may still read. P also
  checks the attempt's session, so a gate held for one session refuses
  another.
- `trusted_launch::prepare` takes no gate. It always builds the `StoreGate`
  from what `launch_scopes` verified, with the envelope's operation time as
  its store wait and the run's deadline. Tests that pause the gate use a
  test-only `prepare_with_gate`.
- `Gate::commit` takes a `Committing` value: the sequence, the tool, the
  reader's refusal if any, the digest, and the result's length.
- `EnvelopeLaunch::Scoped` boxes its `Prepared`. `admission::prepare` is
  visible only inside the automations module.
- The harness gets exactly the envelope's five variables. As for every
  piped launch, the containment receipt path stays with the supervisor.
- The startup sweep removes only a channel's socket and configuration, then
  its directory.

Left for follow-up, because this proposal does not cover them:

- persisting `Prepared::finish`'s operation list, and comparing it with the
  committed rows at settlement;
- sweeping launch folders that a crash leaves in the system temporary
  folder. #1607 adds the startup reaper described in
  [the scoped read service](coven-automations-scoped-read-service.md).

## What this proposal does not decide

- The mapping for the `workspace` root. Only `project` is admitted.
- A scope digest in the execution binding. That is an authority-profile
  change; scopes stay bound through the signed request digest.
- The receipt schema for service operations, and any privacy-classified
  resource reference.
- The mission switch to `r0-read.2`, and Windows.
- Whether `retired`, as distinct from `revoked`, should stop a running
  attempt. The proposal says yes, for review.
- Whether a familiar declaration changed on disk mid-run should stop reads.
  `head_is_current` reads files (`familiar_ledger.rs:536`), outside the
  store's serialization, so the proposal excludes it.

## Alternatives considered

| Option | For | Against |
| --- | --- | --- |
| **A. Per-call `BEGIN IMMEDIATE` commit that rechecks targeted rows, with an immutable table (recommended)** | Serializes with every revocation source as they are written today. Evidence survives crashes. Reuses existing patterns. | One store write per call: at most 1,024 per attempt, each a short write-lock hold. |
| B. An in-memory epoch the daemon bumps on each revocation | No store writes per call | Every revocation path must remember to bump it, including writes that do not pass through daemon memory. A crash loses the evidence. |
| C. Record `Prepared::finish`'s operations once, at the end | Simplest | Not serialized with revocation, which the design requires. A crash loses everything. |
| D. Repeat full `launch_scopes` verification inside each commit | Strongest recheck | Ed25519 verification while holding the write lock stalls other daemon writers. The targeted key `revoked_at` read gives the same answer. |
| E. List `r0-read.2` in `LAUNCHABLE` and branch on `service.is_some()` | Smaller diff | A future caller of the args-only path could launch it without the service. The guard test would have to go. |
