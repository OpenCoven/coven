# coven-agents

`coven-agents` is an experimental, provider-neutral Rust run loop for
OpenCoven. It supplies the orchestration primitives that model adapters,
applications, and familiar runtimes can share:

- bounded model/tool loops
- journaled goal loops with explicit exit criteria
- agent handoffs
- blocking input and output guardrails
- pluggable session journals
- persistent loop journals that can be rediscovered after process or machine restart
- explicit offline reconciliation before ambiguous work resumes
- metadata-only lifecycle observation

Input guardrails apply to the starting agent, and output guardrails apply to the
agent that produces the final output. A `SessionStore` must serialize writers
for each session id or implement optimistic concurrency control.

A failed run returns `RunFailure`, which carries the transcript produced before
the failure alongside the error, so a failing tool never costs the caller the
items the run already produced. The runner does not append a failed run's items
to the session; persisting them is the caller's decision.

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
attach a `LoopReconciler`; it can complete an already-satisfied goal, supply a
safe resume input, confirm an in-flight iteration, or block for operator input.
Each runner receives a process-lifetime instance id. Running checkpoints retain
that id and an attempt id, so another caller in the same live process cannot
revoke the claim under the guise of recovery. Blocked decisions are checkpointed
with their reason and require an explicit journal transition before execution.
