---
name: linear-issue-management
description: Manage Linear issues safely through the GraphQL API, including team/project/label discovery, creating issues, updating issues, searching existing issues, handling mutually-exclusive labels, and using 1Password-backed Linear API auth without printing secrets. Use when filing, triaging, editing, or auditing Linear tickets such as CORE issues.
---

# Linear Issue Management

Use this skill when working with Linear issues through the API.

## Prime directive

Discover before writing. Never guess the team, project, state, assignee, or label IDs from memory when creating or updating a Linear issue.

Before any mutation:

1. Confirm the target workspace/team from the user request or current project context.
2. Query Linear for the exact team/project/label/state IDs.
3. Search for an existing issue to avoid duplicates.
4. Show the intended mutation summary if the action is externally visible or ambiguous.
5. Execute one focused GraphQL mutation.
6. Return the issue key + URL and record any important follow-up context.

## Safety rules

- Do not print, paste, log, or commit the Linear API token.
- Prefer 1Password-backed env injection or an existing secret manager. Do not write tokens to files.
- Treat issue creation, updates, comments, assignment changes, and state transitions as external state-changing actions. Get explicit user approval unless the user already gave a direct instruction in the current conversation.
- Use GraphQL variables. Do not interpolate untrusted user text into a GraphQL query string.
- Keep issue text factual. Do not file speculation as confirmed defects.
- Do not file in a team just because an issue key prefix looks familiar; confirm via API.

## Auth pattern

Use a Linear API key in `LINEAR_API_KEY`.

Preferred shape:

```bash
op run --env-file <linear-env-file> -- <command using LINEAR_API_KEY>
```

or, when the shell already has the token:

```bash
curl https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @payload.json
```

Never echo the token. If debugging auth, print only whether `LINEAR_API_KEY` is set, not its value.

## Workflow: create an issue

1. Read `references/graphql-recipes.md` for query/mutation templates.
2. Discover teams and choose the exact `teamId`.
3. Search existing issues by relevant keywords and repo/project names.
4. Discover labels/projects/states only after the team is known.
5. Resolve label conflicts before mutation.
6. Create the issue with title, description, `teamId`, optional `projectId`, optional `labelIds`, and optional priority.
7. Return the created issue identifier and URL.

## Label rules

- Linear can have grouped labels where child labels are mutually exclusive.
- If two requested labels conflict, choose the more precise/current one and mention the omitted label.
- Known OpenClaw Linear rule: `Infra` and `Improvement` are mutually exclusive grouped child labels. Do not apply both.

## Writing good issue bodies

Use this compact structure unless the user asks otherwise:

```markdown
## Problem
<what is wrong and who it affects>

## Evidence
- <specific file/log/error/link when available>

## Expected behavior
<desired end state>

## Proposed fix
<short implementation direction, if known>

## Acceptance criteria
- [ ] <verifiable outcome>
- [ ] <test or validation gate>
```

For follow-up implementation tickets, include exact repo paths, config keys, issue IDs, and commands when known.

## Updating/searching issues

- Search by issue key first if provided (`CORE-14`).
- Search by URL slug or title keywords when no key is available.
- Fetch current labels/state before replacing anything.
- Prefer additive updates unless the user explicitly asks to remove/replace labels, state, assignee, or project.

## Handoff to coding agents

When handing a Linear issue to Cody or another coding agent, include:

- Linear key + URL
- Repo/path scope
- Summary of the intended code change
- Required verification commands
- Constraints: preserve dirty work, no secret printing, do not touch unrelated files
- Any linked docs/memory references needed for context

## References

- `references/graphql-recipes.md` — GraphQL discovery, search, create, update, and comment templates.
