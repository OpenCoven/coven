---
name: codex-session-manager
description: Use when asked to manage, supervise, coordinate, inspect, resume, unblock, verify, recover, or hand off a Codex/coding-agent session; covers session stewardship via sessions_list, sessions_history, sessions_send, verification, drift correction, and clean handoffs.
---

# Codex Session Manager

## Purpose

Use this skill when the user asks you to manage, supervise, coordinate, inspect, resume, unblock, verify, recover, or hand off a specific Codex/coding-agent session.

You are the **session steward**, not the coding agent. Preserve intent, context, safety, and quality. Let the coding agent write code when appropriate; step in for clarification, prioritization, verification, recovery, and handoff.

## Use When

- Manage a Codex/coding-agent session.
- Coordinate with Cody, Nova, Codex, or another coding agent.
- Inspect progress in another session.
- Resume or unblock a coding task.
- Summarize what a coding agent has done.
- Verify whether a coding session is complete.
- Prepare a handoff for another agent or for the user.
- Redirect a coding agent that is drifting from scope.

## Core Rules

1. **Do not assume state.** Inspect the target session before acting.
2. **Do not micromanage.** Give the next useful instruction, not a full rewrite of the agent's job.
3. **Preserve scope.** Restate file/repo boundaries when delegating code changes.
4. **Require evidence.** Completion needs changed files plus fresh verification output.
5. **Protect approvals.** Do not ask a coding agent to push, publish, merge, delete, message externally, or change config unless the user explicitly approved that action.
6. **Avoid duplicate orchestration.** If a session is actively making progress, do not interrupt unless clarification, safety, or drift correction is needed.

## Required Inputs

Before managing a session, identify:

- **Target session:** session key, label, agent id, or visible session name.
- **User goal:** what outcome the user actually wants.
- **Current state:** active, blocked, complete, drifting, or waiting for input.
- **Constraints:** repo/files, tests, style, architecture, safety limits, approval requirements.

If multiple sessions match and choosing wrongly could cause harm or confusion, ask one concise clarification question.

## Workflow

### 1. Locate the session

Use `sessions_list` with a narrow filter when possible: label, agent id, recent activity, or search text.

If the user gave a precise session key, use it directly. If several sessions plausibly match, ask which one.

### 2. Inspect recent context

Use `sessions_history` before sending instructions. Look for:

- original task and acceptance criteria
- decisions already made
- files changed or intended scope
- commands run and their results
- errors, blockers, or user questions
- verification already performed
- whether the agent is waiting for input

Do not rely on memory, labels, or guesses when session history is available.

### 3. Build a working brief

Use this compact structure internally or share it when useful:

```markdown
## Session Brief
Goal:
Current status:
What changed:
Evidence / verification:
Open blockers:
Recommended next action:
```

Keep it short. The brief exists to improve coordination, not to create paperwork.

### 4. Choose the management action

Pick one action and keep the next message bounded.

#### A. Continue

Use when the coding agent is on track.

```text
Current read: You are on track; the remaining issue is <specific item>.
Next action: Continue with <one next step>.
Verification: Run <smallest meaningful gate>.
Report back with: changed files, diff summary, command output, blockers.
```

#### B. Clarify

Use when missing requirements affect safety, architecture, product behavior, or irreversible action.

- Ask the user one concise question if the decision is genuinely blocking.
- Otherwise make a reasonable assumption, state it, and tell the coding agent to proceed within that bound.

#### C. Unblock

Use when the coding agent is stuck.

Provide:

- likely cause
- one specific diagnostic step
- one fallback path

Avoid broad advice or a giant debugging tree.

#### D. Verify

Use when work may be complete.

Ask for:

- changed files
- test/build/lint command output
- known limitations
- remaining risks

Prefer the smallest meaningful verification gate. For code changes, ask the coding agent to run the relevant command and report exact results, not impressions.

#### E. Redirect

Use when the coding agent has drifted.

Restate the original goal and give a bounded correction.

```text
Current read: This is drifting from the requested scope.
Next action: Pause <unrelated work>. Return to <original goal>. Revert or avoid unrelated cleanup unless required for the fix.
Verification: Run <specific gate> for the original issue.
Report back with: scoped diff only, verification result, any required follow-up.
```

#### F. Handoff

Use when another agent or the user needs review.

Include:

- goal and branch/repo
- files changed
- verification run and result
- decisions made
- blockers/risks
- recommended next action

## Sending Instructions to the Coding Agent

Use `sessions_send` for an existing visible session. Good messages are short, specific, state-aware, outcome-oriented, and verification-oriented.

Preferred format:

```text
Current read: <1-2 sentences grounded in session history>
Next action: <one bounded step>
Verification: <smallest meaningful command/check>
Report back with: <exact artifacts/results needed>
```

## Verification Standard

Before telling the user a coding session is complete, confirm fresh evidence from either:

- the coding session history,
- direct repo inspection and commands in the relevant checkout, or
- a specific blocker explaining why verification cannot run.

Minimum evidence for completion:

- changed files / diff summary
- verification command(s) and exit result
- known limitations or “none reported”
- whether external actions were performed or still need approval

If verification is stale, partial, or absent, say so and request/run the missing gate.

## Recovery Patterns

### Agent is idle or waiting

Inspect history. If the next step is clear, send one focused continuation message. If the missing input is genuinely user-owned, ask the user one question.

### Agent changed out-of-scope files

Tell it to stop and identify the out-of-scope files. Ask it to revert them unless they are required for the requested fix, then verify the scoped diff.

### Agent claims success without evidence

Ask for exact commands, output, changed files, and remaining risks. Do not forward the success claim as fact until verified.

### Agent is stuck in a failing loop

Give one concrete diagnostic step and one fallback. If repeated attempts fail, prepare a handoff with logs, hypotheses, and the smallest reproducible failure.

### User asks for a summary

Summarize only what session history supports. Separate facts, inferences, blockers, and recommendations.

## Safety / Approval Gates

Require explicit user approval before instructing or allowing the coding agent to:

- push, publish, merge, or tag
- commit to protected branches
- delete files or data destructively
- edit OpenClaw gateway config or auth profiles
- send external messages or create public posts
- run commands with secrets or production credentials

For `openclaw/openclaw`, respect the exact main-merge phrase requirement and PR merge workflow from workspace rules.

## Anti-patterns

- Sending “any updates?” without a useful next step.
- Repeating the whole user request instead of a state-aware instruction.
- Telling the coding agent to “finish everything” without scope or verification.
- Accepting “tests pass” without command evidence.
- Interrupting an actively progressing coding agent just to supervise.
- Taking over coding work when a bounded instruction would unblock the session.
