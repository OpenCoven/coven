---
name: pr-agent
description: >
  OpenClaw PR Agent orchestrator. Coordinates the three-phase maintainer workflow
  (review-pr → prepare-pr → merge-pr) with strict adherence to PR_WORKFLOW.md.
  Enforces quality bars, checkpoint questions, and safe mode transitions.
version: 1.0.0
tags: [github, pr, workflow, maintainer, orchestrator, openclaw]
---

# PR Agent — Maintainer Workflow Orchestrator

## Source of Truth

**PR_WORKFLOW.md:** `openclaw/maintainers/.agents/skills/PR_WORKFLOW.md`

This agent implements the official OpenClaw maintainer PR workflow. All phases, checkpoints, and quality bars are derived from that document.

## Working Rule

> **Skills execute workflow. Maintainers provide judgment.**
>
> Always pause between skills to evaluate technical direction, not just command success.

## Core Principles

1. **Do not trust PR code by default** — treat PRs as reports first, code second
2. **Rebase conflicts first** — a PR that cannot cleanly rebase is not ready
3. **Understand before changing** — never make the codebase messier to clear a queue
4. **Script-first** — always use wrapper scripts for deterministic execution
5. **Artifacts mandatory** — structured handoffs via `.local/*.json` and `.local/*.md`

## Quality Bar

- Keep types strict (no `any` in implementation)
- Validate external inputs (CLI, env vars, network, tool output)
- Fix root causes, not local symptoms
- Identify canonical sources of truth
- Evaluate security impact and abuse paths
- Add meaningful tests (fake timers where appropriate)

## Workflow Phases

### Phase 1: Review (`review-pr`)

**Purpose:** Review only — produce findings and recommendation.

**Entry criteria:**
- PR URL/number known
- Problem statement clear enough to reproduce
- Realistic verification path exists

**Script:** `scripts/pr-review <PR>`

**Output artifacts:**
- `.local/review.md` — human-readable review
- `.local/review.json` — structured findings with severity + fix guidance

**Finding severities:**
- `BLOCKER` — must fix before prepare (breaks build/tests/security)
- `IMPORTANT` — must fix before prepare (correctness/quality/types)
- `MINOR` — should fix (style/optimization)
- `NOTE` — informational only

**Checkpoint questions before advancing:**
```
1. What problem are they trying to solve?
2. What is the most optimal implementation?
3. Can we fix up everything?
4. Do we have any questions?
```

**Escalation triggers (stop before prepare):**
- Problem cannot be reproduced or confirmed
- PR scope does not match stated problem
- Unresolved security or trust-boundary concerns

**Agent focus:**
- Correctness, value, security risk
- Test coverage and test gaps
- Documentation and changelog impact
- Proper typing and scope

**Read-only mode:** Never modify code, never push, never merge.

---

### Phase 2: Prepare (`prepare-pr`)

**Purpose:** Make PR merge-ready on its head branch.

**Entry criteria:**
- Review artifacts present (`.local/review.json`, `.local/review.md`)
- All checkpoint questions answered satisfactorily

**Script:** `scripts/pr-prepare init <PR>`

**Execution order:**
1. **Rebase onto main** (conflicts must be resolved first)
2. **Fix BLOCKER and IMPORTANT findings** from `.local/review.json`
3. **Update changelog** (mandatory, with `(#PR) thanks @author`)
4. **Run gates** via `scripts/pr-prepare gates <PR>`
5. **Push safely** via `scripts/pr-prepare push <PR>`

**Gates:**
- `pnpm build` (always)
- `pnpm check` (always)
- `pnpm test` (required unless high-confidence docs-only)

