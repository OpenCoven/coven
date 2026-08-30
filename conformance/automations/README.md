# Coven Automations Conformance Plane (coven.automations.conformance v1)

An executable, implementation-independent certification plane for Coven
Automations v1 (OpenCoven/coven#858, parent program #854). A release must
**prove** — not merely claim — that schedule semantics, state transitions,
identity/authority binding, crash recovery, duplicate prevention, privacy
controls, and client compatibility hold at the exact candidate revision and in
the packed artifacts users install.

Everything here is consumable **without linking Coven private modules**: the
vectors are plain JSON documents, the schemas are plain JSON Schema, and the
runner is a dependency-free Node script.

```
conformance/automations/
  manifest.json          plane version, profiles, artifact ids, hard gates
  schemas/               versioned artifact schemas (definition, occurrence,
                         run, event, receipt, authority) + vector/report/doctor
  vectors/               certification vectors by concern (definitions,
                         schedules, state-machines, idempotency, events,
                         identity-authority, receipts, privacy, diagnostics)
  scenarios/             the 22 golden end-to-end scenarios
  runner/                the standalone runner + reference oracle + tests
  slo/                   release SLO gates (provisional until measured)
  reports/               run receipts (local artifacts, never committed)
```

## Profiles

Confidence is never collapsed into one `compliant` bit. Every result names
exactly which profile and artifact versions passed:

| Profile | Certifies |
| --- | --- |
| `structural` | schema, canonicalization, state-machine, and compatibility validity |
| `scheduler-reliability` | time, timezone/DST, occurrence fencing, misfire, overlap, retry, cancellation, lease, crash/restart, backpressure |
| `runtime-authority` | authenticated principal, familiar embodiment, capability/approval decision, runtime descriptor, fail-closed dispatch |
| `continuity` | exact familiar root/revision, historical rehydration/correlation |
| `privacy` | access control, minimization, redaction, retention, erasure/tombstone, changefeed projection |
| `interoperability` | SDK/Cave/Psyche/runtime clients consuming pinned artifacts and replaying events correctly |
| `full` | all required v1 profiles at one immutable compatibility set |

## Running

```sh
# full certification against the reference oracle (zero dependencies)
node conformance/automations/runner/conformance.mjs --profile all \
  --fuzz 500 --report conformance/automations/reports/last-run.json

# one profile
node conformance/automations/runner/conformance.mjs --profile structural

# reproduce a single vector (debug run, not a certification)
node conformance/automations/runner/conformance.mjs \
  --vector schedules.dst-spring-gap --target reference-oracle

# list the vector inventory
node conformance/automations/runner/conformance.mjs --list

# unit tests for the runner itself
node --test conformance/automations/runner/conformance.test.mjs
```

The runner exits nonzero unless the gate passes: zero failures across every
required profile, and no missing profile.

## Targets

Vectors are target-agnostic operation scripts. The same vector certifies any
target that speaks the `coven.automations.conformance.v1` capability:

- `reference-oracle` (default) — this plane's own deterministic model; always
  available. It proves the vectors are self-consistent and executable.
- `daemon` — a running daemon endpoint over the local socket API.
- `packaged-release` — a packed npm artifact's binary, run without
  source-relative imports.

A target that does not advertise the capability yields **skipped** vectors —
reported separately from failures, never silently passed. The cross-repo
canaries (SDK, Cave, Psyche, Familiar Contract, Threads, packed artifacts) are
vectors with pinned-artifact prerequisites; they run once those artifacts
exist and are skipped with reasons until then.

## Vector shape

Every vector declares profile, version, prerequisites, input, and the exact
expected events/state/receipt/refusal behavior:

```json
{
  "vectorId": "schedules.dst-spring-gap",
  "vectorVersion": 1,
  "profile": "scheduler-reliability",
  "category": "schedules",
  "virtualTime": { "start": "2026-03-08T05:00:00.000Z", "hostTimezone": "America/New_York" },
  "input": { "definitions": [ ... ], "operations": [ ... ] },
  "expected": { "occurrences": [ ... ], "dispatchCount": 1, "invariants": [ ... ] }
}
```

Failures identify the invariant, object ids, event cursor, expected/observed
state, and the exact safe reproduction command. Reports are machine-readable
(`conformance.report.v1`), carry the source revision and per-artifact SHA-256
digests, and are redacted before writing: prompt text, secrets, private
memory, and irrelevant absolute paths never leave the plane.

## Hard gates

The certification gate enforces, over every vector, scenario, and randomized
operation sequence:

1. zero duplicate dispatches for a single fence;
2. zero silent eligible-occurrence loss;
3. zero false success under injected failures;
4. bounded recovery and resource growth (terminal-state monotonicity,
   fence uniqueness, and bounded ledger growth are checked continuously, not
   only at the end).

## Load and SLO

`slo/slo.v1.json` defines the supported local profile, the measured
quantities (latency distributions, queue depth, contention, throughput,
growth, recovery latency, CPU/memory), and the release gates. Gates are
**provisional** until a baseline is measured on the exact release artifact;
the runner then evaluates a measured report:

```sh
node conformance/automations/runner/conformance.mjs --profile all --slo measured.json
```

Missing measures report `provisional` — never a silent pass.

## Operator diagnostics

`diagnostics.doctor.v1` is the contract for the operator surface
(`coven automations doctor/status/explain/occurrence/run/attempts/leases/
events/schedule/reconcile/retry/cancel`). Every supported unhealthy state maps
to a stable finding code with subject ids, a redacted observation, and exact
safe next steps — read-only unless an explicitly guarded operation is
requested (`--dry-run` review first, `--expected-state` guards for retries).
No diagnostic ever recommends deleting rows or blindly rerunning ambiguous
mutating work. The diagnostics vectors under `vectors/diagnostics/` prove
each finding code end to end.

## CI interface

```sh
./scripts/agent-bootstrap
./scripts/agent-check fast                    # deterministic vectors, no network
./scripts/agent-check full                    # + privacy/secret guards, all profiles
./scripts/agent-check automations-conformance # the certification plane only
```

All commands leave the worktree clean and emit machine-readable receipts to
`reports/` (gitignored). Unsupported platform capabilities (e.g. no cargo on
the host) are reported as separate `unsupported-platform` entries, distinct
from failures.

## What slice 1 covers

This plane is slice 1 of the #858 program. Shipped: the runner, the full
vector/scenario inventory, all seven profiles reported separately, the
reference oracle, SLO gate semantics, the doctor contract with finding-code
coverage, and the agent/CI interface. Remaining for later slices: wiring the
daemon/packaged-release target adapters (requires the
`coven.automations.conformance.v1` capability in the Rust implementation),
executing the cross-repo canaries against pinned producer artifacts, running
the load harness to ratify the SLO baselines, and the access-control vectors
(`coven.automations.acl.v1`).
