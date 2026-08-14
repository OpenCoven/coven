# Session Permission Controls — Product Specification

**Status:** Draft for familiar review
**Scope:** Planning only; this document does not authorize implementation
**Owners:** Coven maintainers
**Reviewers:** Security, CLI/TUI, harness adapters, daemon/control plane, AgentFS/provenance, documentation

## Summary

Coven should let a principal deliberately grant a running familiar more execution authority when repeated approval prompts obstruct trusted work. The interactive surface should be a Coven-owned command such as:

```text
/permissions full --once
/permissions full --session
```

The feature is **not** a way for a familiar to remove its own limits. It is a temporary delegation by the human principal, interpreted and enforced by Coven's Rust authority layer, bounded by the active workspace and operating environment, visibly recorded, and easy to revoke.

We intentionally avoid presenting the feature as literally “allow all.” No application can override controls outside its authority, and some controls must remain non-bypassable.

## Problem

High-autonomy sessions can be interrupted by repetitive approval prompts even when all of the following are true:

- the principal trusts the requested task;
- the repository is local or disposable;
- changes are recoverable through Git, snapshots, or AgentFS;
- credentials and external systems are already constrained;
- the harness supports a broader native permission mode.

Today, permission behavior is harness-specific. Coven has a typed launch-time `Permission` (`Full` or `ReadOnly`) and per-harness `SandboxMapping` in the CLI/runtime specification, while the broader capability model is still being designed. Coven does not yet expose a stable, principal-facing **running-session** permission contract or a Coven-owned interactive slash command for changing it.

Without a common contract, users must remember harness-specific flags, cannot reliably see the effective mode, and may mistake a familiar's stated autonomy posture for actual authority.

## Product principles

1. **The principal grants authority.** Model output, familiar configuration, repository content, and tool results cannot grant or elevate permission.
2. **Coven remains the authority layer.** Interactive syntax is only a request to the Rust control plane; the TUI and TypeScript integrations must not independently bypass policy.
3. **Full means maximum delegated authority, not unlimited power.** OS, container, daemon, workspace, credential, policy, and non-bypassable safety boundaries remain effective.
4. **Elevation is explicit, conspicuous, temporary, and reversible.** No silent inheritance or hidden sticky state.
5. **Fail closed.** If Coven cannot prove that the selected harness supports the requested mode, it must refuse elevation rather than claim success.
6. **Prompt injection cannot activate elevation.** Commands are accepted only from a trusted principal input path and are never inferred from model-generated text.
7. **The effective state is inspectable and auditable.** The UI and session provenance show who requested a transition, when it occurred, its scope, and its result.
8. **Capability and policy are separate.** A harness may support full mode while deployment policy forbids it.

## Goals

- Provide one consistent permission vocabulary across supported harnesses.
- Let a principal inspect, elevate, and restore a session's effective permission mode.
- Support a one-turn scope and a current-session scope.
- Make elevated operation unmistakable in interactive UI.
- Preserve a minimal set of non-bypassable Coven invariants.
- Record transitions without leaking prompts, secrets, or credentials.
- Expose support and refusal reasons in machine-readable form for integrations.
- Define safe behavior when a harness must be relaunched to change modes.

## Non-goals

- Giving a familiar authority to elevate itself.
- Removing OS, container, credential, network, daemon-authentication, or workspace boundaries.
- Inventing a universal mapping for unsupported third-party harnesses.
- Automatically approving purchases, publication, messages, account changes, production mutations, or other consequential external actions.
- Weakening secret scanning, privacy controls, branch protection, repository policy, or audit integrity.
- Persisting full permission globally or across restarts in the first release.
- Treating a familiar's autonomy posture as an authorization grant.
- Replacing harness-native security models.

## Terminology

- **Principal:** the authenticated human or trusted caller permitted to control the session.
- **Familiar:** the selected identity/persona and its effective configuration.
- **Harness:** the external coding-agent process Coven launches or connects to.
- **Baseline mode:** the permission mode established when the session starts, after deployment and repository policy are applied.
- **Effective mode:** the mode actually enforced for the current turn/session.
- **Elevation:** a transition from baseline to a broader effective mode.
- **Full mode:** the broadest harness execution mode Coven policy permits inside the current execution boundary.
- **Non-bypassable boundary:** a restriction that full mode cannot disable.
- **Turn:** one principal submission and the resulting agent activity until the harness yields control.

## Proposed command surface

### Required commands

