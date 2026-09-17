# coven-agents

The Rust `coven/crates/coven-agents` crate is an experimental, provider-neutral,
in-process behavior loop for OpenCoven. It is distinct from the
[OpenCoven/coven-agents cloud product](https://github.com/OpenCoven/coven-agents).
Model adapters, applications, and familiar runtimes can share its local
primitives:

- bounded model/tool loops
- journaled goal loops with explicit exit criteria
- agent handoffs (control transfer within one run, not child delegation)
- blocking input and output guardrails
- pluggable session journals
- persistent loop journals that can be rediscovered after process or machine restart
- explicit offline reconciliation before ambiguous work resumes
- metadata-only lifecycle observation

## Policy and context boundaries

Input guardrails apply to the starting agent and every handoff target before
that agent's first model turn or tool execution. Each checks the same
**original user input**, not session history or intermediate assistant/tool
output. Output guardrails apply only to the agent that produces the final
output; they do not screen intermediate contributions from other agents.

A handoff preserves the accumulated model transcript, including loaded session
history, and passes the same host-provided `&C` to models, tools, and guardrails.
The target uses its own configured tools. The runner does not select a bounded
child context, attenuate host authority, or establish cross-principal isolation.
Use this compatibility behavior only within a host-established trust boundary;
target ingress parity is not a complete delegation policy.

Durable invocation ownership belongs to Psyche's orchestration contracts;
process execution, transport, and remote placement belong to Coven's existing
daemon/client/hub boundaries. The
[invocation/delegation migration](https://github.com/OpenCoven/coven/issues/804)
must preserve that split rather than turn this runner into a second distributed
runtime.

## Persistence and recovery boundaries

A `SessionStore` must serialize writers for each session id or implement
optimistic concurrency control.

A failed run returns `RunFailure`, which carries the transcript produced before
the failure alongside the error, so a failing tool never costs the caller the
items the run already produced. The runner does not append a failed run's items
to the session; persisting them is the caller's decision. A tool can succeed
before a later model, guardrail, or session append fails. Neither a failed run
nor missing session output proves that no side effect occurred; the transcript
is not a per-effect ledger and does not authorize automatic retry.

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
MCP, sandbox execution, voice, or realtime transport. Those are adapters and
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

## Behavioral evidence

[`tests/runner.rs`](tests/runner.rs) covers direct/handoff ingress parity,
multi-hop rejection before target execution, tool-call correlation, limits,
session behavior, and terminal event pairing.
[`tests/legacy_boundary.rs`](tests/legacy_boundary.rs) characterizes inherited
history, root-input-only checks, final-only output policy, shared host context,
and a successful tool effect followed by session-append failure.
[`tests/loop_runner.rs`](tests/loop_runner.rs) covers local checkpoint and
reconciliation behavior. These are in-process tests with scripted models and
tools, not real-daemon, provider-containment, packaged-client, or external A2A
conformance receipts.
