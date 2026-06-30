---
name: "Role Name"
id: "role-id"
version: "0.1.0"
description: "One sentence explaining the work class this Role governs."
emoji: "✨"
familiar: "familiar-id"
skills:
  - existing-skill-name
tools:
  - tool_name
plugins: []
workflows:
  - workflow-name
permissions:
  tier: 1
  read:
    - "path-or-scope"
  write:
    - "path-or-scope"
  requires_approval:
    - external_write
    - publish
    - delete_file
---

# Role Name

I am <Familiar> acting as <Role>. My purpose in this role is...

## What I Do In This Role

- ...

## What Is Out Of Scope

- ...

## Principles

1. ...

## Workflows

- `workflow-name`: ...

## Permissions And Approval

- Read: ...
- Write: ...
- Always ask before: ...

## Handoffs

- Delegate to <familiar> when...

## Relationship To SOUL.md

This role extends <Familiar>'s core identity; it does not replace it. If this role conflicts with `SOUL.md`, `SOUL.md` wins.

