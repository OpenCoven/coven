# Psyche O1.1 Corrective Delivery Annex

**Status:** Approved design; implementation evidence remains part of the O1
delivery candidate

**Decision date:** 2026-08-05

**Parent design:** [`specs/psyche/O1_CONTRACT_DESIGN.md`](../../../specs/psyche/O1_CONTRACT_DESIGN.md)

**Scope:** Corrective delivery invariants discovered while implementing O1.
This annex does not create a new named API contract, Psyche prerequisite, or
program workstream.

## 1. Purpose

O1 selected the canonical named-contract handshake and authoritative
session-lifecycle vocabulary required by Psyche. Implementation review exposed
runtime behavior that could violate those semantics even though it did not add
the later Psyche binding, adoption, fencing, or cancellation contracts.

In particular:

- direct CLI continuation could rewrite a terminal or archived source row;
- continuation could select a session owned by a different harness;
- native stream frames could mix Coven ledger identity with harness-native
  conversation identity;
- command-construction and malformed-stream failures could leave misleading
  ledger state;
- descendants could retain inherited pipes or survive after Coven returned an
  error; and
- signal and process-group cleanup could race PID reuse or lose cancellation.

O1.1 freezes the corrective invariants needed to deliver O1 truthfully. It is a
reviewable annex because these invariants widened the implementation boundary
after the original O1 design was approved.

## 2. Approved boundary

O1.1 covers:

1. immutable continuation-source evidence;
2. fresh sibling execution rows for direct CLI continuation;
3. project and harness matching during continuation selection;
4. stable conversation-key propagation without row reuse;
5. Coven-versus-harness stream identity separation;
6. fail-closed command and stream handling;
7. owned process-tree supervision, cancellation, and reaping; and
8. tests and documentation that make these guarantees durable.

O1.1 does not:

- add familiar snapshot, graph, node, attempt, or request-digest binding
  (`C-S3`);
- add stable adoption keys, idempotent adoption, lookup, non-adoption proof, or
  ambiguity fencing (`C-S4` through `C-S6`);
- change the ordered event-cursor contract (`C-S7`);
- claim acknowledged cancellation or satisfy `C-S9`;
- add result or artifact association (`C-S10`);
- claim restart persistence for later Psyche bindings (`C-S11`);
- define new authorization policy or universal denial behavior (`C-S12`);
- satisfy G4 or G6; or
- enable production Psyche child dispatch.

The named contract remains `coven.daemon.v1`. O1.1 introduces no route, wire
version, storage migration, or new persisted lifecycle value.

## 3. Continuation model

### 3.1 Source selection

Automatic direct CLI continuation selects the newest non-archived session that
matches both:

- the canonical project root; and
- the requested harness.

Explicit continuation may resolve a session by ledger ID or by its stable
conversation key, including an archived source. It must reject a source whose
harness differs from the requested harness.

Selection by source status remains permissive. A source may be `created`,
`running`, `idle`, `completed`, `failed`, `killed`, or `orphaned`; selecting it
does not mutate or reopen it.

### 3.2 Fresh sibling execution

Continuation creates a new, initially unarchived sibling row:

- with a new ledger ID;
- with `created` as its initial lifecycle status;
- with the same harness and bounded presentation metadata as the source; and
- grouped by the source `conversation_id` when present, otherwise by the
  source ledger ID.

The sibling alone transitions through the new execution lifecycle:

```text
created -> running -> completed | failed
```

Later existing lifecycle operations may produce other valid current-contract
states, but continuation does not rewrite the source row to represent the new
run.

### 3.3 Immutable source evidence

Continuation preserves the selected source row exactly, including:

- lifecycle status;
- exit code;
- archive overlay;
- creation and update timestamps;
- transcript and event references; and
- terminal or unresolved evidence.

This prevents a new run from erasing whether an earlier run completed, failed,
was killed, became orphaned, or was archived. It does not make the sibling a
Psyche adoption record and does not prove one-attempt/one-session binding.

