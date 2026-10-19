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
- blocking input and output guardrails
- fail-closed review of proposed tool calls before dispatch
- pluggable session journals
- persistent loop journals that can be rediscovered after process or machine restart
- explicit offline reconciliation before ambiguous work resumes
- metadata-only lifecycle observation
- validated logical agent references with optional immutable revisions
- versioned, serializable invocation lifecycle events

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
correlation: it does not adopt, persist, retry, cancel, time out, or place the
child anywhere. Legacy `Handoff` remains available as in-run control transfer
during migration.

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

`AgentRef` pairs a validated logical agent ID with an optional immutable
`AgentRevision`. Agent IDs and revisions are non-empty, at most 127 bytes, and
contain no whitespace or control characters. `Runner::new` rejects agents whose
legacy `AgentId` cannot form a valid reference. Callers can register a revision
with `Agent::with_revision`; an explicit `InvocationRequest` must then name that
exact revision or the run fails before model execution. The compatibility
`Runner::run` and `Runner::run_with_invocation` entry points resolve the
currently registered revision automatically.

`InvocationObserver` receives a separate canonical metadata stream without
changing the existing `RunObserver` event shapes. Every admitted request emits
one versioned `started` event and one terminal `completed` or `failed` event.
Legacy handoffs inside the same run are emitted as `control_transferred`, not as
durable delegation. Every event carries the original requested target and
source; failures separately identify `failed_at`, so a target-resolution
failure is not mislabeled as execution by that target. The
`coven.agent-invocation-event.v1` wire schema rejects unknown fields. Attempt
and executor bindings remain absent until Coven has an executor contract that
can supply them truthfully.

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

## Supported checkpoint and deferred orchestration

The existing `Runner` provides local control transfer, correlation, and
bounded local delegation with selected context and attenuated authority.
`GoalLoopRunner` journals iteration boundaries; it does not supply durable
child invocation adoption or shared executor conformance. `AgentLoop`
extraction, wall-clock deadlines and cancellation, durable invocation
adoption/recovery, remote executors, unified observability, and removal of
legacy handoff/full-transcript behavior remain deferred under #804.

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
[`tests/loop_runner.rs`](tests/loop_runner.rs) covers local checkpoint and
reconciliation behavior. These are in-process tests with scripted models and
tools, not real-daemon, provider-containment, packaged-client, or external A2A
conformance.
