# Coven goal-loop design

## Decision

Port Looper's reusable orchestration idea into `coven-agents`, not its Go daemon
or GitHub-specific planner, reviewer, fixer, and worker implementation.

The first slice is a provider-neutral `GoalLoopRunner` that repeatedly invokes
the existing deterministic `Runner` until an injected evaluator declares the
goal complete. It is the runtime primitive for the existing
`coven.workflow.v1` `loop-until-done` pattern.

## Boundaries

`GoalLoopRunner` owns:

- bounded iteration sequencing;
- exit-criteria evaluation;
- atomic compare-and-set checkpoint transitions at iteration boundaries;
- safe resume from a pending checkpoint;
- fail-closed handling of an ambiguous in-flight checkpoint;
- checkpoint discovery after process or machine restart;
- explicit reconciliation of external state observed while Coven was offline;
- process-instance fencing so recovery cannot revoke a live iteration claim;
- persistent blocked state for operator-owned recovery.

The crate supplies an immutable-generation `FileLoopJournal` for local durable
recovery. Higher-level adapters still own:

- model and harness transport;
- daemon scheduling and cancellation;
- worktrees, claims, and forge state;
- human approvals and UI.

This preserves `coven-agents` as a workspace leaf. It does not depend on
`coven-cli`, SQLite, GitHub, Cave, or Coven Code.

## State model

Each loop checkpoint stores the loop id, stable starting-agent id, completed
iteration count, status, and optional next input.

Valid automatic progression is:

```text
pending -> running -> pending
                   -> completed
                   -> blocked
                   -> failed
                   -> exhausted
```

A process or machine that restarts from `pending` can safely continue from the
saved input. `LoopJournal::list` lets the daemon discover work without relying
on volatile scheduler state.
A restart from `running` is ambiguous because the model or tools may already
have caused external side effects. Without a reconciler, the runner refuses
automatic replay rather than guessing. A `LoopReconciler` may inspect the
authoritative external system and return one of five explicit decisions:

- leave the checkpoint unchanged;
- mark the goal complete;
- resume with a supplied input after proving replay is safe;
- confirm the in-flight iteration completed and continue with the next input;
- block for operator input.

A running checkpoint records both an attempt id and a process-lifetime instance
id. A runner must use an id that changes on every daemon boot. It refuses to
reconcile a running attempt owned by its own process instance, preventing a
second local caller from turning live work back into pending work. After reboot,
the new process id can reconcile the orphaned attempt. A blocked decision and
its reason are persisted atomically; execution does not retry it until an
operator or policy adapter explicitly transitions the checkpoint.

A malformed `pending` checkpoint without saved input also fails closed rather
than silently running with empty input.

The pending-to-running transition is an atomic claim. Two callers may read the
same pending checkpoint, but only one compare-and-set may advance it and invoke
the agent. Reaching the iteration limit transitions directly from `running` to
`exhausted`, leaving no crash window in which the final iteration appears
resumable.

## Evaluation

`LoopEvaluator<C>` receives the completed iteration number, its `RunResult`, and
the application context. It returns either:

- `Complete`; or
- `Continue { input }`.

The evaluator is the code-level form of a workflow's `exit_criteria`. Keeping it
outside the model run prevents an agent from silently deciding that its own
work meets the goal.

## Verification

Focused tests cover convergence, resume from a pending checkpoint, refusal to
replay an in-flight iteration, malformed pending state, iteration exhaustion,
starting-agent identity mismatch, a compare-and-set race in which only one
iteration claim may succeed, file-journal reconstruction and discovery,
reboot-style resume, offline reconciliation of an in-flight iteration, live
attempt fencing, and persistent blocked recovery state.

## Deferred adapters

This slice deliberately does not add a daemon route, GitHub recipe, or visual
workflow editor. The daemon still needs to select a journal root, rebuild the
appropriate model/evaluator/reconciler adapters for discovered checkpoints, and
schedule them during startup.
