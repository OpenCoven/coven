---
name: familiar-board-stewardship
description: "Keep familiar-owned Coven board tasks current and actionable."
---

# Familiar Board Stewardship

Use when a Coven familiar starts work, finishes work, receives durable work,
finds follow-up work, or prepares a handoff.

## Default rule

The Coven board is the shared operational surface. Familiars should default to
checking their assigned board lane, keeping owned cards current, and creating or
routing cards when new durable work appears.

This skill is runtime-portable. It is not tied to OpenClaw, Cave, Codex, Claude
Code, Warp, or any single harness. A runtime may implement board operations
through an API, CLI, local file, MCP tool, or queued sync layer as long as it
preserves the same card semantics.

## Start-of-work check

1. List active board cards for the familiar's agent id and relevant project.
2. Prefer existing assigned cards over inventing new work.
3. Claim or mark the card active before doing substantial work when the runtime
   supports claims.
4. If the work is not represented on the board and is durable, create a card
   with clear acceptance criteria.

## During work

- Update the card when scope changes, blockers appear, or follow-up cards are
  created.
- Decompose vague parent cards into concrete children before dispatching or
  executing.
- Route work according to familiar lane:
  - Astra: strategy, business map, priorities, decisions, routing.
  - Charm: outward-facing language, outreach drafts, public narrative.
  - Sage: research, evidence, synthesis, legal/process background.
  - Cody: code implementation and verification.
  - Kitty: general execution support, cleanup, release readiness, operational
    sweeps.
  - Echo: continuity, retrospection, memory promotion, pattern review.
  - Nova: orchestration, system changes, board hygiene, cross-lane routing.

## Adding cards

Create a new card when work is:

- Durable beyond the current chat turn.
- Relevant to another familiar or future session.
- A discovered blocker, dependency, or follow-up.
- A recurring operating responsibility.

Each card should include:

- A specific title with a verb.
- Board/project namespace.
- Assignee or familiar when clear.
- Labels for lane and topic.
- Notes with acceptance criteria and routing context.
- Parent/child links when part of a larger effort.

## Completing or handing off

Before completion, release, or handoff:

1. Summarize what changed or what was learned.
2. Attach proof when applicable: command, test, screenshot, source, artifact, or
   reason verification was skipped.
3. List created child cards or follow-ups.
4. Leave the card unambiguous for the next familiar.

## Cadence

- At least once per active work session, check assigned board cards.
- During daily or retrospective sweeps, note stale assigned cards, blocked work,
  and missing cards.
- Weekly, each familiar with an active lane should either advance one owned
  card, update its blocker/status, or explicitly mark the lane quiet.

## Boundaries

- Do not perform external actions merely because a card exists; external sends,
  account changes, public posts, pushes, merges, and configuration patches still
  need the normal approval gates.
- Do not create noisy micro-cards for transient private thoughts.
- Do not mark work complete without evidence or a clear skipped-verification
  note.

## Canonical source

Canonical skill source:

```text
OpenCoven/coven/skills/familiar-board-stewardship/
```

Harnesses should consume this directory by symlink or package distribution
rather than copying it into OpenClaw-specific workspace skills.
