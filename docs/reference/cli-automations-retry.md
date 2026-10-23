---
summary: "Retry an automation run held after a failure, before its deadline, through the owner-local daemon."
read_when:
  - Retrying a routine's failed run before its deadline settles it
  - Acting on a retry_held item from coven automations explain
  - Looking up coven automations retry
title: "coven automations retry"
description: "Reference for coven automations retry: the attempt it must name, why each call uses a fresh adoption key, its refusals, and its JSON output."
source_adjacent_reason: "Tracks the retry command in crates/coven-cli/src/automations/retry_cli.rs over the attempt.retry.v1 handler in crates/coven-cli/src/automations/attempt_retry.rs, including its refusals and JSON contract."
---

`coven automations retry` opens the next attempt of a run held after a
failure. When a run fails with no automatic retry left, the run and the
routine's claims stay held until the run's deadline, and
`coven automations explain` lists it as `retry_held`. The first scheduler pass
at or after the deadline settles the run `failed`. Before then, this command
sends `coven.automations.attempt.retry.v1` to the owner-local daemon. The daemon
opens the next attempt, classified `operator_retry`, and returns the occurrence
to `planned`, and the scheduler dispatches it.

```sh
coven automations attempts <run-id> --json
coven automations retry <run-id> --prior-attempt 1 --prior-disposition failed
coven automations retry <run-id> --prior-attempt 2 --prior-disposition timed_out --note "after the outage" --json
```

## Options

| Option | Notes |
| --- | --- |
| `<RUN_ID>` | The held run. |
| `--prior-attempt <N>` | The run's latest attempt number, at least 1. |
| `--prior-disposition <D>` | How that attempt settled: `failed`, `timed_out` or `cancelled`. |
| `--note <TEXT>` | Recorded with the new attempt. Not blank, at most 500 characters, and with no control characters such as newlines. It is not printed back. |
| `--json` | Prints only the JSON. Without it, a heading comes first. |

The daemon opens the attempt only when the run's latest attempt is the one
named, with that disposition, so a retry never lands on an attempt it did not
mean. Read the latest attempt with `coven automations attempts <run-id>` first.
Running the command twice opens one attempt. The second call finds the new
attempt still live, or names an attempt that is no longer the latest, and is
refused.
An ambiguous attempt is never retried. Its occurrence is recovered with
`occurrence.recover.v1` instead.

When the command gets no answer from the daemon, or an answer it cannot check,
it says the outcome is unknown: the daemon may have opened the attempt. Check
`coven automations attempts <run-id>` before running it again. When there was
no answer, `coven daemon status` shows whether the daemon is running.

Each call sends a fresh adoption key. The daemon stores a refusal under its key
and replays it, so a reused key would repeat a refusal whose cause has since
cleared, such as a stop that has since been reconciled.

## Refusals

Like the inspection commands, `retry` needs a compatible running daemon on
owner-local IPC and never starts one; if it is unavailable, check
`coven daemon status`. A refused retry exits unsuccessfully with one of these
fixed messages. The daemon's own error text is never printed.

| When | Message |
| --- | --- |
| No such run | `automation run was not found` |
| A Runtime Authority run | `a Runtime Authority run cannot be retried by this command` |
| The latest attempt has another number or disposition | `the run's latest attempt is not the attempt and disposition named; check ...` |
| The latest attempt is ambiguous | `the run's latest attempt is ambiguous; recover its occurrence instead` |
| A stop fence or pending cancellation on the run | `the run has an unresolved stop, so it cannot be retried` |
| The run's deadline has passed | `the run's deadline has passed; it cannot be retried` |
| Any other reason the run is not held for a retry | `the run is not held for a retry: ...` |

The last row covers these cases:

- the run is not running;
- it has no occurrence or no attempt, or its occurrence is no longer claimed or
  running;
- its latest attempt is still live;
- it has no deadline;
- it has used all ten attempts;
- its routine is not active, or is quarantined.

Any other refusal reports only its HTTP status.

## Output

```json
{
  "schemaVersion": "coven.automations.retry.v1",
  "retry": {
    "runId": "run-1",
    "occurrenceId": "occurrence-1",
    "attemptId": "attempt-run-1-2",
    "attemptNumber": 2,
    "priorDisposition": "failed",
    "status": "planned",
    "retriedAt": "2026-10-23T12:00:00.000Z"
  }
}
```

`status` is the occurrence's state after the retry. The run still holds the
routine's claims: `coven automations explain` lists it as `running_run`, and as
`eligible_now` while the scheduler can claim it. Before printing, the command
checks that the answer is the retry it asked for. Nothing else is printed: no
adoption key, note or daemon reason.
