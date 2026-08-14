# Session Permission Controls — Technical Plan

**Status:** Draft for familiar review
**Depends on:** [`PRODUCT.md`](PRODUCT.md), [`../coven-harness-capabilities/PRODUCT.md`](../coven-harness-capabilities/PRODUCT.md), [`../coven-harness-capabilities/TECH.md`](../coven-harness-capabilities/TECH.md), [`../../docs/ENGINE-CONTRACT.md`](../../docs/ENGINE-CONTRACT.md)
**Scope:** Architecture and verification plan; no implementation is authorized by this document

## 1. Architectural decision

Permission transitions are control-plane operations owned by Rust. They are not chat messages and must never be forwarded to the harness as prompts.

The intended flow is:

```text
trusted principal input
        │
        ▼
Coven command parser ──► authorization + policy ──► capability resolution
        │                                            │
        │ denied                                     │ supported
        ▼                                            ▼
structured refusal                         transition coordinator
                                                     │
                                   ┌─────────────────┴────────────────┐
                                   ▼                                  ▼
                              live update                     relaunch + resume
                                   │                                  │
                                   └─────────────────┬────────────────┘
                                                     ▼
                                         verify effective mode
                                                     │
                                      audit event + UI state update
```

The TUI may render confirmation and status, but it does not decide whether elevation is allowed. TypeScript integrations remain thin clients of the same Rust-owned contract.

## 2. Relationship to existing architecture

### Current surfaces

- `crates/coven-cli/src/harness.rs` maps the typed launch-time `Permission` (`Full` or `ReadOnly`) through a per-harness `SandboxMapping` into harness arguments.
- `docs/reference/cli-run.md` documents launch-time `--permission <LEVEL>` behavior as harness dependent.
- The harness-capabilities specs propose stable capability descriptors and validation.
- The engine contract establishes a Rust control plane with thin harness adapters.

### Required convergence

This plan must not create a competing capability registry. The session-permission feature should consume the canonical harness capability descriptors proposed by `specs/coven-harness-capabilities/`.

The existing launch-time `Permission` plus `SandboxMapping` is a useful typed adapter mechanism, but it is insufficient as the running-session authority contract because:

- `Full` still maps to harness-specific semantics;
- argument expansion proves formatting, not semantic support at the detected harness version;
- it cannot express whether changes are live or require relaunch;
- it cannot distinguish support, policy denial, and unknown capability;
- it does not describe resume/revocation behavior or effective-state verification.

It should remain the adapter-level launch mechanism during migration, while principal-facing session state gains the richer typed contract below.

## 3. Core domain model