## 4. Stream identity contract

Every top-level `session_id` emitted by `coven run --stream-json` identifies
the current Coven ledger row. On continuation, that is the fresh sibling ID.

When a native harness emits its own session or conversation identifier:

- Coven retains it separately as `harness_session_id` when the frame does not
  already provide that field;
- Coven rewrites only the top-level `session_id`;
- nested unrelated identifiers remain unchanged; and
- malformed JSON or a non-object native frame fails the run rather than being
  forwarded as valid Coven protocol.

The stable conversation key supplied to the harness is not promoted into the
new sibling's ledger identity. Ledger identity, conversation grouping, and
harness-native identity remain distinct concepts.

## 5. Process-supervision model

### 5.1 Ownership

Once Coven spawns a native or normalized stream harness, Coven owns the direct
child and its contained process tree until terminal cleanup. A child wrapper,
package-manager shim, or descendant may not outlive a failed or cancelled Coven
run merely because it inherited stdout, stderr, or another process handle.

Unix launches use a contained process group. Windows launches use the existing
strict Job Object containment. Platform-specific mechanics may differ. Coven
installs containment before resuming the child, and the owning supervisor
orders termination before reaping and returning from a failed or cancelled
run.

### 5.2 Supervision loop

Native stdout is read on a helper thread and delivered to the owning supervisor
as line messages. The supervisor:

1. validates and normalizes frames;
2. checks cancellation on a bounded polling interval;
3. observes direct-child exit without creating a PID-reuse window;
4. terminates remaining descendants before the Unix process-group leader is
   reaped;
5. drains queued output only for a bounded post-exit interval; and
6. reaps the direct child and preserves its original exit status.

On Unix, nonblocking exit observation uses `waitid` with
`WEXITED | WNOHANG | WNOWAIT`. A zero `si_pid` means no child has exited and
must not be interpreted as terminal state.

The loop must not wait indefinitely for a descendant-held pipe, busy-spin after
a reader disconnect, or signal a numeric process group after its leader has
been reaped.

### 5.3 Signal handling

The async signal handler records cancellation intent only. It never kills a
numeric PID or process group.

The owning supervisor observes the recorded intent and performs ordered
termination and reaping while it still owns the child tree. Helper threads
inherit a signal mask that prevents them from stealing supervised termination
signals. Teardown restores the previous signal dispositions without silently
discarding a pending signal.

This is runtime cleanup evidence, not the authoritative cancellation
acknowledgement required by `C-S9`.

## 6. Failure semantics

The current sibling row moves to `failed` when execution cannot be established
or supervised, including:

- command construction or argument validation failure after row creation;
- process spawn failure;
- missing required process pipes;
- malformed or unsupported native stream frames;
- stream read or normalization failure in the supervised native-stream path;
- forwarding or flush failure returned by that supervised path;
- cancellation observed by the supervisor;
- bounded pre-exit supervision timeout; and
- process-tree containment setup failure.

A failed pre-spawn or pre-execution path must not leave the sibling as
`running`. After spawning a contained child, an error returned by the
supervised native-stream path must first initiate ordered termination and
reaping. The current process-tree API does not expose a separately verified
cleanup result, so this annex does not claim that an operating-system
termination failure can itself be persisted as a ledger failure.

Captured-output compatibility paths outside that supervisor retain their
existing best-effort output emission behavior. This annex does not convert
those ignored output-emission errors into lifecycle transitions.
Likewise, in the native-stream supervisor described in section 5.2, expiration
of the bounded post-exit drain interval stops further draining but preserves
the direct child's exit status; it is not independently classified as a
lifecycle failure. This does not change the existing Codex protocol path,
where drain expiry is a protocol error and the run fails.

The source row is never rewritten to absorb sibling failure.

The OpenClaw lifecycle adapter continues to map persisted terminal evidence
explicitly:

| Coven status | Adapter stop reason |
|---|---|
| `completed` | `completed` |
| `failed` | `error` |
| `killed` | `cancelled` |
| `orphaned` | `error` |