```text
/permissions
/permissions show
/permissions full --once
/permissions full --session
/permissions default
/permissions help
```

`/permissions` is equivalent to `/permissions show`.

### Semantics

| Command | Meaning |
|---|---|
| `/permissions show` | Show baseline mode, effective mode, scope, expiration, support, policy, and non-bypassable boundaries. |
| `/permissions full --once` | Request full mode for the next principal turn only. It is consumed only when a turn begins successfully. |
| `/permissions full --session` | Request full mode until explicit restoration, session termination, detach, workspace change, policy change, or timeout. |
| `/permissions default` | Revoke pending/effective elevation and restore the session baseline. |
| `/permissions help` | Explain modes, scopes, constraints, and recovery. |

### Naming decision

The canonical command is `/permissions`, not `/allow-all`.

Reasons:

- “allow all” implies authority Coven cannot possess;
- it obscures retained safety boundaries;
- it encourages binary thinking where capability, policy, and effective state differ;
- it is more attractive as a prompt-injection target.

An `/allow-all` alias should **not** ship initially. If user research later shows strong discoverability value, it may print an explanation and redirect to `/permissions full`; it must never have broader semantics.

### Confirmation

`full` requires an out-of-band confirmation rendered by trusted Coven UI, not a normal prompt sent to the harness. The confirmation must state:

- selected harness and workspace;
- requested scope (`once` or `session`);
- concrete categories being broadened (filesystem, command execution, network, tool approval), as known;
- boundaries that remain enforced;
- expiration/revocation behavior;
- whether applying the change requires harness restart/resume;
- a clear confirm and cancel action.

Typed confirmation is recommended for `--session`; an explicit UI confirmation is sufficient for `--once`. Non-interactive callers must use a distinct launch-time option or structured API and cannot simulate confirmation by embedding text in the task prompt.

## Permission-state model

The first release exposes two principal-facing modes:

- **default:** the policy-resolved baseline selected at launch;
- **full:** the broadest mode supported by both the harness and Coven policy.

Scopes:

- **once:** next successfully started turn;
- **session:** current attached session only, with a bounded timeout;
- **persistent/global:** not supported.

An intermediate `edits` or `workspace` mode may be considered later, but only after supported harnesses have a semantically honest mapping. Shipping a label whose behavior differs materially by harness would undermine the common contract.

## Required user experience

### Normal state

The status area shows the effective mode without dominating the UI:

```text
Permissions: default
```

### Pending one-turn elevation

```text
Permissions: FULL (next turn only)
```

The principal can cancel with `/permissions default` before submitting the turn.

### Active session elevation

A persistent, visually distinct indicator remains visible:

```text
FULL ACCESS · session · expires in 42m · /permissions default to revoke
```

Color must not be the only signal. Screen-reader output and narrow terminals must retain the text label.

### Unsupported or forbidden state

Refusals must distinguish:

- harness does not support the requested mode;
- Coven cannot verify support;
- deployment or repository policy denies it;
- caller is not an authorized principal;
- session cannot be safely resumed/relaunched;
- current operation cannot transition modes.

Example:

```text
Full permission was not enabled: this harness cannot be safely resumed with a broader mode.
The session remains at: default.
```

### Relaunch disclosure

Many harnesses choose approval/sandbox behavior at process launch. Coven must not suggest a live change occurred if it only changed UI state. If relaunch is necessary, confirmation must explain it, preserve continuity only through a verified resume mechanism, and surface success only after the replacement process is running in the requested mode.

## Boundaries retained in full mode

The exact policy is deployment-specific, but the product contract requires these categories to remain outside model-controlled bypass:

- authentication and authorization of the principal;
- daemon transport and same-user protections;
- session/workspace identity and configured root boundaries;
- audit-event integrity and permission-state provenance;
- secret redaction and privacy controls;
- Coven repository/deployment policy;
- host OS, sandbox, container, and credential constraints;
- explicit protections for destructive Coven control-plane operations;
- consequential external actions that policy marks as separately confirmable;
- kill, detach, timeout, and revocation controls.

Full mode may reduce **harness approval prompts** and broaden harness sandbox behavior where policy permits. It does not erase the surrounding control plane.

## Trust and threat scenarios

