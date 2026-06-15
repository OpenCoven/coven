# Familiar Identity Model Design

Date: 2026-06-15
Status: Draft architecture canon
Owner: OpenCoven / Coven daemon
Source: Valentina's "OpenCoven Should Not Treat Identity As Configuration" research report

## Executive Summary

OpenCoven should treat familiar identity as a first-class runtime object, not as prompt text or client configuration. Capabilities, skills, tools, MCP servers, plugins, harness adapters, and models are increasingly converging across the agent ecosystem; durable identity is the layer OpenCoven can own.

The core architectural move is:

```text
Familiar manifest
  -> identity resolver
  -> effective familiar
  -> session
```

This is deeper than the common framework pattern:

```text
Agent definition
  -> session
```

The resolver becomes the authority that turns a declared familiar into the concrete identity, memory profile, relationship posture, capability intent, governance constraints, and harness-specific prompt material used by a running session.

## Current State

Coven already has the beginning of this idea:

- `crates/coven-cli/src/harness.rs` defines `FamiliarContext`.
- `coven run --familiar` can inject a concise identity preamble into harness prompts.
- `crates/coven-cli/src/api.rs` accepts `familiar_id` and `caller_familiar_id` for daemon launch requests.
- `crates/coven-cli/src/eval_loop.rs` resolves familiar workspace paths from `~/.coven/familiars.toml`.
- `docs/familiars/index.md` states that a familiar outlives a single harness session.
- `docs/familiars/identity.md` is still a stub and should become the public explanation once the model lands.

That current model is useful, but too shallow. It mostly resolves name and role into a text preamble. The next model should resolve a durable familiar declaration into a typed effective familiar.

## Design Principle

Identity is not configuration.

Configuration answers:

- which command to run;
- which model to request;
- which tools are available;
- which working directory to use.

Identity answers:

- who this familiar is;
- what relationship it has to the owner and other familiars;
- what principles constrain its behavior;
- what memory profile follows it;
- what autonomy and governance posture applies;
- how it should remain itself across model, provider, harness, runtime, plugin, and client changes.

## Manifest Shape

Add a first-class `familiar.yaml` manifest format. The manifest declares stable identity intent. It does not directly grant permissions.

```yaml
schema_version: coven.familiar.v1
id: nova
display_name: Nova

identity:
  purpose: Trusted companion for building, organizing, remembering, and moving OpenCoven forward.
  roles:
    - orchestrator
    - maintainer-companion
  principles:
    - Truth before confidence.
    - Prefer local execution when it protects privacy and agency.
    - Explain tradeoffs without pretending uncertainty away.
  relationships:
    owner:
      kind: sovereign-source
      display_name: Valentina
    team:
      - familiar_id: sage
        kind: peer-researcher
      - familiar_id: cody
        kind: implementation-partner

memory:
  profile: continuity-curated
  scopes:
    - long_term
    - daily_notes
    - project_context
  retention:
    default: curated
    secrets: never_persist_without_explicit_request

skills:
  required:
    - systematic-debugging
    - verification-before-completion
  preferred:
    - writing-plans
    - requesting-code-review

workflows:
  defaults:
    - code-review
    - research-synthesis
    - release-triage

capability_intent:
  filesystem: local_workspace
  github: maintainer_assist
  browser: verify_when_needed
  desktop: explicit_consent_only

governance:
  autonomy: supervised
  external_actions: require_approval
  merge_rules:
    protected_branches:
      - main
    require_verification: true
  provenance:
    coauthor_when_relevant: true
```

## Effective Familiar Shape

The resolver emits a normalized `EffectiveFamiliar`. This is the object sessions consume.

```json
{
  "schemaVersion": "coven.effective_familiar.v1",
  "id": "nova",
  "displayName": "Nova",
  "identityPreamble": "[Identity: You are Nova...]",
  "roles": ["orchestrator", "maintainer-companion"],
  "principles": ["Truth before confidence."],
  "relationships": {
    "owner": {
      "kind": "sovereign-source",
      "displayName": "Valentina"
    }
  },
  "memoryProfile": {
    "profile": "continuity-curated",
    "scopes": ["long_term", "daily_notes", "project_context"]
  },
  "skills": {
    "required": ["systematic-debugging", "verification-before-completion"],
    "preferred": ["writing-plans", "requesting-code-review"]
  },
  "capabilityPolicy": {
    "filesystem": "local_workspace",
    "github": "maintainer_assist",
    "browser": "verify_when_needed",
    "desktop": "explicit_consent_only"
  },
  "governance": {
    "autonomy": "supervised",
    "externalActions": "require_approval"
  },
  "provenance": {
    "manifestPath": "~/.coven/familiars/nova/familiar.yaml",
    "resolvedAt": "2026-06-15T00:00:00Z"
  }
}
```