`killed -> cancelled` is presentation-level classification only. It does not
claim acknowledged process termination. `orphaned` is unresolved ownership and
must never be presented as successful completion.

## 7. Error and diagnostic requirements

Continuation mismatch errors identify the requested and stored harness but do
not expose prompts, credentials, private paths, or unrelated session content.

Native protocol failures identify the harness and failure class. They may
include the existing bounded execution error but must not echo secret-bearing
environment state or raw private configuration.

All errors fail closed:

- no fallback to a different harness;
- no fallback to source-row reuse;
- no forwarding malformed native output as trusted JSONL;
- no inferred completion from pipe closure; and
- no local conversion of `orphaned` into success.

## 8. Verification design

The implementation evidence must include hermetic tests for:

### 8.1 Continuation

- automatic continuation filters by project root and harness;
- explicit continuation rejects harness mismatch;
- explicit continuation may select an archived source;
- every valid source status produces a fresh sibling;
- source status, exit code, archive overlay, and timestamps remain unchanged;
- sibling conversation grouping uses `conversation_id` or the source ledger ID;
- unknown sources fail without creating a sibling; and
- `--detach` and `--continue` remain mutually exclusive.

### 8.2 Stream identity and errors

- wrapper, native, assistant, output, and result frames use the sibling ledger
  ID;
- harness-native identity is retained separately;
- nested unrelated IDs are unchanged;
- malformed JSON and non-object frames fail;
- command-construction failure persists `failed`; and
- no failure path leaves a new row indefinitely `running`.

### 8.3 Process ownership

- malformed output terminates and reaps the direct child and descendants;
- cancellation is observed promptly and returns an error;
- a cancellation recorded before or immediately after spawn cannot escape
  supervision;
- a direct child that exits while a descendant holds stdout does not hang the
  runner;
- post-exit drain is bounded;
- a live child is not mistaken for exited by nonblocking Unix observation;
- process-group termination occurs before Unix reaping;
- no armed numeric PID remains after reaping;
- signal handlers and masks are restored; and
- Windows Job Object and Unix process-group paths preserve equivalent
  observable behavior.

### 8.4 Repository gates

The delivery candidate must pass:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test
python3 scripts/check-api-contract-docs-test.py
python3 scripts/check-api-contract-docs.py
python3 scripts/check-secrets-test.py
python3 scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
python3 scripts/check-coven-privacy.py --range origin/main...HEAD
git diff --check origin/main...HEAD
```

Timing-sensitive tests must prove a meaningful separation between the
descendant lifetime and the supervisor deadline rather than depending on a
narrow scheduler threshold.

## 9. Compatibility and rollout

O1.1 is additive at the behavioral level:

- no endpoint or response schema changes;
- no storage migration;
- no new lifecycle value;
- no change to the legacy `/api-version` response;
- no new Psyche permission or execution authority; and
- no production child-dispatch enablement.

The visible CLI correction is intentional: continuation now creates a new
ledger row rather than reopening the selected row, and cross-harness
continuation is rejected. Documentation and tests must land with the runtime
change.

Rollback is a normal PR revert only while no consumer depends on immutable
continuation evidence. Once released, restoring source-row mutation would be a
semantic regression and requires a separately reviewed compatibility decision.

## 10. Completion evidence

O1.1 is complete only when:

1. this corrective annex is approved and committed;
2. the implementation diff is reviewed against both O1 and O1.1;
3. every verification gate above passes against the exact candidate head;
4. the reviewed PR merges;
5. issue #567 and Bead `coven-psy-o1` record the observed merge SHA and
   verification evidence; and
6. the record explicitly states that C-S3 through C-S6, C-S9 through C-S12,
   G4, G6, and production Psyche child dispatch remain incomplete.

O1.1 strengthens the truthfulness and safety of O1 delivery. It does not expand
the set of Psyche prerequisites represented as satisfied.
