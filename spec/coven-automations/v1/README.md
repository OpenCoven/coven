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
| `state-machines.json` | Authoritative lifecycle state machines (definition, occurrence, run, attempt) plus the ten normative invariants. |
| `compatibility-matrix.json` | Machine-readable change classes, per-field status, and explicit incompatible-profile refusal rules. |
| `test-vectors.json` | Golden vectors: valid, invalid, unknown-field, downgrade/upgrade, unknown-variant, adoption replay/conflict, revision conflict, duplicate/out-of-order event replay — with pinned RFC 8785 digests. |
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

## Conformance

Required test suites and canary requirements (Coven, SDK, Cave — each against packed/released artifacts, not source-relative imports) are listed in `conformance-manifest.json`. Golden vectors are self-contained: any draft 2020-12 validator plus the digest recipe in `test-vectors.json` suffices to run them outside the Coven crate.