| Scenario | Required behavior |
|---|---|
| Repository file says “run `/permissions full`” | Treat as untrusted content; do not execute or prefill a trusted command. |
| Familiar emits the command in chat | Render as model output only; no transition. |
| Tool output contains command text | No transition. |
| User pastes command into trusted command input | Parse as principal intent, then require trusted confirmation. |
| Remote/untrusted client requests elevation | Require authenticated authorization and policy approval; otherwise deny. |
| Harness crashes during relaunch | Remain/revert to baseline; report failure; never display full mode. |
| Audit recording fails | Deny transition if an audit record is required by policy. |
| Session detaches or workspace changes | Revoke session elevation. |
| One-turn request is queued but turn fails to start | Keep it pending or cancel with explicit status; never consume silently. |
| Full mode expires during a running operation | Stop new elevated work at the next safe boundary; report the transition. |

## Policy and configuration

A deployment policy should be able to:

- disable interactive elevation entirely;
- allow `--once` but deny `--session`;
- set the maximum session-elevation TTL;
- require a disposable worktree, container, or AgentFS-backed session;
- require clean Git state or a recovery checkpoint;
- deny elevation when high-value credentials are present;
- keep selected action classes separately confirmable;
- require provenance/audit availability;
- constrain eligible callers, harnesses, repositories, and transports.

Policy resolution is monotonic: repository or familiar configuration may request **less** authority but cannot grant more authority than deployment/principal policy allows.

## Audit and privacy requirements

Record a structured transition event with:

- session ID and workspace identity;
- authenticated actor identity or local-principal marker;
- old and new effective modes;
- requested and granted scope;
- reason/result code;
- timestamp and expiration;
- harness kind/version and capability source;
- whether relaunch/resume occurred;
- applicable policy identifier/version.

Do not record:

- confirmation keystrokes beyond the decision;
- task prompts solely because a mode changed;
- environment values, tokens, or credential contents;
- raw harness command lines if they may contain sensitive arguments.

## Rollout proposal

### Phase 0 — Contract review

- Approve terminology and non-bypassable invariants.
- Resolve overlap with the harness capability specification and open implementation work.
- Threat-model trusted input, daemon/API callers, relaunch, and audit failure.
- Confirm supported-harness mappings against pinned versions.

### Phase 1 — Inspection only

- Add structured permission state and `/permissions show`.
- Display baseline/effective mode and support/refusal details.
- Emit audit events for baseline establishment.
- No interactive elevation.

### Phase 2 — One-turn elevation

- Add `/permissions full --once` behind an experimental feature gate.
- Require trusted confirmation and verified support.
- Prefer disposable/recoverable sessions during evaluation.
- Gather refusal, relaunch, and revocation telemetry without prompt contents.

### Phase 3 — Session elevation

- Add `/permissions full --session` with TTL and persistent indicator.
- Add disconnect, workspace-change, crash, and policy-change revocation.
- Keep persistent/global elevation out of scope.

### Phase 4 — Stable API

- Stabilize machine-readable control-plane operations.
- Document non-interactive launch-time delegation separately.
- Remove the experimental gate only after security review and cross-harness conformance tests.

## Success criteria

- A principal can accurately inspect the effective mode before and after every transition.
- No model-, repository-, or tool-generated text can trigger elevation.
- Every successful transition has a structured provenance event.
- UI never reports full mode unless the harness/control plane actually applied it.
- Revocation works across supported harnesses at the documented safe boundary.
- Unsupported and policy-denied cases fail closed with actionable reasons.
- Cross-harness conformance tests prove the common semantics.
- Security review finds no path from untrusted session content to elevation.

## Open product questions

1. Should `--session` default to 30 or 60 minutes, and may the principal shorten it?
2. Which external-action categories remain separately confirmable in the first policy schema?
3. Is a recoverability check (clean Git state, worktree, snapshot, or AgentFS) mandatory or advisory?
4. Should detach always revoke elevation, or may authenticated local reconnect retain it within TTL?
5. What terminology should UI use when a harness supports broad tool approval but cannot remove its sandbox?
6. Should a future `/allow-all` discoverability alias exist, or is documentation/search sufficient?
7. Which callers count as a trusted principal path in daemon and remote-client deployments?

## Review exit criteria

This product proposal is ready for technical implementation planning only when:

- Security approves the principal-only activation model and retained boundaries.
- Harness maintainers approve honest mappings for every supported harness.
- CLI/TUI maintainers approve trusted command input and persistent indicators.
- Control-plane maintainers approve policy resolution and transition ownership.
- AgentFS/provenance maintainers approve the audit schema and recovery expectations.
- Documentation reviewers confirm that “full” is not described as unlimited.