## Resolver Responsibilities

The identity resolver is responsible for deterministic normalization, not model behavior.

It should:

1. Load the declared familiar manifest.
2. Validate schema version and required identity fields.
3. Merge user-level defaults, project-level overrides, and session request overrides using explicit precedence.
4. Resolve relationships to known familiar IDs where possible.
5. Resolve memory profile into allowed memory scopes and storage roots.
6. Resolve skill references into known local or installed skills.
7. Resolve capability intent into policy, not raw tool grants.
8. Attach governance gates that downstream launch paths must respect.
9. Render harness-specific identity material without losing the typed source object.

It should not:

- directly run tools;
- silently widen permissions;
- mutate memory;
- claim relationship state that is not declared or inferred by an explicit rule;
- hide unsupported fields from clients.

## Precedence Model

Use a simple, inspectable precedence chain:

```text
built-in schema defaults
  < user familiar manifest
  < project familiar overlay
  < workflow invocation intent
  < one-shot session override
```

Every override must be reflected in `provenance` so clients can explain why a familiar behaved a certain way.

## Relationship Is Architectural

Relationship changes behavior without changing skills.

Example:

```yaml
id: forge
identity:
  roles: [engineer]
  relationships:
    owner:
      kind: maintainer
```

and:

```yaml
id: forge
identity:
  roles: [engineer]
  relationships:
    owner:
      kind: apprentice
```

can use the same harness and skills, but should resolve different defaults for autonomy, explanation style, memory write behavior, and approval gates.

This makes relationship part of runtime policy, not decorative copy.

## Soul, Mind, Hands

Use this mental model in docs and architecture reviews:

```text
Familiar
  Soul  -> purpose, role, principles, relationship
  Mind  -> skills, workflows, memory profile
  Hands -> tools, harnesses, MCP, browser, desktop, GitHub
```

Hands are swappable. Soul is not.

Nova with GitHub and Nova with Jira should still resolve as Nova.

## Governance Boundary

Manifest `capability_intent` is intentionally not a permission grant. It tells the resolver what the familiar expects. The daemon, client, user approvals, worktree protocol, and repository rules remain the enforcement layers.

That distinction keeps familiar identity expressive without making it a bypass around trust gates.

## Relationship To Ward And Familiar Contract

The Familiar Contract defines what a familiar is and what it owes.

Ward is the runtime enforcement layer for where identity can extend and under what conditions.

The identity resolver sits between them:

```text
Familiar Contract
  -> familiar.yaml
  -> identity resolver
  -> EffectiveFamiliar
  -> Ward / daemon / harness launch
  -> session
```

The resolver should not replace Contract or Ward. It makes their inputs concrete at session time.

## Product Implications

If this lands, OpenCoven can credibly claim:

- familiars survive model and harness swaps;
- identity can be versioned, diffed, reviewed, and rolled back;
- relationships can influence permissions and autonomy;
- memory scope is part of identity, not a random tool setting;
- multi-familiar orchestration can route to named entities instead of anonymous workers;
- clients can render why a familiar has a capability or governance gate.

## Initial Milestones

1. Add `schemas/familiar/coven.familiar.v1.schema.json`.
2. Add typed Rust structs for declared and effective familiars.
3. Implement `resolve_familiar(coven_home, project_root, request)` in the Coven CLI/daemon crate.
4. Replace the current shallow `FamiliarContext` construction with resolver output.
5. Add `coven familiars resolve <id> --json` for inspection.
6. Update `docs/familiars/identity.md` from stub to public docs.
7. Teach Cave and CastCodes to display effective familiar identity provenance when launching sessions.

## Non-Goals

- Do not build a broad plugin marketplace in this work.
- Do not grant tools directly from `familiar.yaml`.
- Do not make relationship fields anthropomorphic claims of consciousness.
- Do not require every harness to support system prompts.
- Do not migrate every existing familiar at once.

## Open Questions

1. Should `familiar.yaml` live at `~/.coven/familiars/<id>/familiar.yaml`, inside project `.coven/familiars/`, or both?
2. Should relationship kinds be an enum in v1, or freeform strings with known recommendations?
3. Should skill references be resolved by name only, or include source/package provenance?
4. How much of `EffectiveFamiliar` belongs in daemon API responses by default?
5. Should `Familiar Contract` be imported as a schema dependency or remain a linked conceptual authority?
