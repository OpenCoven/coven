---
summary: "Familiars are OpenCoven's persistent named agents. Coven runs them. Comux and OpenMeow show them. They remember."
read_when:
  - Introducing OpenCoven's product layer to a stakeholder
  - Deciding whether to design a familiar or just run a harness
title: "Familiars"
---

**Familiars** are OpenCoven's product layer above the Coven runtime: persistent named agents with memory, tools, identity, roles, and continuity. A familiar is not a faceless bot. It has a name, a purpose, a memory, a toolset, a voice, a role, and a place in a larger workflow.

> OpenCoven turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to you.

<Columns>
  <Card title="What is a familiar?" href="/familiars/what-is-a-familiar" icon="sparkles">
    The product concept layered above the technical [harness](/harnesses) concept.
  </Card>
  <Card title="Naming and voice" href="/familiars/naming-and-voice" icon="quote">
    Names, voices, and the brand promise of personal-not-pretending-human agents.
  </Card>
  <Card title="Roles" href="/familiars/roles" icon="users">
    The roles a familiar can take inside a workflow.
  </Card>
</Columns>

## Familiar vs. harness

```mermaid
flowchart LR
  Familiar[Familiar: name, memory, voice, role] -->|runs on| Harness[Harness: Codex / Claude / future]
  Harness -->|inside| Coven[Coven daemon]
```

A familiar can swap harnesses. A harness does not know which familiar it is serving. Coven is the substrate that keeps the boundary honest.

## What every familiar has

- **A name and voice** — see [Naming and voice](/familiars/naming-and-voice).
- **Memory** — working, persistent, episodic, semantic. See [Memory](/memory).
- **Tools** — exec, apply-patch, web-fetch, plus skills and plugins.
- **Identity** — a stable handle across sessions, devices, and harness swaps.
- **A role** — the slot a familiar occupies in a workflow.
- **A place in the circle** — multi-familiar workflows let specialists coordinate.

## How a familiar lives across sessions

```mermaid
sequenceDiagram
  participant U as User
  participant F as Familiar identity
  participant M as Memory store
  participant S as Session 1 (Codex)
  participant S2 as Session 2 (Claude Code)

  U->>F: "Run with Aria"
  F->>M: load persistent memory
  F->>S: launch Codex with persona prompt
  S-->>M: persist episodic events
  F->>S2: later, launch Claude Code with same identity
  M-->>S2: replay relevant memory
```

The familiar identity outlives any single session. Coven only requires that **each session** pins a project root; the familiar layer is free to span multiple sessions and harnesses.

## Multi-familiar

<Columns>
  <Card title="Handoff" href="/familiars/handoff" icon="git-branch">
    Phase 1 — explicit transfer of task + context between familiars.
  </Card>
  <Card title="Orchestration" href="/familiars/orchestration" icon="route">
    Phase 2 — capability discovery, router, load balancing.
  </Card>
  <Card title="Parallel lanes" href="/familiars/parallel-lanes" icon="columns">
    Specialist lanes that work the same task in parallel.
  </Card>
</Columns>

## Related

- [Memory overview](/memory)
- [Sessions](/sessions)
- [Rituals](/rituals) — archive, summon, sacrifice
- [Brand](/reference/brand) — naming, voice, mark
