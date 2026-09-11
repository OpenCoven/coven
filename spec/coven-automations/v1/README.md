# Coven Automations v1 (`coven.automations.v1`)

Machine-readable contract artifacts for the Coven automations protocol defined in [`docs/architecture/coven-automations-v1.md`](../../../docs/architecture/coven-automations-v1.md) (OpenCoven/coven issue #855, foundation #816).

Cave, the SDK, Psyche adapters, runtimes, and future implementations consume these artifacts — never Coven internals, never hand-maintained parallel types.

## Artifacts

| File | Purpose |
| --- | --- |
| `protocol-version.json` | Contract profile registry; contract version is separate from implementation/release version. |
| `capabilities.json` | Variant negotiation, including explicit negative negotiation (`refused`) — unknown variants fail closed with `CAPABILITY_UNSUPPORTED`. |
| `common.schema.json` | Shared value definitions (ids, digests, principals, timestamps, extension bag). |
| `automation-definition.schema.json` | `AutomationDefinition`: identity, monotonic revision + integrity digest, versioned trigger/condition/action unions, binding, policies, provenance. |
| `automation-occurrence.schema.json` | `AutomationOccurrence`: occurrence key, exact definition revision pin, fence/lease, cancellation/recovery, event window. |
| `automation-run.schema.json` | `AutomationRun`: exact familiar/principal/authority/runtime binding, attempts, terminal disposition, delivery, receipt reference. |
| `automation-attempt.schema.json` | `AutomationAttempt`: adoption key, dispatch fence, worker correlation, retry classification, cursors, ambiguous disposition. |
| `automation-receipt.schema.json` | `AutomationReceipt`: immutable versioned receipt with digests, side-effect class, integrity/authentication, privacy/retention. |
| `command-envelope.schema.json` | Every command (create, revise, activate, pause, disable, tombstone, run now, cancel, retry/recover, list/get/history/health, events read/subscribe, legacy import) + response envelope with adoption-key semantics. |
| `error-envelope.schema.json` | Typed error codes and the frozen HTTP/control-action status mapping. |
| `event-envelope.schema.json` | Changefeed envelope: streams, gapless sequences, event ids, causation, compaction snapshots. |
| `conformance-result.schema.json` | Portable conformance-result envelope with a separately signed statement, exact source/protocol/runner binding, audit-only versus release-eligibility scope, per-profile suite results, freshness, and optional P-256 authentication. |
| `state-machines.json` | Authoritative lifecycle state machines (definition, occurrence, run, attempt) plus the ten normative invariants. |
| `compatibility-matrix.json` | Machine-readable change classes, per-field status, and explicit incompatible-profile refusal rules. |
| `test-vectors.json` | Golden vectors: valid, invalid, unknown-field, downgrade/upgrade, unknown-variant, adoption replay/conflict, revision conflict, duplicate/out-of-order event replay — with pinned RFC 8785 digests. |
| `conformance-result.vectors.json` | Synthetic valid and invalid result-envelope vectors. These are contract tests, not evidence that any implementation or profile passes. |
| `coven.automations.v1.d.ts` | Pinned TypeScript projection of the schemas for SDK/Cave canaries. |

## Compatibility rules

- Unknown schema versions fail closed: `SCHEMA_VERSION_UNSUPPORTED`, never approximation.
- Unknown trigger/condition/action/policy variants fail closed: `CAPABILITY_UNSUPPORTED` naming the variant.
- Schema validity is structural, not capability approval. The reserved `outputTarget.atomic`
  shape remains represented in schemas, types, and golden fixtures, while
  `capabilities.json` explicitly refuses it until delivery is pinned to a
  definition revision and crash-recoverable.
- Unknown fields fail closed (`additionalProperties: false`); optional data travels only in the namespaced `extensions` bag, which is preserved and never interpreted until promoted by a new profile.
- Digests are SHA-256 over RFC 8785 (JCS) canonical JSON — never over ad-hoc serialization.
- Contract profile (`coven.automations.v1`) is independent of implementation release versions.
- Historical records pin the exact definition revision and digest they were created and executed against, and are never reinterpreted by current definitions.
- Durable schedule timezones are canonical `utc` or validated IANA TZIDs. Legacy
  `local` is accepted only at compatibility boundaries, resolved before
  persistence, and recorded as an explicit definition-revision migration. On
  Unix, an effective `TZ` override must itself name `utc` or an exact IANA TZID;
  POSIX rules, zone-file paths, and malformed values fail closed rather than
  silently falling back to the host zone.
- Spring-forward gaps skip nonexistent wall times. Fall-back folds select the
  first occurrence (the earlier UTC instant). Both rules are deterministic and
  pinned by `test-vectors.json`.
- Native retries preserve one run across immutable attempts. Only the protocol
  classes `transient_dispatch`, `lease_expired`, and `runtime_unavailable` may
  auto-retry, and only when pre-side-effect evidence proves the disposition.
  Ownership-retained and ambiguous outcomes never auto-retry.
- Retry eligibility is persisted as `notBefore` from the observed failure
  time. Fixed delays are exact; exponential delays use deterministic full
  jitter bounded to one day. Retry waiting remains inside the original run
  timeout, and exhaustion quarantines the definition until an explicit
  operator release.

## Conformance

Required test suites and canary requirements (Coven, SDK, Cave — each against packed/released artifacts, not source-relative imports) are listed in `conformance-manifest.json`. Golden vectors are self-contained: any draft 2020-12 validator plus the digest recipe in `test-vectors.json` suffices to run them outside the Coven crate.

`conformance-result.schema.json` and `conformance-result.vectors.json` define the
portable envelope accepted by the Rust verifier. The verifier always checks the
JCS statement digest and exact source, protocol artifact, runner artifact, and
vector-set bindings, plus the exact implementation package, binary, archive, or
image exercised by the suites. An `audit_only` result remains audit-only even
when it has a valid trusted signature. A `release_eligibility` result
additionally requires the caller's exact subject-artifact and policy bindings,
a caller-pinned suite inventory for every required passed profile, an exact
match to one caller-allowed environment, bounded freshness, expiry, and a valid
P-256 signature from a caller-supplied trusted key. The result's own
`requiredSuites` values are evidence to compare against that policy; they are
never a release trust root and are not derived from ambient source files. A
policy requiring the `full` profile must also pin the exact suite inventory for
all six component profiles; a producer cannot hide dummy component suites
behind a passed `full` status.

This slice defines and verifies result envelopes only. It does **not** execute
the vectors, certify the daemon or npm packages, provide a production signer or
trust root, or claim that any profile currently passes. JSON Schema proves only
the closed structural shape; it does not prove signatures, digests, suite
execution, profile semantics, caller-pinned release inventories, environment
eligibility, freshness, or artifact equality. `fileCount` follows JSON Schema
integer semantics, so mathematically integral forms such as `19.0` and `1.9e1`
are accepted and normalized to `19`, while fractional, nonpositive, and
non-JCS-safe values are rejected. Runtime Authority and full v1 certification
remain blocked on #857's upstream signed runtime-terminal-evidence producer.

## Immutable bundle

CI packages this directory as
`coven-automations-v1-contract-<source-commit>.tar.gz`. The archive contains
these contract files under `coven-automations-v1/` plus `manifest.json`.
The manifest binds the bundle to the exact source commit, records the SHA-256
and byte size of every contract file, and publishes `contractContentSha256`
over the lexically ordered `relative-path\0sha256\n` pairs. That content digest
excludes the source commit and archive metadata, so consumers can distinguish
unchanged contract bytes from a newly source-bound release bundle.

SDK, Cave, and other canaries must download the exact-commit CI or release
artifact and verify its digest and manifest. Importing this source directory
directly is not a conformance result.
