# Coven Automations audit runner

This directory contains the first executable Coven Automations v1 conformance
slice. It is deliberately limited to **audit-only** observations:

- it cannot emit `release_eligibility`;
- it cannot claim the aggregate `full` profile;
- an unavailable target produces `not_applicable` results and a nonzero exit;
- malformed target output produces `failed` results with static errors;
- raw target evidence, stdout, stderr, paths, and command lines are not copied
  into the result.

The output is a
[`coven.automations.conformance-result.v1`](../../../spec/coven-automations/v1/conformance-result.schema.json)
envelope. Passing output is not release certification. A verifier must still
pin the source, protocol bundle, runner, vector set, tested subject artifact,
suite inventory, and environment. Release eligibility additionally requires
trusted authentication and a release policy.

## Run

```sh
node conformance/automations/runner/conformance.mjs \
  --job /absolute/path/to/audit-job.json \
  --target-command /absolute/path/to/coven
```

The target command must be the exact executable represented by
`subjectArtifact.sha256`. Wrappers and extra target arguments are intentionally
unsupported because they would break the binding between the reported subject
and the implementation that actually ran. The runner reads those bytes once
and starts every probe from a fresh private copy, so one invocation cannot
replace the executable used by the next.

Exit status `0` means every requested suite passed. Status `1` means the runner
emitted a valid non-passing audit result. Status `2` means the job itself was
invalid and no result was emitted.

This initial standalone runner supports Linux and macOS. It fails closed on
Windows until target processes can be contained in a kill-on-close Job Object.
The native Coven target commands themselves remain cross-platform.

## Audit job

The job is a closed JSON object. Its source, protocol, runner, subject, and
environment fields map directly into the result statement:

```json
{
  "schemaVersion": "coven.automations.conformance-job.v1",
  "resultId": "audit-2026-09-08T120000Z",
  "decisionScope": { "kind": "audit_only" },
  "source": {
    "repository": "https://github.com/OpenCoven/coven",
    "commit": "1111111111111111111111111111111111111111"
  },
  "protocolArtifact": {
    "bundleSchemaVersion": "coven.automations.bundle.v1",
    "sourceCommit": "1111111111111111111111111111111111111111",
    "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "fileCount": 19
  },
  "runner": {
    "name": "coven-automations-conformance-runner",
    "version": "0.1.0",
    "artifactSha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "vectorSetSha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  },
  "subjectArtifact": {
    "artifactId": "coven-cli",
    "artifactVersion": "0.1.0",
    "platform": { "os": "linux", "arch": "x86_64" },
    "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
  },
  "environment": {
    "os": "linux",
    "arch": "x86_64",
    "runtime": "node-24"
  },
  "observedAt": "2026-09-08T12:00:00.000Z",
  "suites": [
    {
      "profile": "structural",
      "suiteId": "capability-negotiation",
      "vector": {
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": [
          {
            "caseId": "supported-minimal",
            "definition": {
              "schemaVersion": 1,
              "id": "native-supported",
              "name": "Native supported",
              "status": "PAUSED",
              "rrule": "FREQ=DAILY",
              "timezone": "local",
              "prompt": "Run the native conformance probe",
              "misfire": "latest",
              "overlap": "forbid",
              "timeoutMinutes": 30,
              "runtime": "coven-code"
            },
            "expected": {
              "outcome": "supported"
            }
          }
        ]
      }
    }
  ]
}
```

The example values are placeholders, not evidence. Use the exact SHA-256 and
source metadata for the files and binary under test. `source.commit` must equal
`protocolArtifact.sourceCommit`. The runner also recomputes and requires:

- `runner.artifactSha256` from `conformance.mjs`;
- `runner.vectorSetSha256` from the JCS bytes of the job's `suites` array;
- `subjectArtifact.sha256` from `--target-command`.

The checked-in
[`attempt-terminal-immutability.vectors.json`](attempt-terminal-immutability.vectors.json),
[`capability-negotiation.vectors.json`](capability-negotiation.vectors.json),
[`definition-validation.vectors.json`](definition-validation.vectors.json),
[`event-reducer-determinism.vectors.json`](event-reducer-determinism.vectors.json),
[`occurrence-fence-uniqueness.vectors.json`](occurrence-fence-uniqueness.vectors.json),
[`receipt-integrity-validation.vectors.json`](receipt-integrity-validation.vectors.json),
and
[`run-terminal-monotonicity.vectors.json`](run-terminal-monotonicity.vectors.json)
files contain the current nonempty vector sets.

## Target protocol

The runner invokes the target directly without a shell.

`<target> automations conformance capability` returns:

```json
{
  "schemaVersion": "coven.automations.conformance-target-capability.v1",
  "profiles": [
    {
      "profile": "structural",
      "suites": [
        "attempt-terminal-immutability",
        "capability-negotiation",
        "definition-validation",
        "event-reducer-determinism",
        "occurrence-fence-uniqueness",
        "receipt-integrity-validation",
        "run-terminal-monotonicity"
      ]
    }
  ]
}
```

For each advertised suite, the runner invokes
`<target> automations conformance evaluate`, writes one
`coven.automations.conformance-suite-request.v1` object to standard input, and
expects one `coven.automations.conformance-suite-result.v1` object on standard
output.

The native Coven target currently implements six structural suites.
`attempt-terminal-immutability` executes every terminal attempt state against
the production SQLite ledger and proves that later updates and deletion are
refused. `capability-negotiation` executes the checked-in cases against Rust's
real routine-definition parser and capability policy. `definition-validation`
executes the portable v1 definition parser, integrity verification, and typed
serialization path, accepting a canonical valid definition only when its
normalized JCS digest matches and rejecting a tampered definition.
`event-reducer-determinism` replays a portable event stream through the native
Rust reducer both canonically and with an exact duplicate delivery, requiring
the same normalized final state and expected digest.
`occurrence-fence-uniqueness` executes a portable three-case matrix against the
production SQLite occurrence schema, proving that one automation cannot claim
the same scheduled slot twice while different automations may share a slot and
one automation may claim distinct slots.
`receipt-integrity-validation` parses portable receipts through the native
typed receipt contract. It requires a canonical receipt with a matching JCS
SHA-256 integrity value and a minimally tampered receipt whose unchanged
integrity value must be rejected. Accepted receipts are compared by a pinned
digest of their normalized typed representation; receipt contents are not
returned as target evidence.
`run-terminal-monotonicity` executes the checked-in settlement and replay cases
against the real Rust run ledger in an isolated in-memory store, proving that a
later terminal observation cannot rewrite the first committed terminal state.
Each vector must use distinct first and replay statuses so a passing case
exercises an actual conflict. These suites do not claim the complete definition,
occurrence, or attempt state machines.

The runner computes a JCS SHA-256 digest of returned evidence and discards the
raw evidence after building the result envelope. Evidence is restricted to JSON
values that can be canonicalized consistently across the JavaScript runner and
Rust verifier. The native target rejects evaluation requests larger than one
MiB before JSON parsing.
Each target operation has a two-second deadline followed by process-tree
termination; output is capped at one MiB.
