# coven-agents

The Rust `coven/crates/coven-agents` crate is an experimental, provider-neutral,
in-process behavior loop for OpenCoven. It is distinct from the
[OpenCoven/coven-agents cloud product](https://github.com/OpenCoven/coven-agents).
Model adapters, applications, and familiar runtimes can share its local
primitives:

- bounded model/tool loops
- journaled goal loops with explicit exit criteria
- agent handoffs (control transfer within one run, not child delegation)
- bounded child delegation with selected context and attenuated authority
- cooperative cancellation and deadlines on an injectable clock
- parent-to-child cancellation, deadlines and concurrency bounds for
  delegated children, with a recovery disposition when a child's effect is
  unknown
- blocking input and output guardrails
- fail-closed review of proposed tool calls before dispatch
- pluggable session journals
- persistent loop journals that can be rediscovered after process or machine restart
- explicit offline reconciliation before ambiguous work resumes
- metadata-only lifecycle observation
- validated logical agent references with optional immutable revisions
- versioned, serializable invocation lifecycle events
- durable, at-most-once invocation records that survive a restart and keep
  unknown outcomes explicit

## Policy and context boundaries

Input guardrails apply to the starting agent and every handoff target before
that agent's next model turn or tool execution, including when re-entering a
previous agent. Each checks the same **original user input**, not session
history or intermediate assistant/tool output. Output guardrails apply only
to the agent that produces the final output; they do not screen intermediate
contributions from other agents.

A handoff preserves the accumulated model transcript, including loaded session
history, and passes the same host-provided `&C` to models, tools, guardrails,
and proposal reviewers. The target uses its own instructions, tools, handoffs,
and reviewers. A handoff does not select a bounded child context, attenuate
host authority, or establish cross-principal isolation; `DelegationSpec`
below is the contract for that. Use handoff only within a host-established
trust boundary; target ingress parity is not a complete delegation policy.

Durable invocation ownership belongs to Psyche's orchestration contracts;
process execution, transport, and remote placement belong to Coven's existing
daemon/client/hub boundaries. The
[invocation/delegation migration](https://github.com/OpenCoven/coven/issues/804)
must preserve that split rather than turn this runner into a second distributed
runtime.

## Bounded delegation

`DelegationSpec` is the structured child-invocation contract: a validated
`AgentRef` target, a non-empty `TaskInput` of at most 64 KiB, a
`ContextManifest` of at most 64 unique `ContextRef`s (id, `sha256:` digest,
provenance; never payload), an `AuthorityScope` (project, tool names, opaque
resource names, and whether the child may delegate further), a
`DelegationBudget` of model turns and control transfers, and a
`ResultContract` bounding the final output. Every component validates at
construction and on the wire; the shape is closed and carries names and
digests only, so no credential has a place to be serialized.

The parent holds a `DelegationGrant`: its invocation, its `AgentRef`, and the
authority, context and budget it may pass down. A root grant is a trusted host
assertion, like `LoopRecoveryFence`. `PreparedDelegation::prepare` proves the
spec against the grant before anything is dispatched, and every check fails
closed with a distinct `AuthorityError`: the parent must hold `may_delegate`;
the project must match; requested tools and resources must be subsets;
`may_delegate` cannot be conferred by a parent without it; the budget cannot
exceed the parent's; and every context reference must be one the parent holds,
digest and provenance included. Nothing is narrowed on the requester's behalf:
asking for more than the parent holds is a refusal, not a silent reduction.
A successful proof binds a fresh child `InvocationContext` whose parent is the
grant's invocation and records `DelegationDigests` (spec, authority, context)
for traces. `child_grant` yields what the child may in turn pass down, so a
grandchild is bounded by the child's attenuated authority and budget.

`Runner::run_delegation` consumes a `PreparedDelegation` and runs the target
as its own invocation with `InvocationSource::Agent` naming the parent. The
child starts from the task input and the selected context only: there is no
session load, and the parent's transcript is not inherited. Each reference is
resolved through the host's `ContextResolver` and verified against its digest
before the first model turn; a resolver error or mismatch fails with
`RunFailureKind::Context`. Tools outside the attenuated authority are not
advertised to the model, and a call to one fails with
`RunFailureKind::Authority` before review or dispatch. Authority is bound to
the invocation, so a handoff inside the child stays within it. Output larger
than the result contract fails with `RunFailureKind::Limit` instead of
completing. The failure carries the child invocation and partial transcript
in a `DelegationFailure`.

Identity, capability, authority and trust stay distinct: an `AgentRef` names
an agent, registered tools are what it can do, `AuthorityScope` is what this
invocation may use, and the grant is what the host vouches for. Resource
names are proven as subsets at the boundary; the crate does not execute
resources, so tools and the host enforce `AuthorityScope::resources` against
the context they are given. Delegation here is local execution with
correlation: it does not adopt, persist, retry, or place the child anywhere.
A delegated child can be cancelled and bounded in time, as described under
[delegated children](#delegated-children). Legacy `Handoff` remains available
as in-run control transfer during migration.

## Deadlines and cancellation

`Runner::run_invocation_controlled` runs a request under `RunControls`: an
optional `CancellationToken` and an optional timeout measured on the runner's
`Clock` (`SystemClock` by default; `ManualClock` for deterministic tests;
replace it with `Runner::with_clock`). Cancelling a token cancels its child
tokens, never its parent. The default controls never stop a run, and the
legacy entry points use them.

The runner never preempts a future. It observes a stop at checkpoints: before
each model turn, after each model response, and before each tool dispatch.
Tools and models can watch the same `RunSignal` through
`Tool::execute_with_signal` and `Model::generate_with_signal`. Both default
to `execute` and `generate`, so existing implementations are unchanged. The
signal reports `stop_reason()` (`cancelled` ahead of `deadline_exceeded`) and
`remaining()` time.

Outcomes are distinct and never retried:

- **Cancellation requested.** One `CancellationRequested` event, in both
  streams, the first time the runner observes the stop. It is always
  followed immediately by the terminal failure below.
- **Cancelled.** The stop was acknowledged with no tool in flight. A model
  response that arrives after the stop is discarded and none of its actions
  run. Tool calls that already returned keep their results in the partial
  transcript. Cancellation does not mean nothing happened.
- **Termination unknown.** A tool was in flight when the stop arrived and
  then failed. Its effect may or may not have happened, so the run fails as
  `TerminationUnknown` with the call id. A tool turns this into a clean
  cancellation only by returning `NotStarted`. That mirrors Coven's rule
  that `not_started` is asserted explicitly and never inferred from errors,
  timeouts, cancellation or disconnection. `NotStarted` without a stop is an
  ordinary tool failure.

A tool that returns successfully during a stop is a known effect: its result
is recorded and the run is acknowledged as cancelled at the next checkpoint.
Once a final output passes policy, the run completes even if a stop arrives
afterwards. These are in-memory outcomes of one local run. They are not
persisted states, adoption decisions, or proof that an external process
stopped.

### Delegated children

`Runner::run_delegation_controlled` runs a prepared delegation under controls
derived from its parent, usually `RunSignal::child_controls()`. Cancelling the
parent's token stops the child; cancelling the child never stops the parent.
The child's deadline is the earlier of the parent's remaining time and the
spec's own `DelegationBudget::with_timeout`. When the parent's budget has a
timeout, `PreparedDelegation::prepare` refuses a child with none or a longer
one (`DeadlineWidened`). A host that orchestrates delegation outside any run
can start a `RunSignal::new` to derive child controls from.

`Runner::with_child_limit(n)` lets at most `n` delegated children of one
parent invocation run at once on that runner. A child beyond the limit is
refused with `ChildLimitExceeded` before its target runs, as a `Limit`
failure with the usual paired lifecycle events. A slot is released when the
child finishes or its future is dropped. There is no limit by default.

`DelegationFailure::disposition()` tells the parent what a failed child means:

- **Failed.** The child failed or was refused, and nothing about its outcome
  is ambiguous beyond its partial transcript.
- **Cancelled.** The child acknowledged a stop with no tool in flight.
- **Recovery required.** The child, or one of its descendants, left an effect
  whose outcome is unknown (`TerminationUnknown` or `RecoveryRequired`).

A tool that returns such a child failure fails its own run as
`RecoveryRequired`, with the call id, whether or not a stop arrived. It
counts even when wrapped in another error, as long as the wrapper reports
its source. That outcome outranks a clean cancellation, and the work must be
reconciled before any retry.

## Tool proposal review

`ProposalReview<C>` sits beside the guardrail traits. Reviewers are registered
per agent with `Agent::with_proposal_review` and see each resolved tool call
(name and arguments) before it runs, returning `Permit`, `ProposalOnly`,
`Reject`, or `Unavailable`. Reviewers run in registration order and the first
non-permit verdict wins. The seam fails closed: a reviewer that returns an error
fails the run with `RunFailureKind::ProposalReview`; there is no default
permit. A non-permitted call does not fail the run. The runner records the
`RunItem::ToolCall`, skips execution, and appends a `RunItem::ToolResult` of
`{"executed": false, "review": {"reviewer", "verdict", "reason"}}` so the model
sees the decision as an ordinary tool result. Observers receive one
`RunEvent::ProposalReviewed` per reviewer consulted, carrying the payload-free
`ReviewOutcome` and never the reason. A handoff target's reviewers gate its own
tool calls. Verdicts are evidence, not authorization: a permit records that a
named reviewer raised no objection, it grants no capability and is not an
operator approval. The crate ships the seam and fake reviewers only; a live
reviewer is an adapter that implements the trait outside this crate.

Review is per call, not transactional across a model response: an earlier
permitted call can execute before a later reviewer errors. An errored proposal
has a review event but is not appended as a `RunItem::ToolCall`. Ordinary
non-permit verdicts retain their call/result pair and allow later calls to be
reviewed. Input rejection prevents target model execution and proposal review.

## Persistence and invocation boundaries

A `SessionStore` must serialize writers for each session id or implement
optimistic concurrency control.

A failed run returns `RunFailure`, which carries the transcript produced before
the failure alongside the error, so a failing tool never costs the caller the
items the run already produced. Execution or policy failure prevents session
append; persisting those partial items is the caller's decision. After final
output passes policy, the runner attempts append before reporting success,
so an append error returns a failure even if the store already wrote items.
A tool can succeed before a later model, guardrail, reviewer, or session append
fails. Neither a failed run nor missing
session output proves that no side effect occurred; the transcript is not a
per-effect ledger and does not authorize automatic retry.

Each `Runner::run` call creates a fresh root `InvocationContext`. The same
context is carried by every `RunEvent`, the successful `RunResult`, and any
partial `RunFailure`. Use `RunEvent::invocation()` to read it without matching
individual event variants. `Runner::run_with_invocation` accepts an explicit
context, including an optional parent invocation ID for caller-managed nested
work. These IDs provide correlation only: they do not authorize execution or
promise durable adoption, idempotency, deduplication, or safe retries.
`DurableInvocationRunner` adds at-most-once execution per id (see
[durable invocations](#durable-invocations)).

`AgentRef` pairs a validated logical agent ID with an optional immutable
`AgentRevision`. Agent IDs and revisions are non-empty, at most 127 bytes, and
contain no whitespace or control characters. `Runner::new` rejects agents whose
legacy `AgentId` cannot form a valid reference. Callers can register a revision
with `Agent::with_revision`; an explicit `InvocationRequest` must then name that
exact revision or the run fails before model execution. The compatibility
`Runner::run` and `Runner::run_with_invocation` entry points resolve the
currently registered revision automatically.

`InvocationObserver` receives a separate canonical metadata stream without changing the existing `RunObserver` event shapes. Every admitted request emits one versioned `started` event and one terminal `completed` or `failed` event. Legacy handoffs inside the same run are emitted as `control_transferred`, not as durable delegation. A run that observes a stop emits one `cancellation_requested` event immediately before its terminal `failed`. Every event carries the original requested target and source; failures separately identify `failed_at`, so a target-resolution failure is not mislabeled as execution by that target. The `coven.agent-invocation-event.v1` wire schema rejects unknown fields. Events for prepared delegated children also carry `delegation` with the spec, authority and selected-context digests. The same digests accompany start, cancellation, control transfer and terminal events, including refusals before the target runs. They are correlation evidence, not portable authority or context payloads. Direct and legacy events omit the optional field; old events still decode. Older readers that reject unknown fields reject delegated events with this new metadata instead of silently losing it. Attempt and executor bindings remain absent until Coven has an executor contract that can supply them truthfully.

This pre-1.0 API adds an `invocation` field to event and outcome shapes.
`RunFailure::invocation` is boxed, and `RunFailure::new_items` is an owned boxed
slice; use `.as_ref()` for a slice or `.into_vec()` for a growable transcript.
Successful results retain `Vec<RunItem>`.

Tool call ids must be unique across a run, including call and result ids loaded
from session history. Results correlate to calls by id, so the runner rejects a
response that reuses one before running any tool in that response, with
`RunError::DuplicateToolCallId`.

`GoalLoopRunner` composes the single-run `Runner` into a bounded
`loop-until-done` primitive. An injected `LoopEvaluator` decides whether each
result satisfies the goal or supplies the next input, while an injected
`LoopJournal` persists every iteration boundary. A pending checkpoint resumes
from its saved input. A running checkpoint fails closed with
`LoopError::AmbiguousInFlight` instead of automatically replaying work that may
already have produced external side effects. Journal implementations must
implement `compare_and_set` atomically so only one caller can claim a pending
iteration.

`InMemoryLoopJournal` exists for tests and ephemeral callers. Durable
applications should adapt `LoopJournal` to their authoritative store; the crate
does not own SQLite, daemon scheduling, GitHub labels, or UI state.

The crate deliberately does not include an OpenAI client, a daemon command,
MCP, sandbox execution, voice, realtime transport, or an HTTP-backed proposal
reviewer. Those are adapters and
application concerns. Keeping this crate as a workspace leaf also allows it to
move into its own repository if the API stabilizes.

The implementation was derived from public behavioral documentation and
OpenCoven's existing runtime requirements, not from another SDK's source code.
See the design document under `docs/superpowers/specs/`.

`FileLoopJournal` stores immutable, atomically published checkpoint generations
behind per-loop file locks. A restarted daemon can call `LoopJournal::list` to
discover pending and in-flight work, reconstruct its runners, and resume only
safe checkpoints. Applications that need to compare remote state after downtime
attach a `LoopReconciler`, prove the previous executor can no longer act, and
call `GoalLoopRunner::reconcile` with a `LoopRecoveryFence`. Reconciliation can
complete an already-satisfied goal, supply a safe resume input, confirm an
in-flight iteration, or block for operator input. Ordinary `run` calls never
reclaim running work. Each runner receives a process-lifetime instance id;
running checkpoints retain that id and an attempt id so the live owner also
cannot revoke its own claim under the guise of recovery. Blocked decisions are
checkpointed with their reason and require an explicit journal transition
before execution.


`LoopRecoveryFence` is a trusted host assertion, not independently verified
executor termination. The host/reconciler must establish that the previous
executor cannot act before supplying it; a nonempty evidence string alone does
not establish distributed fencing.

## Durable invocations

`DurableInvocationRunner` runs an `InvocationRequest` against an
`InvocationJournal`, so each invocation id executes at most once and its
outcome survives a restart. `FileInvocationJournal` stores records with the
same crash-safe checkpoints as `FileLoopJournal`. `InMemoryInvocationJournal`
is for tests.

The journal is separate from any `SessionStore`. An `InvocationRecord` holds
the invocation's identity, source and target, a digest of the exact request,
its state and the attempt that adopted it. Content appears only as digests;
no prompt, transcript or output is stored.

Every run takes the same steps:

1. **Reserve.** The request and its digest are written as `reserved` before
   anything runs. `invocation_request_digest` covers the identity, source,
   target, input and run options.
2. **Adopt.** One attempt, bound to the runner's process instance id, writes
   `running` before the first model turn. Effects may happen from here on.
3. **Run** under the caller's `RunControls`.
4. **Settle** into exactly one state.

The state names follow Psyche's attempt vocabulary:

| State | Meaning | A duplicate submission |
| --- | --- | --- |
| `reserved` | Recorded and never adopted, so proven not adopted. | Dispatches it, once. |
| `running` | Adopted by an attempt. | `ActiveInFlight` while that attempt runs in this process; otherwise `RecoveryRequired`. |
| `completed` | The final output passed policy. Only its digest is kept. | Replayed from the journal. |
| `rejected` | Failed before the first model turn, so nothing ran. | Replayed. |
| `failed` | Failed with a known kind after execution began. | Replayed. |
| `cancelled` | A stop was acknowledged with no tool in flight. | Replayed. |
| `termination_unknown` | A stop arrived while a tool ran, and the tool did not prove it did nothing. | `RecoveryRequired`. |
| `recovery_required` | A tool reported a descendant whose outcome is unknown. | `RecoveryRequired`. |

The same invocation id with a different request, input or options is a
`Conflict`, never a replay. Nothing is retried: to try again after a settled
failure, use a new invocation id.

Unknown stays unknown across a restart:

- `adoption_unknown` is never written. If the adoption write fails, the
  runner returns `AdoptionUnknown` and does not run. A later read finds the
  record either still `reserved`, which is proven not adopted and safe to
  dispatch, or `running`, which requires recovery.
- A `running` record whose attempt is not executing in this process requires
  recovery in every process. That covers a restart, a dropped run future and
  a failed settlement write. `SettlementFailed` still hands the run's outcome
  to the caller.
- As with `GoalLoopRunner`, `process_instance_id` must change on every
  process start.

There is no reconciliation path yet. A record that requires recovery stays that way until Psyche's existing fencing contract has an authoritative consumer binding for these process-bound attempts (see [OpenCoven/psyche#19](https://github.com/OpenCoven/psyche/issues/19)). An opaque fence token alone does not prove that a prior attempt can no longer act.

`DurableInvocationRunner::run_delegation` consumes a `PreparedDelegation` and records its child/parent identity and delegation, authority and context digests before context resolution or model execution. Its request digest includes the delegation contract, so direct submission cannot reuse the child identity to bypass attenuation. The ordinary delegation runner still enforces context, authority, result bounds and inherited cancellation/deadline controls. The child deadline includes time spent in journal I/O before dispatch. Journal errors with unresolved adoption or settlement propagate through tool error chains as parent `recovery_required` outcomes.

Prepared delegations remain single-use. This API adds no retry, reconstruction of a consumed proof, parent scheduling or reconciliation. Hosts can discover children through `InvocationJournal::list` and each record's `parent_id`. Old records without delegation metadata continue to decode; delegated records carry only digests, never task text or resolved context.

## Supported checkpoint and deferred orchestration

The existing `Runner` provides local control transfer, correlation, and bounded local delegation with selected context and attenuated authority. `GoalLoopRunner` journals iteration boundaries; it does not supply durable child invocation adoption or shared executor conformance. `DurableInvocationRunner` records root and delegated child invocations durably. Several things remain deferred under #804: fenced reconciliation of an invocation whose outcome is unknown, remote executors, unified observability, and removal of legacy handoff/full-transcript behavior.

Internally, `Runner` is a facade over a crate-private `AgentLoop`. The loop
runs one agent's model and tool turns: input and output guardrails, the
result contract, proposal review, authority filtering, and tool dispatch. It
stops at a final output, a control-transfer request, or the shared turn
budget. The runner owns everything around it: request and revision
validation, session load and append, delegated context resolution,
control-transfer counting and target lookup, and the lifecycle events that
start and end an invocation. Legacy handoff is visible as the runner passing
one transcript from agent to agent. `AgentLoop` is not public API; it is the
intended core of a future in-process executor.

## Behavioral evidence

[`tests/runner.rs`](tests/runner.rs) covers direct/handoff ingress parity,
multi-hop rejection before target execution, tool-call correlation, limits,
session behavior, proposal refusal, and terminal event pairing.
[`tests/invocation_contract.rs`](tests/invocation_contract.rs) covers validated
references, revision binding, and canonical invocation metadata.
[`tests/legacy_boundary.rs`](tests/legacy_boundary.rs) characterizes inherited
history, original-input-only checks, final-only output policy, shared host
context, and a successful effect followed by session-append failure.
[`tests/delegation.rs`](tests/delegation.rs) covers the closed delegation
wire shape, fail-closed attenuation (cross-project, tool, resource,
re-delegation, budget, context leak), bounded child context without parent
transcript inheritance, authority enforcement across control transfer, digest
verification before model execution, and result-contract and budget limits.
[`tests/lifecycle.rs`](tests/lifecycle.rs) covers cancellation before start,
deadlines on an injected clock, stops during model calls and tool batches,
termination-unknown versus `NotStarted`, signal observation by tools, the
closed stop vocabulary, and unchanged legacy entry points. Interleavings are
driven from inside scripted models and tools; nothing sleeps.
[`tests/loop_runner.rs`](tests/loop_runner.rs) covers local checkpoint and
reconciliation behavior. These are in-process tests with scripted models and
tools, not real-daemon, provider-containment, packaged-client, or external A2A
conformance.