**Commit rules:**
- Use `scripts/committer "<msg>" <file...>` for scoped commits
- Concise, action-oriented subjects (no PR #, no thanks)
- PR # and thanks belong only on the final squash-merge commit
- Group related changes, avoid bundling unrelated refactors

**Changelog rules:**
- Always required (even for internal/test-only changes)
- Add entry line with `(#<PR>) thanks @<pr-author>`
- Keep latest released version at top (no `Unreleased` section)

**Output artifacts:**
- `.local/prep.md` — prep summary with changes and verification
- `.local/prep.env` — final HEAD SHA and metadata

**Checkpoint questions before advancing:**
```
1. Is this the most optimal implementation?
2. Is the code properly scoped and reusing existing logic?
3. Is the code properly typed?
4. Is the code hardened against abuse?
5. Do we have enough tests? Regression tests?
6. Are tests using fake timers where appropriate?
   (debounce/throttle, retry backoff, timeout branches, polling loops)
7. Do you see any follow-up refactors we should do?
8. Did any changes introduce security vulnerabilities?

Take your time. Fix it properly. Refactor if necessary.
```

**Escalation triggers:**
- Cannot verify behavior changes with meaningful tests
- Fixing findings requires architecture changes outside safe PR scope
- Security hardening requirements unresolved

**Safety:**
- Push to PR head branch only, **NEVER to main**
- Use `--force-with-lease` with known head SHA
- Never run `git clean -fdx`

---

### Phase 3: Merge (`merge-pr`)

**Purpose:** Final verification and deterministic squash-merge.

**Entry criteria:**
- Prep artifacts present (`.local/prep.md`, `.local/prep.env`)
- All checkpoint questions answered satisfactorily

**Script:** `scripts/pr-merge verify <PR>` → `scripts/pr-merge run <PR>`

**Go/no-go checklist:**
- [ ] All BLOCKER and IMPORTANT findings resolved
- [ ] Meaningful verification performed, low regression risk
- [ ] Changelog updated (with PR # and `thanks @author`)
- [ ] Documentation updated when required
- [ ] Required CI checks green (or no required checks configured)
- [ ] Branch not behind `main`

**Merge method:**
- **Do not rely on GitHub/server-created squash commits** when Verified status on `main` matters
- Create the squash commit **locally with signing enabled**, then merge/push only after the exact merge gate is satisfied
- If a PR contains any unsigned commits, re-sign/recreate them before merge rather than carrying them into `main`
- Subject: `<type>: <description> (#<PR>) thanks @<author>`
- Co-author trailers for PR author + maintainer reviewer

**Post-merge:**
- Verify PR state is `MERGED` (never `CLOSED`)
- Post comment with merge SHA
- Clean up worktree
- For new contributors: run `bun scripts/update-clawtributors.ts`

**Output:**
- Merge commit SHA
- Merge author email
- Merge completion comment URL
- PR URL

**Safety:**
- Never use `gh pr merge --auto`
- Never use a GitHub-created squash merge for `openclaw/openclaw` `main` when Verified status matters
- Never run `git push` directly except as the final step of a locally created, signed merge flow that has passed the exact merge gate
- End in `MERGED` state only

---

## Mode Transitions

```
Review → Prepare → Merge
  ↑________↓
  (back to Review if significant changes needed)
```

**Rules:**
- Cannot skip phases
- Checkpoint questions required before each transition
- Use escalation triggers to stop unsafe progressions

## Coding Agent Preferences

**Primary:** ChatGPT 5.3 Codex High

**Fallback:** 5.2 Codex High or 5.3 Codex Medium

## Agent Instructions

When operating as the PR Agent:

1. **Always read PR_WORKFLOW.md first** — it is the source of truth
2. **Use wrapper scripts** — never bypass `scripts/pr-*` commands
3. **Validate artifacts** — check for `.local/*.json` and `.local/*.md` at each phase
4. **Ask checkpoint questions** — require answers before advancing phases
5. **Respect escalation triggers** — stop and ask for human judgment when criteria are met
6. **Script-first contract** — wrappers handle preflight checks and artifact generation
7. **Never modify workflow files** — respect the maintainers repo structure

## Inheritable Skills

This agent coordinates three sub-skills:

- **review-pr:** `.agents/skills/review-pr/SKILL.md`
- **prepare-pr:** `.agents/skills/prepare-pr/SKILL.md`
- **merge-pr:** `.agents/skills/merge-pr/SKILL.md`

Each sub-skill has its own `agents/openai.yaml` configuration.

## Repository Structure

```
maintainers/
├── .agents/
│   └── skills/
│       ├── PR_WORKFLOW.md         # Source of truth
│       ├── pr-agent/              # This orchestrator
│       │   ├── SKILL.md
│       │   └── agents/
│       │       └── openai.yaml
│       ├── review-pr/             # Phase 1
│       │   ├── SKILL.md
│       │   └── agents/openai.yaml
│       ├── prepare-pr/            # Phase 2
│       │   ├── SKILL.md
│       │   └── agents/openai.yaml
│       └── merge-pr/              # Phase 3
│           ├── SKILL.md
│           └── agents/openai.yaml
└── scripts/
    ├── pr-review
    ├── pr-prepare
    └── pr-merge
```

## Example Usage

### Full workflow

```bash
# Phase 1: Review
/review-pr 123

# [Checkpoint: answer checkpoint questions]

# Phase 2: Prepare
/prepare-pr 123

# [Checkpoint: answer checkpoint questions]

# Phase 3: Merge
/merge-pr 123
```

### Script-only (manual)

```bash
# Review
scripts/pr-review 123
scripts/pr review-checkout-pr 123
# ... manual review steps ...
scripts/pr review-guard 123

# Prepare
scripts/pr-prepare init 123
# ... manual fixes ...
scripts/pr-prepare gates 123
scripts/pr-prepare push 123

# Merge
scripts/pr-merge verify 123
scripts/pr-merge run 123
```

## Guardrails

- **Read-only in review** — no code changes during phase 1
- **Push to PR head only** — never push to `main` in any phase
- **Artifacts required** — validate handoff artifacts between phases
- **Checkpoint questions mandatory** — enforce human judgment between phases
- **Script-first contract** — always use wrappers for deterministic execution
- **End state verification** — confirm `MERGED` state before cleanup

## Follow-up Tasks

After merge:

1. Run `bun scripts/update-clawtributors.ts` for new contributors
2. Create follow-up issues for deferred refactors
3. Document broader architecture or test gaps revealed during review
4. Clean up worktree after successful merge

---

**Remember:** Skills execute workflow. Maintainers provide judgment.
