# Coven Agents Rust Design

## Decision

Build an independent Rust orchestration core for OpenCoven, but do not attempt a
feature-for-feature port of the OpenAI Agents SDK.

The useful common denominator is a provider-neutral run loop around agents,
tools, handoffs, guardrails, sessions, and lifecycle events. OpenAI-specific
hosted tools, Responses transport, realtime voice, and tracing export belong in
adapters. OpenCoven's project boundaries, harness sessions, familiar identity,
memory, and user approvals remain owned by their existing systems.

## Placement

Start as the leaf crate `crates/coven-agents` in the MIT-licensed `coven`
workspace. It must not depend on `coven-cli`, the daemon, or another workspace
crate. This validates the API in the ecosystem's Rust authority repository
without wiring unfinished orchestration into the product. If the crate develops
independent consumers and release cadence, extract it into a dedicated
repository before a stable release.

Adding the same code to `coven-code` would couple a reusable SDK to a GPL,
application-specific coding loop. Adding it to Cave would put authority logic in
a desktop client. Both are rejected.

## Clean-room boundary

The design is derived from public behavioral documentation and OpenCoven
requirements. No upstream implementation code is copied or translated. The
upstream project is MIT-licensed, so this boundary is an architectural and
provenance choice rather than a license requirement.

Shared generic vocabulary such as agent, runner, tool, handoff, guardrail,
session, trace, and model is treated as domain terminology. Types and behavior
are designed for Rust and OpenCoven instead of preserving Python API parity.

## MVP architecture

### Agent catalog

An `Agent<C>` contains a stable id, display name, instructions, a model adapter,
tools, handoffs, and input/output guardrails. Handoffs reference stable agent
ids rather than owning recursive agent values, avoiding reference cycles and
making the graph validate before execution.

### Model adapter

`Model<C>` receives a complete `ModelRequest`: active identity, instructions,
conversation items, tool definitions, and handoff definitions. It returns an
optional assistant message plus model actions. Provider adapters own wire-format
translation.

### Deterministic runner

The runner:

1. validates all ids, tool names, handoff names, and handoff targets;
2. runs starting-agent input guardrails before any model or tool side effect;
3. loads optional session history;
4. calls the active model;
5. executes tool calls sequentially or changes active agent for one handoff;
6. repeats within explicit turn and handoff limits;
7. runs final-agent output guardrails;
8. appends successful runs to the session;
9. emits metadata-only lifecycle events.

Sequential tool execution is intentional in the MVP. Parallel execution needs
an explicit ordering, cancellation, and side-effect policy and should not be
implied by an implementation detail.

### Extension traits

- `Tool<C>`: JSON-schema description plus asynchronous local execution.
- `InputGuardrail<C>` and `OutputGuardrail<C>`: blocking checks returning allow
  or a reasoned rejection.
- `SessionStore`: load and append conversation items.
- `RunObserver`: non-failing metadata event sink.

Application context is an immutable `&C`; mutable dependencies use explicit
interior mutability. This makes sharing across asynchronous tools and model
adapters visible in their types.

## Error and persistence semantics

Configuration errors are detected before a run. Runtime errors preserve the
agent, tool, guardrail stage, operation, and source error where applicable.
Unknown model-requested tools or handoffs fail closed.

Tool call ids must be unique across a run, including ids already present in
loaded session history. A `ToolResult` correlates to its call by id, so a reused
id makes the transcript ambiguous and no consumer can pair a result with the
call that produced it. The runner screens an entire response for duplicates
before executing any of its tools, so a duplicate late in a batch cannot leave
the side effects of earlier calls behind. Violations fail closed with
`RunError::DuplicateToolCallId`.

Input guardrails are blocking by default. This is safer for a local tool runtime
because rejected input cannot race with model calls or tool side effects.

The MVP appends to a session only after a final output passes output guardrails.
Failed or interrupted runs remain observable through the caller and observer,
but are not written as successful conversation history. Durable resumability
requires a separate run-state journal and is deferred.

A failed run returns `RunFailure`, which pairs the `RunError` with the
transcript the run had produced when it failed, plus the turn and handoff
counts. A tool that fails mid-turn does not invalidate the user message,
assistant message, and completed tool results that preceded it, so discarding
them would destroy work the caller cannot reconstruct: the conversation could
not be rendered, logged, or retried. `RunFailure` therefore returns those items
instead. Because the runner still refuses to append them to the session, a
partial transcript can never become durable history by accident; persisting one
is an explicit caller decision.

Session load and append are separate operations. Session-store implementations
must serialize writers per session id or implement optimistic concurrency
control; the runner does not merge concurrent turns.

Observers receive names, ids, counts, and state transitions, not prompts,
arguments, outputs, or model reasoning. Sensitive payload capture must be an
explicit adapter policy.

Every run emits exactly one `RunStarted` event followed by exactly one terminal
`RunCompleted` or `RunFailed` event. This holds even when the starting agent is
unregistered, so observers that open per-run state on the start event always
receive a matching close.

## Non-goals

- OpenAI or Anthropic HTTP clients
- compatibility with the Python SDK API
- voice or realtime pipelines
- hosted tools
- MCP integration
- sandbox lifecycle
- SQLite persistence
- human approval interruption/resume
- retries or automatic replay of side-effecting calls
- daemon, CLI, Cave, or `coven-code` integration

## Testing

Tests use deterministic fake models and local tools. They cover final output,
tool round trips, handoffs, guardrail short-circuiting, session history,
configuration validation, observer metadata, and bounded-loop failures. No
network credentials or provider APIs are required.

## Follow-up risks

The principal risk is prematurely stabilizing a public API before provider,
approval, and durable run-state adapters exercise it. Keep the crate
experimental and workspace-local until at least two real consumers share it.
The next coherent slice is a typed OpenAI/Anthropic-neutral test adapter or a
Coven event-ledger adapter, not broad feature parity.
