---
summary: "How familiar identity is declared, resolved, and carried across sessions."
read_when:
  - Understanding why a familiar is more than a harness prompt
  - Designing a familiar that survives model or provider changes
  - Explaining how relationship affects familiar behavior
title: "Identity"
description: "Identity for OpenCoven familiars: declared familiar manifests, effective familiar resolution, relationships, memory profile, and governance."
---

Familiar identity is the layer that lets a named agent remain itself while its harness, model, tools, or client changes.

```text
familiar.yaml
  -> identity resolver
  -> effective familiar
  -> session
```

## Identity is not configuration

Configuration chooses commands, models, paths, and tools. Identity declares purpose, roles, principles, relationships, memory profile, and governance.

Coven resolves identity before launching a session so clients and harnesses receive an explicit effective familiar instead of a loose prompt string.

## What identity carries

A familiar identity should include:

- **Purpose** — why the familiar exists.
- **Roles** — the work it is suited to perform.
- **Principles** — behavioral constraints such as truth before confidence.
- **Relationships** — how it relates to its owner and other familiars.
- **Memory profile** — what continuity follows it.
- **Capability intent** — what kinds of hands it expects to use.
- **Governance** — autonomy, approval, and provenance rules.

## Soul, mind, hands

OpenCoven separates a familiar into three layers:

```text
Familiar
  Soul  -> purpose, roles, principles, relationships
  Mind  -> skills, workflows, memory profile
  Hands -> tools, harnesses, MCP, browser, desktop, GitHub
```

Hands can change without changing the familiar. Nova using GitHub and Nova using another work system should still resolve as Nova.

## Relationships matter

Relationship is architectural, not decorative.

An engineer familiar acting as a maintainer should not resolve the same governance posture as the same engineer familiar acting as an apprentice. The skills may be identical, but autonomy, explanation style, memory behavior, and approval gates can differ.

## Effective familiar

The identity resolver turns a declared familiar into an effective familiar:

```json
{
  "schemaVersion": "coven.effective_familiar.v1",
  "id": "nova",
  "displayName": "Nova",
  "roles": ["orchestrator", "maintainer-companion"],
  "principles": ["Truth before confidence."],
  "memoryProfile": {
    "profile": "continuity-curated"
  },
  "governance": {
    "autonomy": "supervised",
    "externalActions": "require_approval"
  }
}
```

The effective familiar is what a session consumes. The manifest is the declaration; the resolver is the authority that normalizes it.

## Governance

Capability intent is not a permission grant. It is resolved into policy metadata that the daemon, clients, approval gates, and repository rules can enforce.

This keeps identity expressive without turning it into a bypass around trust.
