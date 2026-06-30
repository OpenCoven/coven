---
name: "OpenCoven Role Creation Process"
description: "Create, validate, and activate ROLE.md specs for familiars with low-lift agentic guidance."
version: "0.1.0"
kind: "role-authoring"
tags:
  - opencoven
  - roles
  - familiars
  - covencave
---

# OpenCoven Role Creation Process

Use this skill when a CovenCave user wants to create, review, revise, or standardize Roles for familiars.

A Role is not a plugin list. A Role is the familiar's domain-specific way of being while doing a class of work. It bundles identity context, skills, tools, workflows, plugins, and permission declarations.

## Low-Lift Agentic Flow

1. **Read the familiar.**
   - Load the familiar's `SOUL.md`, `IDENTITY.md`, local skill doctrine, existing Roles, and any user-provided purpose.
   - If the familiar has no workspace yet, create or select one before writing Roles.

2. **Decide whether a Role is actually needed.**
   - Use a Role for a domain-specific frame plus workflows and permissions.
   - Use a Skill for one reusable procedure.
   - Use a Workflow for one multi-step path inside an existing Role.
   - Use a Plugin/tool grant for capability without identity context.
   - Leave core identity in `SOUL.md`.

3. **Draft the Role.**
   - Use `templates/ROLE.md` as the starting point.
   - Keep the body specific enough to change behavior.
   - Do not duplicate the familiar's `SOUL.md`.

4. **Draft workflows.**
   - Every item listed in the Role's `workflows:` frontmatter should have a matching file under `workflows/<workflow-id>.md`.
   - Use `templates/workflow.md`.

5. **Choose canonical placement.**
   - Familiar-specific source of truth: `~/.coven/roles/familiars/<familiar>/<role-id>/`.
   - Shared global source of truth: `~/.coven/roles/global/<role-id>/`.
   - Symlink into the harness-visible familiar workspace: `<familiar-workspace>/roles/<role-id>`.

6. **Set activation deliberately.**
   - Primary domain Roles may start active after review.
   - Narrow task Roles should start inactive.
   - High-authority Roles should start inactive.

7. **Validate.**
   - Run `scripts/validate-roles.mjs`.
   - Confirm required frontmatter, listed workflows, symlinks, and `SOUL.md` relationship text.

8. **Report cleanly.**
   - Roles created.
   - Roles activated.
   - Source files read.
   - Validation output.
   - Open questions or permission risks.

## Required Role Contract

Each `ROLE.md` must include:

- `name`
- `id`
- `version`
- `description`
- `familiar`
- `skills`
- `tools`
- `plugins`
- `workflows`
- `permissions`
- a body with purpose, scope, principles, workflows, permissions, handoffs, and relationship to `SOUL.md`.

## Anti-Patterns

Avoid:

- Role as duplicated `SOUL.md`.
- Role with only frontmatter and no identity context body.
- One Role per tiny task.
- Broad tool access just in case.
- Personality/vibe rules that belong in core identity.
- Assuming Role permission declarations enforce themselves.

## Recommended Defaults

- Authority layer wins over Role declarations.
- Cave active state is UI/session state.
- Role manifest is intent and context.
- `SOUL.md` wins over every Role.
- Keep Kitty-like generalists role-light unless a role adds concrete behavior.

