---
name: codeflow-maintainer
description: >
  OpenClaw maintainer PR workflow modes and skills. Maps to openclaw/maintainers
  PR_WORKFLOW.md phases (review-pr → prepare-pr → merge-pr). Provides structured
  modes with allowed/blocked actions, focus areas, checkpoint questions, and
  agent prompt injection for each phase.
version: 1.0.0
tags: [github, pr, workflow, maintainer, code-review, openclaw]
inheritable: true
---

# CodeFlow Maintainer Skill

## Source of Truth

**PR_WORKFLOW.md:** https://github.com/openclaw/maintainers/blob/main/.agents/skills/PR_WORKFLOW.md

This skill implements the workflow defined in `openclaw/maintainers`. All modes, checkpoints, and quality bars derive from that document.

## Working Rule

> Skills execute workflow. Maintainers provide judgment.
> Always pause between skills to evaluate technical direction, not just command success.

## Workflow Modes

### 1. Triage

**Purpose:** Assess whether PR is worth reviewing.

**Focus:**
- Is the problem statement clear?
- Does a realistic verification path exist?
- Is this PR worth our time?
- Are there duplicates?

**Recommendation:** Proceed to Review, Request Info, or Close.

**Agent prompt:**
```
[Mode: TRIAGE] Assess whether this PR is worth reviewing.
Focus on problem clarity, scope match, duplicate detection.
DO NOT suggest code changes, approvals, or merges.
```

### 2. Review (`review-pr`)

**Purpose:** Review only — produce findings and recommendation. DO NOT modify code.

**Quality bar:**
- Do not trust PR code by default
- Keep types strict (no `any`)
- Evaluate security impact and abuse paths
- Understand the system before changing it

**Finding severities:**
- `BLOCKER` — must fix before prepare
- `IMPORTANT` — must fix before prepare
- `MINOR` — should fix (optional)
- `NOTE` — informational only

**Artifacts:** `.local/review.md`, `.local/review.json`

**Checkpoint before advancing:**
1. What problem are they trying to solve?
2. What is the most optimal implementation?
3. Can we fix up everything?
4. Do we have any questions?

**Agent prompt:**
```
[Mode: REVIEW] Produce findings, DO NOT modify code.
Focus: correctness, security, tests, types, scope, hardening.
Severity: BLOCKER > IMPORTANT > MINOR > NOTE.
Recommendation: READY FOR PREPARE, NEEDS WORK, NEEDS DISCUSSION, or CLOSE.
```

### 3. Prepare (`prepare-pr`)

**Purpose:** Make PR merge-ready on its head branch.

**Order:** Rebase → Fix BLOCKER/IMPORTANT → Changelog → Gates → Push

**Gates:** `pnpm build`, `pnpm check`, `pnpm test`

**Rules:**
- Rebase onto main FIRST before any fixes
- Add changelog entry with `(#PR)` and `thanks @author`
- Push to PR head branch only, NEVER to main
- Use `scripts/committer` for commits

**Checkpoint before advancing:**
1. Most optimal implementation?
2. Properly scoped and reusing existing logic?
3. Properly typed and hardened?
4. Enough meaningful tests? (fake timers where appropriate)
5. Any security vulnerabilities introduced?
6. Follow-up refactors to defer?

**Agent prompt:**
```
[Mode: PREPARE] Make PR merge-ready. Rebase first, then fix, then gates.
Push to PR head branch only, never to main.
Gates: pnpm build, pnpm check, pnpm test.
```

### 4. Merge (`merge-pr`)

**Purpose:** Final verification and squash-merge.

**Go/no-go checklist:**
- All BLOCKER and IMPORTANT findings resolved
- Meaningful verification, low regression risk
- Changelog updated with PR # and thanks @author
- CI green, branch not behind main

**Merge method:** Squash with `Co-authored-by:` trailers for PR author + maintainer.

**Post-merge:** Verify PR reaches MERGED state. Post comment with merge SHA.

**Agent prompt:**
```
[Mode: MERGE] All gates must be green. Squash merge with Co-authored-by.
Verify MERGED state. Post comment with SHA.
DO NOT modify code — go back to Prepare if needed.
```

### 5. Post-Merge

**Purpose:** Cleanup, attribution, follow-ups.

**Tasks:**
- Delete branch
- Update clawtributors for new contributors (`bun scripts/update-clawtributors.ts`)
- Create follow-up issues for deferred refactors
- Clean up worktree

## Mode Transitions

```
Triage → Review → Prepare → Merge → Post-Merge
         ↑___________↓  (back to Prepare if changes needed)
```

- Each transition requires checkpoint questions answered
- Modes cannot be skipped (triage → prepare is not valid)
- `read-only` overrides everything when active

## Related Skills

### PR Agent Orchestrator

For concrete workflow execution, see the **pr-agent** skill:

- **Location:** `~/.openclaw/workspace/skills/pr-agent/`
- **Purpose:** Orchestrates the three-phase workflow with script-first execution
- **Source:** https://github.com/openclaw/maintainers/tree/main/.agents/skills/pr-agent

The pr-agent skill provides:
- Automatic artifact validation (`.local/*.json`, `.local/*.md`)
- Script wrapper invocation (`scripts/pr-review`, `scripts/pr-prepare`, `scripts/pr-merge`)
- Checkpoint enforcement between phases
- Escalation trigger detection

### Inheritable Skills

This skill is designed to be inherited by other agents:

- **review-pr:** https://github.com/openclaw/maintainers/tree/main/.agents/skills/review-pr
- **prepare-pr:** https://github.com/openclaw/maintainers/tree/main/.agents/skills/prepare-pr
- **merge-pr:** https://github.com/openclaw/maintainers/tree/main/.agents/skills/merge-pr

Agents inheriting this skill should:
1. Follow the mode progression strictly
2. Use the checkpoint questions before advancing
3. Inject the mode-specific prompt into their system context
4. Respect the quality bar (types, security, tests, scope)
