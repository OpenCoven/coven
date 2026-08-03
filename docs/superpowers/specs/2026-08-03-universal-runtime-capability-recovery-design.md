# Universal Runtime Capability Recovery Design

- **Date:** 2026-08-03
- **Status:** recovered scoped design; documentation only
- **Issue:** [#572](https://github.com/OpenCoven/coven/issues/572)
- **Related work:** preserved design at `7323395a7dc4af99e7557ee0929fafee9c651720:docs/superpowers/specs/2026-07-29-universal-runtime-capability-design.md`, Psyche audit/contract work in [#566](https://github.com/OpenCoven/coven/pull/566) and [#567](https://github.com/OpenCoven/coven/issues/567)

## Decision summary

Recover the useful parts of the abandoned universal-runtime design, but narrow it to
Coven's current authority boundary and public support policy.

The recovered design keeps three principles:

1. **Rust remains the authority** for launch, capability evaluation, and denial.
2. **Raw harness-native discovery stays separate** from effective launch
   capability truth.
3. **Public runtime support stays limited** to Codex, Claude Code, and GitHub
   Copilot CLI until a separate policy decision widens that set.

This recovery does **not** promise endpoint or production implementation work in
this PR. It records the future contract shape and the acceptance bar for later
implementation.

## Current state to preserve and reconcile

### What the daemon already exposes

Under `/api/v1`, Coven already exposes the named public contract
`coven.daemon.v1` through `GET /api/v1/health`. The current implementation also
serves `GET /api/v1/api-version`, but today that route returns the route token
`"v1"` and `supportedApiVersions: ["v1"]`, while public docs describe the named
contract string `"coven.daemon.v1"`. Psyche issue #567 correctly treats that as
an O1 contract gap that must be frozen before runtime-capability work can claim
production conformance.

### What `/api/v1/capabilities/harnesses` already means

`GET /api/v1/capabilities/harnesses` is already a shipped read surface. Its job
is narrow and factual: report Coven-owned skills plus harness-native discoveries
such as global instructions, skills, plugins, warnings, and scan timestamps.
It is **not** an effective launch descriptor, **not** a required-capability
handshake, and **not** the source of truth for whether a runtime may be used for
some requested behavior.

### What Rust types already exist

Coven already has the core authority ingredients for a future effective runtime
layer:

- `api.rs` defines the public daemon contract boundary and structured error
  envelope.
- `harness.rs` defines `HarnessCommandSpec` and `ContinuityArgs` for
  launch-time behavior. Its `capabilities`, `sandbox`, and `stream_args`
  fields use the pinned shared-spec dependency
  `coven_runtime_spec::{Capabilities, SandboxMapping, StreamArgs}`
  (`crates/coven-cli/Cargo.toml` pins tag `v0.2.0`; `Cargo.lock` locks commit
  `2f0e068027f36b1dd32d919f54a40a3baede54c2`). Those shared types are schema
  inputs, but Coven's Rust adapters and launch-time capability evaluation remain
  authoritative for actual argv construction, denial, and spawn behavior.
- `capabilities.rs` defines `HarnessCapabilityManifest`, raw
  `CapabilityWarning { kind, path, message }`, and the aggregate response
  returned by `/api/v1/capabilities/harnesses`.
- `store.rs` and `daemon.rs` define authoritative session lifecycle behavior,
  including persisted states such as `created`, `running`, `completed`,
  `failed`, `killed`, `idle`, and `orphaned`.

The recovered design must build from those Rust-owned types instead of moving
authority into TypeScript or inventing a separate client-owned interpretation.

### What policy must constrain the design

Repository guidance in `AGENTS.md`, `README.md`, and `CONTRIBUTING.md` freezes
public supported harnesses to **Codex, Claude Code, and GitHub Copilot CLI**
until policy and adapter contracts stabilize. `coven-code` remains an internal
tool/runtime detail, not a public familiar runtime. Read-only scans for other
harness ids may continue to exist, but they must not silently become public
runtime support.

## Goals

1. Define one future Rust-owned **effective runtime descriptor** contract for
   supported harnesses.
2. Keep the raw harness-native scan surface intact and backward compatible.
3. Define required-capability evaluation that fails closed before launch.
4. Align future runtime-capability work with Psyche's O1 requirement to freeze
   exact version and lifecycle vocabulary.
5. Prevent speculative harness adapters, widened support policy, or client-side
   authority drift.

## Non-goals

- No production implementation in this PR.
- No new public promise that `cursor`, `gemini`, `opencode`, or any registry
  adapter is supported as a Coven runtime.
- No promise that a new endpoint lands before issue #567's version/lifecycle
  contract freeze.
- No client-side fallback that infers runtime capability from docs, labels,
  scanned files, or UI heuristics.
- No change to the existing meaning of `/api/v1/capabilities/harnesses`.

## Recovered design

## 1. Versioning model

Future runtime-capability work must live under the existing daemon API contract,
not beside it.

- **Daemon contract:** `coven.daemon.v1` remains the public API contract name.
- **Route prefix:** `/api/v1` remains the transport path.
- **Descriptor schema:** the effective runtime payload names its own additive
  schema version, `coven.runtime.descriptor.v1`.

That split gives Coven one stable rule:

- `/api/v1/*` negotiates the daemon contract;
- `descriptorVersion` negotiates the runtime-descriptor document shape.

Breaking descriptor-shape changes require either a new descriptor version or a
new daemon contract version, depending on whether the break is confined to the
descriptor payload or changes broader API semantics.

### Recovery rule for issue #567

No implementation may claim this design is current until the daemon's public
version vocabulary is internally consistent. For O1, the named string
`coven.daemon.v1` is the canonical public vocabulary; the bare route token
`v1` is routing syntax, not the externally meaningful contract name.

## 2. Public runtime support classes

The future resolver distinguishes four support classes:

- `supported_runtime` - public supported harnesses: `codex`, `claude`,
  `copilot`.
- `internal_tool` - Rust-owned tools not exposed as public runtimes:
  `coven-code`.
- `observational_scan` - harness ids that may appear in raw scan output but are
  not public runtime commitments.
- `unknown` - not recognized by the runtime-descriptor resolver.

Only `supported_runtime` entries may appear in the future public effective
runtime-descriptor listing. `internal_tool` and `observational_scan` entries may
have internal structs or diagnostics, but they are excluded from the public
runtime handshake unless a later policy decision explicitly widens support.

### Harness launches versus public runtime descriptors

The recovered design keeps current harness-keyed launches and future public
runtime-descriptor addressing distinct:

- Existing launch surfaces that name a harness directly continue under today's
  Rust harness policy. That includes the current supported public harnesses and
  existing harness-only launches such as `coven-code`.
- Those harness-keyed launches do **not** require a public runtime descriptor to
  exist first.
- A future request that explicitly supplies `runtimeId` opts into the public
  runtime-descriptor resolver. Rust must resolve that id only against the public
  `supported_runtime` set.
- If `runtimeId` is outside that set — including `coven-code`,
  `observational_scan` entries, or unknown ids — the daemon must fail before
  argv construction or spawn with stable `404 descriptor_unavailable`. It must
  not silently fall back to a harness-keyed launch.

### v1 session request semantics

`POST /api/v1/sessions` keeps the current required `harness` field in v1.
`runtimeId` is optional and acts only as an explicit public-runtime selector and
consistency check; it never replaces `harness`.

The canonical public runtime mapping is intentionally closed:

| `harness` | Canonical public `runtimeId` |
|---|---|
| `codex` | `codex` |
| `claude` | `claude` |
| `copilot` | `copilot` |
| `coven-code` | none — harness-only, not a public runtime |

The launch rules are:

1. `harness` remains required and is validated with today's harness launch
   policy first.
2. If `runtimeId` is absent, the daemon preserves the existing harness-only
   launch behavior exactly.
3. If `runtimeId` is present, the daemon first resolves it against the public
   supported-runtime descriptor set. Unknown, unsupported, or non-public ids —
   including `coven-code` and observational-only scan ids — fail before spawn
   with `404 descriptor_unavailable`.
4. If `runtimeId` resolves, it must equal the canonical public runtime mapped
   from `harness`. If it does not, or if the selected harness has no canonical
   public runtime mapping, the daemon fails before spawn with stable
   `400 runtime_harness_mismatch`.
5. Only after both checks pass may the daemon evaluate required capabilities.
   There is no fallback, downgrade, or ignore path for a supplied `runtimeId`.

### Exact session request examples

Accepted legacy harness-only launch:

```json
{
  "projectRoot": "/repo",
  "harness": "codex",
  "prompt": "Fix the tests"
}
```

Accepted explicit public runtime match:

```json
{
  "projectRoot": "/repo",
  "harness": "codex",
  "runtimeId": "codex",
  "prompt": "Fix the tests"
}
```

Rejected public-runtime/harness mismatch:

```json
{
  "error": {
    "code": "runtime_harness_mismatch",
    "details": {
      "harness": "codex",
      "runtimeId": "claude",
      "canonicalRuntimeId": "codex"
    }
  }
}
```

Rejected non-public runtime id:

```json
{
  "error": {
    "code": "descriptor_unavailable",
    "details": {
      "runtimeId": "coven-code"
    }
  }
}
```

Preserved harness-only internal-tool launch:

```json
{
  "projectRoot": "/repo",
  "harness": "coven-code",
  "prompt": "Open the TUI"
}
```

Rejected attempt to pair a harness-only tool with a public runtime selector:

```json
{
  "error": {
    "code": "runtime_harness_mismatch",
    "details": {
      "harness": "coven-code",
      "runtimeId": "codex",
      "canonicalRuntimeId": null
    }
  }
}
```

## 3. Rust authority types

Future implementation should add explicit Rust-owned effective-descriptor types,
separate from both `HarnessCommandSpec` and `HarnessCapabilityManifest`:

```rust
pub struct EffectiveRuntimeDescriptor {
    pub descriptor_version: String,
    pub runtime_id: String,
    pub runtime_label: String,
    pub support_class: RuntimeSupportClass,
    pub admission: RuntimeAdmission,
    pub availability: AvailabilityDescriptor,
    pub capabilities: BTreeMap<String, CapabilityDescriptor>,
    pub native_integrations: NativeIntegrationSummary,
    pub warnings: Vec<RuntimeWarning>,
}

pub struct RuntimeWarning {
    pub code: RuntimeWarningCode,
    pub scope: RuntimeWarningScope,
    pub capability_id: Option<String>,
    pub path: Option<String>,
    pub message: String,
}
```

Supporting types should include:

- `RuntimeSupportClass`
- `RuntimeAdmission`
- `AvailabilityDescriptor`
- `CapabilityDescriptor`
- `CapabilityState`
- `CapabilityReason`
- `NativeIntegrationSummary`
- `RequiredCapabilitySet`
- `RequiredCapabilityEvaluation`
- `RuntimeWarningCode`
- `RuntimeWarningScope`

These are **derived authority types**. They do not replace existing launch
specs or scan manifests. Instead:

- `HarnessCommandSpec` stays the source for launch mechanics;
- `HarnessCapabilityManifest` stays the source for raw native scan data; and
- `EffectiveRuntimeDescriptor` becomes the source for client-facing capability
  truth.

`RuntimeWarning` is a stable contract type, not an open-ended passthrough bag.
For descriptor v1 its fields and enums are:

- `code` (`RuntimeWarningCode`, serialized snake_case):
  - `launch_spec_gap`
  - `host_availability_undetermined`
  - `native_scan_parse_error`
  - `native_scan_permission_denied`
- `scope` (`RuntimeWarningScope`, serialized snake_case):
  - `runtime`
  - `capability`
  - `native_integration`
- `capability_id` (`Option<String>`) — present only when the warning is tied to
  one capability family such as `conversation.stream`.
- `path` (`Option<String>`) — present only for warnings derived from local file
  inspection.
- `message` (`String`) — human-readable explanatory prose. Clients may display
  it, but compatibility branches on `code`, `scope`, and the other structured
  fields rather than prose text.

Descriptor v1 warning mapping is intentionally closed:

- raw `CapabilityWarning.kind == "parse_error"` maps to
  `code = native_scan_parse_error`;
- raw `CapabilityWarning.kind == "permission_denied"` maps to
  `code = native_scan_permission_denied`; and
- any widened raw warning-kind vocabulary requires an explicit descriptor-schema
  update rather than passthrough strings.

`warnings` is an ordered list, not a set. The resolver returns it in this
stable order:

1. `scope` order: `runtime`, then `capability`, then `native_integration`;
2. within a scope, `code` order as declared above;
3. then `capability_id` (lexicographic, missing last);
4. then `path` (lexicographic, missing last); and
5. finally `message` (lexicographic).

## 4. Effective descriptor semantics

The resolver computes one descriptor per supported runtime from this
intersection:

```text
supported harness policy
  ∩ bundled Rust launch spec
  ∩ current host availability
  ∩ passive native scan evidence
  ∩ local enforcement capability
  = EffectiveRuntimeDescriptor
```

Initial capability families should be limited to behavior that already has a
clear Rust launch or lifecycle interpretation:

- `launch.text`
- `model.selection`
- `prompt.system`
- `access.read_only`
- `filesystem.additional_directories`
- `conversation.stream`
- `conversation.resume`
- `conversation.preassigned_session_id`
- `reasoning.think`
- `reasoning.speed`
- `transport.local`

Capabilities such as remote SSH execution, attachments, profile binding, or
structured tool telemetry may be added later, but only when a Rust-owned
contract and enforcement model exist.

Each capability has exactly one state:

- `supported` - available and enforceable on this host
- `unavailable` - conceptually supported, but missing a required local
  dependency or host condition
- `unverified` - declared or inferred, but not yet backed by sufficient local
  evidence for safe use
- `unsupported` - not provided by the supported runtime contract

The descriptor must also carry a machine-readable `reason`. Descriptor v1 keeps
that reason vocabulary closed:

- `harness_not_installed`
- `version_unknown`
- `version_too_old`
- `capability_not_advertised`
- `policy_denied`
- `platform_unsupported`

`supported` is the only state that serializes `reason: null`. Every
non-supported capability state must serialize exactly one of the closed enum
values above. The initial v1 mapping is:

| `state` | `reason` | Required mapping rule |
|---|---|---|
| `supported` | `null` | Capability is available and enforceable on this host. |
| `unavailable` | `harness_not_installed` | The required harness executable or local adapter prerequisite is missing. |
| `unavailable` | `version_too_old` | The harness version is known and below the minimum version needed for the capability. |
| `unverified` | `version_unknown` | The harness exists, but the daemon cannot determine a trustworthy version or equivalent evidence yet. |
| `unsupported` | `capability_not_advertised` | The supported runtime contract or passive native evidence does not advertise the capability. |
| `unsupported` | `policy_denied` | Coven intentionally withholds the capability even if a native surface may exist, because the public support policy does not admit it. |
| `unsupported` | `platform_unsupported` | The capability is outside the supported runtime contract for the current OS or host class. |

Clients may improve copy, but they must not reinterpret either the `state` or
the closed `CapabilityReason` value.

## 5. Relationship to `/api/v1/capabilities/harnesses`

The existing harness-capability routes remain intact:

- `GET /api/v1/capabilities/harnesses`
- `GET /api/v1/capabilities/:harnessId`

They continue to expose raw native discoveries and warnings with the current
snake_case payloads.

Future effective runtime descriptors must use a **separate read surface** so the
existing route does not acquire a second meaning. The preferred shape is:

- `GET /api/v1/runtimes`
- `GET /api/v1/runtimes/:runtimeId`
- optional later refresh action only after the O1 contract freeze lands

This keeps the distinctions clean:

- `/capabilities` = control-plane catalog
- `/capabilities/harnesses` = raw native scan facts
- `/runtimes` = effective runtime capability truth

## 6. Required capability evaluation

Future runtime launches must accept a caller-supplied required-capability set,
validated in Rust before the daemon constructs argv or spawns a process.

The evaluation algorithm is:

1. Reject the request if a required capability id is unknown to the daemon.
2. Resolve the effective descriptor for the requested supported runtime.
3. Reject the request if any required capability is not in `supported` state.
4. Return the descriptor-backed denial reason in structured details.
5. Launch only when every required capability is `supported`.

### Evaluation result shape

The internal Rust result should distinguish:

- `accepted`
- `descriptor_unavailable`
- `runtime_harness_mismatch`
- `unknown_capability`
- `unsupported_capability`
- `unavailable_capability`
- `unverified_capability`

### Error and compatibility behavior

The HTTP API continues to use the existing structured error envelope.

For the recovered design, the compatibility rules are:

- Unknown required capability id: `400 invalid_request`
- Unknown or publicly unsupported runtime id on the future `/api/v1/runtimes`
  surface: `404 descriptor_unavailable`
- Explicit `runtimeId` on a future launch request that is not in the public
  supported-runtime set — including `coven-code` — must fail with the same
  stable `404 descriptor_unavailable` before argv construction or spawn, while
  harness-keyed launches without `runtimeId` continue under the current harness
  policy.
- Explicit supported `runtimeId` values that do not equal the canonical public
  runtime mapped from the required `harness` must fail before spawn with stable
  `400 runtime_harness_mismatch`.
- Known capability not in `supported` state: fail closed before launch with
  `409 runtime_capability_not_met` and `details` containing at least
  `{ runtimeId, capability, state, reason }`
- Clients branch on code and `details`, never prose

Because issue #567 is still open, this design does **not** claim that
`descriptor_unavailable`, `runtime_harness_mismatch`, or
`runtime_capability_not_met` already exists today. The future implementation
must add them explicitly and document them in `docs/API-CONTRACT.md` and
`docs/reference/api.md` in the same change.

## 7. Lifecycle and descriptor truth

Psyche's W1 audit requires exact terminal vocabulary and exact capability
negotiation. The recovered runtime design therefore adopts two hard rules:

1. Effective runtime descriptors must reference only authoritative persisted
   lifecycle states documented by Coven's Rust store/runtime behavior:
   `created`, `running`, `completed`, `failed`, `killed`, `idle`, and
   `orphaned`. Archive remains separate metadata in `archived_at`, not a
   lifecycle status.
2. `idle` is an authoritative persisted status, not a UI-only synonym. Rust
   writes `idle` when a conversation-grouped session child exits cleanly but
   the conversation remains extendable via `conversation_id`; the exit event for
   that child still records `completed`, so consumers must preserve both pieces
   of meaning instead of flattening `idle` into generic success prose.

A runtime descriptor may advertise conversation features such as resume or
streaming, but it must not redefine session terminal semantics.

## 8. Future implementation acceptance

A future implementation satisfies this recovered design only if all of the
following are true:

1. **Version vocabulary is frozen first.** O1 lands and public docs/tests agree
   on `coven.daemon.v1` and the exact persisted lifecycle vocabulary.
2. **Rust owns descriptor generation.** No TypeScript package or UI computes the
   effective capability truth independently.
3. **Raw scans stay raw.** `/api/v1/capabilities/harnesses` remains backward
   compatible and distinct from effective descriptors.
4. **Support policy stays scoped.** The public descriptor list includes only
   Codex, Claude Code, and GitHub Copilot CLI. `coven-code` remains excluded as
   a public runtime, and observational scans do not silently become supported.
5. **Session requests stay unambiguous.** `harness` remains required,
   `runtimeId` stays optional, unsupported/non-public runtime ids return
   `descriptor_unavailable`, supported mismatches return
   `runtime_harness_mismatch`, and supplied `runtimeId` values are never
   ignored or used as fallback hints.
6. **Required capabilities fail closed.** The daemon rejects unknown or unmet
   required capabilities before launch.
7. **Capability states are machine-readable.** Non-supported states expose exact
   `state` values plus the closed v1 `CapabilityReason` enum, and `supported`
   serializes `reason: null`.
8. **Docs and tests land together.** Any new runtime endpoint or error code ships
   with exact request/error examples in contract docs and executable regression
   coverage that asserts the exact enum/code values.
9. **No speculative adapters.** The work does not promise registry adapters,
   downloadable plugins, or widened harness support.

## Implementation boundary

This recovery PR is documentation only. It restores the design intent as a
scoped future contract and records the acceptance bar. It does **not** add a
new endpoint, change the daemon API, change launch behavior, or widen the
supported harness set.