Illustrative Rust types (names are provisional):

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    Once,
    Session,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PermissionState {
    pub baseline: PermissionMode,
    pub effective: PermissionMode,
    pub pending: Option<PermissionGrant>,
    pub active_grant: Option<PermissionGrant>,
    pub capability: PermissionCapability,
    pub policy: PermissionPolicyDecision,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PermissionGrant {
    pub grant_id: String,
    pub scope: PermissionScope,
    pub granted_by: PrincipalId,
    pub granted_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub policy_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PermissionCapability {
    Supported {
        application: PermissionApplication,
        revocation: PermissionRevocation,
        source: CapabilitySource,
    },
    Unsupported { reason: String },
    Unknown { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionApplication {
    Live,
    RelaunchResume,
    LaunchOnly,
}
```

Rules:

- `effective == Full` only after adapter verification succeeds.
- A pending `Once` grant is not effective until a turn starts.
- `generation` increments on each accepted transition and protects against stale clients/races.
- Unknown capability is a denial for elevation.
- Strings sent to harnesses are derived adapter details, never accepted as proof of capability.

## 4. State machine

```text
                    request once + confirm
 DEFAULT ─────────────────────────────────────► FULL_PENDING_ONCE
    ▲                                                  │
    │ default / expiry / policy denial                 │ next turn starts
    │                                                  ▼
    ├────────────────────────────────────────── FULL_ACTIVE_ONCE
    │                                                  │
    │                                          turn completes/fails
    │                                                  │
    │        request session + confirm                 ▼
    └◄──────────────────────────────────────────── DEFAULT
    │
    └──────────────────────────────────────────► FULL_ACTIVE_SESSION
                                                    │
                         default / timeout / detach / workspace change /
                         policy change / unrecoverable adapter failure
                                                    │
                                                    ▼
                                                 DEFAULT
```

A transient `Transitioning` substate should be represented internally during live update or relaunch. It must not be advertised as `Full`.

### One-turn consumption

A one-turn grant is consumed only after all preconditions pass and the harness begins the turn in verified full mode. If request submission fails before that point, the state remains pending and the UI explains it. After the turn reaches a terminal state, Coven restores baseline before accepting the next principal turn.

### Revocation boundary

If the harness supports live revocation, apply and verify immediately. If it requires relaunch, Coven should:

1. prevent submission of new turns;
2. request a safe stop at the adapter-defined boundary;
3. relaunch/resume at baseline;
4. verify baseline;
5. update audit/UI state.

Emergency process termination remains available to the principal. UI must not promise that already-issued external side effects can be undone.

## 5. Trusted command input

### Parsing boundary

Slash commands are recognized only from an authenticated, principal-originated input event with an explicit source classification, for example:

```rust
pub enum InputOrigin {
    LocalInteractivePrincipal,
    AuthenticatedRemotePrincipal(PrincipalId),
    HarnessOutput,
    ToolOutput,
    RepositoryContent,
    Automation,
}
```

Only policy-authorized principal variants may create a permission request. The parser must never inspect arbitrary transcript text for executable commands.

### Command grammar

```text
permissions-command = "/permissions" [ SP action [ SP scope ] ]
action              = "show" | "full" | "default" | "help"
scope               = "--once" | "--session"
```

Constraints:

- `full` requires exactly one scope.
- `show`, `default`, and `help` reject scopes.
- unknown options fail without forwarding any text to the harness.
- command matching is exact after normal terminal line handling; no fuzzy correction for elevation.
- aliases are excluded from the first release.

### Confirmation token

Confirmation should operate on an opaque, short-lived server-side request:

```rust
pub struct PermissionConfirmationChallenge {
    pub challenge_id: String,
    pub session_id: SessionId,
    pub expected_generation: u64,
    pub requested_mode: PermissionMode,
    pub requested_scope: PermissionScope,
    pub expires_at: Timestamp,
    pub summary: PermissionImpactSummary,
}
```

The UI returns a decision referencing `challenge_id`; it does not resubmit a natural-language command. Challenges are single-use, expire quickly, bind to session/generation/principal, and are invalidated by workspace or policy changes.

## 6. Policy evaluation

The control plane computes:

```text
allowed = caller authorization
       ∩ deployment policy
       ∩ repository policy
       ∩ session constraints
       ∩ harness capability
       ∩ runtime preconditions
```

Later layers may only narrow authority.

Suggested decision shape:

```rust
pub enum PermissionPolicyDecision {
    Allowed {
        scopes: BTreeSet<PermissionScope>,
        max_session_ttl: Duration,
        retained_boundaries: Vec<BoundaryId>,
        requirements: Vec<PermissionRequirement>,
        policy_version: String,
    },
    Denied {
        code: PermissionDenialCode,
        safe_message: String,
        policy_version: String,
    },
}
```

Stable denial codes should include:

- `caller_not_authorized`
- `interactive_elevation_disabled`
- `scope_not_allowed`
- `harness_unsupported`
- `capability_unknown`
- `resume_unavailable`
- `audit_unavailable`
- `workspace_not_recoverable`
- `credentials_too_broad`
- `transition_in_progress`
- `session_not_attached`
- `policy_changed`

Messages must be safe for logs and must not disclose which secrets or credentials triggered a denial.

## 7. Harness capability contract

Extend the canonical harness capability descriptor rather than introducing ad hoc tests. A permission descriptor needs at least:

```json
{
  "permissions": {
    "modes": ["default", "full"],
    "full": {
      "application": "relaunch_resume",
      "revocation": "relaunch_resume",
      "adapter_value": "<internal, not principal-facing>",
      "verified_versions": ["..."]
    }
  }
}
```

The actual schema should follow the capability spec's typed Rust model and evidence rules. The descriptor must identify:

- semantic support for broad approval/sandbox mode;
- supported version range and evidence source;
- whether application is live, relaunch/resume, or launch-only;
- whether baseline restoration is live or requires relaunch;
- whether session continuity can be preserved;
- categories that remain constrained by the harness;
- verification mechanism after launch.

### Supported harnesses

Only Codex, Claude Code, and GitHub Copilot CLI are eligible while repository policy limits the supported set. Each adapter mapping requires an evidence packet against pinned/supported versions. Planning documents must not hard-code guessed flags.

### Verification

The adapter should verify effective application using the strongest available mechanism, in order:

1. harness-reported structured session configuration;
2. stable machine-readable startup metadata;
3. adapter-owned launch contract plus version-pinned conformance evidence;
4. otherwise capability is unknown and elevation is denied.

A process starting successfully is not sufficient evidence by itself if the harness may silently ignore a flag.

## 8. Transition coordinator

The coordinator is the sole mutation point and should expose operations conceptually like:

```rust
inspect_permissions(session_id, principal) -> PermissionState
prepare_permission_change(session_id, principal, request, generation)
    -> PermissionConfirmationChallenge
confirm_permission_change(challenge_id, principal, decision)
    -> PermissionTransitionResult
restore_default(session_id, principal, generation)
    -> PermissionTransitionResult
```

### Atomicity

A successful transition consists of:

1. authorization/policy/capability checks;
2. audit intent event (if required by policy);
3. adapter application or relaunch;
4. effective-mode verification;
5. durable state mutation with generation increment;
6. audit result event;
7. client notification.

On failure, Coven records a safe failure result and preserves/restores baseline. If baseline cannot be verified, mark the session permission state `indeterminate`, block new turns, and require restart or termination. Never optimistically report baseline or full.

### Concurrency

- Serialize transitions per session.
- Require expected generation from clients.
- Reject duplicate/late confirmations.
- A turn-start operation and a permission transition must share the same session lock or transactional boundary.
- Policy updates invalidate outstanding challenges.
- Disconnect invalidates unconfirmed challenges and revokes active session grants according to product policy.

## 9. Session persistence and lifecycle

The first release should not restore full grants after daemon restart. Persist enough information to explain that an elevation ended, but initialize the resumed session at baseline.

Revocation triggers:

- explicit `/permissions default`;
- one-turn completion;
- TTL expiration;
- detach/disconnect as selected by product policy;
- workspace or repository identity change;
- principal identity change;
- harness process replacement outside the coordinator;
- capability/version mismatch;
- policy update that no longer permits the grant;
- daemon restart;
- audit subsystem failure when policy requires audit.

If a session is resumed, permission state must be re-derived and verified, never trusted solely from stored UI metadata.

## 10. API and event model

Exact transport placement depends on daemon evolution, but local TUI and remote integrations should consume one structured contract.

Suggested response fields:

```json
{
  "session_id": "...",
  "baseline": "default",
  "effective": "full",
  "scope": "session",
  "expires_at": "...",
  "generation": 7,
  "application": "relaunch_resume",
  "capability_status": "supported",
  "policy_status": "allowed",
  "retained_boundaries": ["daemon_auth", "workspace_root", "audit_integrity"]
}
```

Suggested event kinds:

- `permission.baseline_established`
- `permission.change_requested`
- `permission.change_denied`
- `permission.change_cancelled`
- `permission.transition_started`
- `permission.transition_succeeded`
- `permission.transition_failed`
- `permission.revoked`
- `permission.expired`
- `permission.indeterminate`

Event payloads use IDs/codes and safe summaries, never prompt contents or credentials.

## 11. TUI implementation plan

Likely work areas include `crates/coven-cli/src/tui/` for input routing, confirmation modal, state subscription, status rendering, and accessibility. Before implementation, maintainers should map the precise current input path and ensure slash command extraction does not affect ordinary prompt submission.

Required UI components:

- exact command parser with completions/help;
- inspection view;
- trusted confirmation dialog;
- persistent elevated-state banner;
- countdown/expiration display for session grants;
- immediate restore control;
- structured refusal/error rendering;
- transition-in-progress state that blocks conflicting input;
- terminal-width and non-color accessibility tests.

No UI component may directly rewrite harness arguments or set its own effective mode.

## 12. Launch-time and non-interactive mode

Interactive slash commands and launch-time delegation share the same typed policy model but have distinct authorization flows.

A future launch surface may resemble:

```text
coven run <harness> --permission full --permission-scope session ...
```

For non-interactive use:

- authority must be explicit in process arguments or a trusted structured API;
- prompt text cannot request elevation;
- policy may require a named profile rather than confirmation;
- effective mode is included in startup output and provenance;
- legacy harness-specific permission strings should be deprecated only after migration compatibility is designed.

This plan does not finalize CLI syntax or deprecation timing.

## 13. Audit storage and AgentFS integration

Permission events should join the canonical session timeline/provenance stream, including AgentFS-backed sessions where available. Event ordering must distinguish request, transition, effective turn, and revocation.

Security properties:

- append-only or integrity-protected according to the existing provenance design;
- actor/session/workspace binding;
- safe failure when required audit storage is unavailable;
- no environment dumps or raw process arguments;
- retention follows session privacy policy;
- exported timelines clearly label full-mode intervals.

A useful review artifact is a timeline example:

```text
10:00 baseline default established
10:03 full/once requested by local principal
10:03 challenge confirmed
10:03 harness relaunched and full verified
10:04 turn 18 started under full
10:11 turn 18 completed
10:11 baseline restored and verified
```

## 14. Threat model and mitigations

| Threat | Mitigation |
|---|---|
| Prompt injection asks for elevation | Origin-typed trusted input; never parse transcript/model/tool text as commands. |
| Familiar invokes internal API/tool | Permission operations are not model tools; principal authorization is mandatory. |
| Clickjacking/ambiguous confirmation | Coven-owned modal with workspace, scope, boundaries, expiry; no model-supplied UI text. |
| Replay of confirmation | Opaque single-use challenge bound to principal/session/generation with short TTL. |
| Stale remote client overwrites state | Generation checks and per-session serialization. |
| Harness flag silently ignored | Versioned capability evidence plus effective-mode verification; unknown fails closed. |
| Relaunch loses or crosses session identity | Adapter verifies resume/session identity before marking success. |
| UI says default while process remains full | Verification on revocation; indeterminate state blocks new turns. |
| Session elevation survives unexpectedly | Revoke on lifecycle triggers; never persist across daemon restart initially. |
| Audit log leaks credentials | Structured allowlisted fields; no raw argv/env/prompt. |
| Policy TOCTOU | Bind challenge to policy version and re-evaluate at confirmation/application. |
| Symlink/workspace swap | Bind to canonical workspace identity and invalidate on change. |
| External side effects continue after revoke | Document safe-boundary semantics; block new work; terminate process for emergency stop. |

## 15. Test strategy

### Unit tests

- strict command grammar and rejection cases;
- origin authorization matrix;
- policy intersection and monotonic narrowing;
- challenge expiry, replay, wrong principal, wrong generation;
- state-machine transitions and one-turn consumption;
- TTL and lifecycle revocation;
- safe denial messages and audit redaction;
- capability unknown/unsupported behavior.

### Property/state-machine tests

Assert invariants across generated event sequences:

- only confirmed principal requests can lead to `effective == Full`;
- `effective == Full` implies supported capability and allowed policy;
- at most one active transition/grant per session;
- generation increases monotonically;
- restart initializes baseline;
- no turn starts while state is transitioning or indeterminate;
- a one-turn grant applies to at most one turn.

### Adapter contract tests

For every supported harness/version fixture:

- correct full-mode launch mapping;
- verification succeeds only when mode is effective;
- relaunch/resume preserves session identity where claimed;
- default restoration is verified;
- unsupported versions fail closed;
- extra arguments cannot override coordinator-owned permission arguments.

### Integration tests

- local TUI confirmation and cancellation;
- model output containing `/permissions full --session` has no effect;
- pasted repository text has no effect until explicitly entered and confirmed by principal;
- disconnect, timeout, workspace switch, crash, and daemon restart revoke appropriately;
- audit outage behavior follows policy;
- simultaneous turn submission and elevation request serialize correctly;
- terminal status is accessible without color.

### Security tests

- fuzz command parser and structured API;
- replay/cross-session challenge attempts;
- malicious harness output spoofing permission state;
- crafted capability manifests and version strings;
- symlink/workspace identity changes;
- log scanning for prompt, argv, environment, and credential leakage;
- unauthorized local/remote caller attempts.

### CI gates

Normal repository gates remain mandatory:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

Run npm build/tests if integration packages change.

## 16. Observability

Collect privacy-preserving counters/timings where telemetry policy permits:

- inspection requests;
- elevation requests by scope;
- confirmed/cancelled/denied transitions by stable reason code;
- transition and revocation duration;
- relaunch/resume failure counts;
- indeterminate-state count;
- automatic revocations by trigger;
- unsupported harness/version requests.

Do not collect prompts, command contents beyond the canonical permission operation, repository paths, usernames, or credential metadata merely for this feature.

## 17. Implementation slices

Each slice should be independently reviewable and keep user-visible elevation disabled until its prerequisites land.

### Slice A — Domain and capability schema

- Add typed permission state, scope, capability, denial, and transition types.
- Extend canonical harness capability descriptors with evidence.
- Add adapter fixtures and conformance tests.

### Slice B — Inspection path

- Establish baseline state at session start.
- Add control-plane inspection operation and audit event.
- Implement `/permissions show` and status rendering.

### Slice C — Transition coordinator

- Add authorization/policy evaluation, challenges, generation checks, state machine, and audit results.
- Keep full transitions behind a compile-time or experimental runtime gate.

### Slice D — One-turn adapter flow

- Implement apply/verify/restore for one supported harness first.
- Add full lifecycle and failure-path tests.
- Expand only after the first adapter passes security review.

### Slice E — Session scope and TTL

- Add persistent indicator, expiration, detach/workspace/policy revocation, and emergency restore.

### Slice F — Remaining supported harnesses and stable API

- Add evidence-backed adapters for the other supported harnesses.
- Stabilize structured daemon/integration APIs after conformance.
- Publish user/operator documentation and migration guidance.

## 18. Documentation deliverables

Before general availability:

- CLI reference for `/permissions` and launch-time modes;
- conceptual guide distinguishing familiar identity, capability, policy, and authority;
- operator policy reference with secure defaults;
- threat-model/security note;
- harness support/version matrix;
- recovery guide for indeterminate transitions;
- release notes that call the feature elevated permissions, not unlimited access.

## 19. Review checklist

### Authority/security

- [ ] Only authenticated principal input can request or confirm elevation.
- [ ] No model-visible tool can invoke the transition API.
- [ ] Full retains documented non-bypassable boundaries.
- [ ] Unknown capability and audit failure fail closed where required.
- [ ] Challenge replay and state races are covered.

### Harness adapters

- [ ] Each mapping has pinned-version evidence.
- [ ] Application and revocation behavior are truthful.
- [ ] Resume identity is verified.
- [ ] Extra arguments cannot countermand control-plane permission arguments.

### UX/accessibility

- [ ] Mode, scope, workspace, and expiration are conspicuous.
- [ ] Confirmation is Coven-owned and unambiguous.
- [ ] Restore is immediate to request and easy to discover.
- [ ] Color is not the only elevated-state signal.
- [ ] Refusals distinguish policy, support, and runtime failure.

### Reliability/provenance

- [ ] Every effective interval is represented in provenance.
- [ ] Crash/restart/detach behavior is deterministic.
- [ ] Indeterminate state blocks new turns.
- [ ] Logs exclude secrets, prompt bodies, raw environment, and unsafe argv.

## 20. Unresolved technical decisions

1. Which crate/module should own session permission state as daemon session APIs mature?
2. What existing session resume guarantees does each supported harness provide, and are they strong enough for relaunch transitions?
3. Can all adapter-owned permission arguments be placed after or otherwise protected from user `extra_args` overrides?
4. What is the canonical workspace identity across ordinary, worktree, and AgentFS-backed sessions?
5. Which audit backend is mandatory before one-turn elevation can ship?
6. Should expiration use wall clock plus monotonic timers, and how is daemon suspend handled?
7. Is emergency revocation process termination automatic after a grace period or explicitly principal-driven?
8. How does a remote client prove a confirmation was directly initiated by a principal rather than generated by an agent?
9. What exact capability-schema changes belong in the existing harness-capabilities work to avoid parallel registries?

## 21. Go/no-go gates

Interactive elevation remains disabled until all of these are satisfied:

- typed state and policy live in Rust;
- trusted input origins and principal authorization are enforced;
- capability evidence exists for at least one supported harness/version;
- apply and restore can both be verified;
- audit events and privacy review are complete;
- crash/restart/timeout/replay/race tests pass;
- elevated and indeterminate UI states are conspicuous;
- security reviewers approve the threat model;
- documentation never promises literal absence of limitations.
