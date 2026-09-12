---
summary: "Drive Threads deadline and restart scenarios through the real daemon without correctness sleeps."
read_when:
  - Building deterministic Threads daemon acceptance tests
title: "Threads test clock"
description: "An opt-in, non-production clock fixture for real Threads scheduler and restart tests."
source_adjacent_reason: "Documents the feature-gated Threads clock and its owner-local test controls."
---

# Threads test clock

The `threads-test-clock` feature lets you advance a synthetic daemon home's
logical time and run its real proposal scheduler without waiting on wall time.

```sh
cargo test --locked -p coven-cli --features threads-test-clock
```

**Use this feature only with disposable synthetic homes.** Default builds do
not expose its routes, inspect fixture activation, or replace wall-clock time.
Do not enable it in release builds or a real familiar's home.

## Activate a fixture before starting the daemon

Under your temporary `COVEN_HOME`, create
`test-fixtures/threads-deterministic-clock/` with these files:

| File | Contents |
| --- | --- |
| `enabled` | `threads_test_clock_v1` followed by a newline |
| `capability` | A fresh per-run fixture token, never a production credential |
| `state.json` | `{"now":"2026-09-09T10:00:00Z"}` |

Both fixture directories must be owner-only private directories (`0700` on
Unix), and all three files must be private regular files (`0600` on Unix).
Symlinks, malformed state, missing activated state, and insecure permissions
fail closed. Stray capability or state files without `enabled` do not activate
the clock.

Start the feature-built daemon against that home through the normal daemon
startup path. Activated fixtures suppress the background Threads scheduler;
explicit ticks below are the only way to progress proposal deadlines.

## Advance and tick through the daemon socket

Send these JSON requests over the owner-local daemon transport. Substitute
your per-run token for `YOUR_FIXTURE_TOKEN`.

```http
POST /api/v1/internal/threads/test-clock
Content-Type: application/json

{"capability":"YOUR_FIXTURE_TOKEN","now":"2026-09-09T10:00:59Z"}
```

```http
POST /api/v1/internal/threads/test-clock/tick
Content-Type: application/json

{"capability":"YOUR_FIXTURE_TOKEN"}
```

Time inputs, persisted state, and control response timestamps use RFC 3339.
Updates cannot move time backwards. A tick returns the number of completed
proposals from one bounded `process_due_threads_proposals` pass; it does not
manufacture pending envelopes, verdicts, or audit rows.

Restart with the same disposable home to retain logical time and exercise
normal proposal recovery. Advance to just before `min_visible`, exactly the
earliest close, and beyond the deadline. Submit proposals through supported
intake, and inspect filesystem, pending, and `ward_audit` effects after each
step. The fixture token controls time only; it never supplies proposal,
approval, or protected-write authority.

See [Threads terminal recovery](threads-terminal-recovery.md) for typed close
evidence and the distinction between recovering pending work and quarantining
an unverifiable interrupted apply.
