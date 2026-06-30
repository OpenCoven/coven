# PR Agent — Maintainer Workflow Orchestrator

This skill coordinates the OpenClaw maintainer PR workflow across three phases:

1. **review-pr** — Review-only analysis with structured findings
2. **prepare-pr** — Fix findings and prepare PR for merge
3. **merge-pr** — Deterministic squash-merge with attribution

## Purpose

The PR Agent ensures:

- **Strict phase progression** — Review → Prepare → Merge (no skipping)
- **Checkpoint enforcement** — Human judgment required between phases
- **Quality bar adherence** — Types, tests, security, scope
- **Script-first execution** — Deterministic wrapper commands
- **Artifact validation** — Structured handoffs via `.local/*.json` and `.local/*.md`

## Source of Truth

All workflow rules, quality bars, and checkpoint questions derive from:

**`PR_WORKFLOW.md`** — `.agents/skills/PR_WORKFLOW.md`

## Usage

### As an orchestrator

The PR Agent guides you through the full workflow:

```bash
# Start with a PR number or URL
/pr-agent 123
```

The agent will:

1. Run `review-pr` and produce `.local/review.json` + `.local/review.md`
2. Ask checkpoint questions before advancing to `prepare-pr`
3. Run `prepare-pr` to fix findings, rebase, and push to PR head
4. Ask checkpoint questions before advancing to `merge-pr`
5. Run `merge-pr` to perform a **locally signed** squash merge with co-author attribution

### Manual phase execution

You can also invoke individual phases:

```bash
/review-pr 123
# [answer checkpoint questions]

/prepare-pr 123
# [answer checkpoint questions]

/merge-pr 123
```

### Script-only execution

For debugging or advanced use:

```bash
scripts/pr-review 123
scripts/pr-prepare init 123
scripts/pr-merge verify 123
scripts/pr-merge run 123
```

## Checkpoint Questions

### Before Prepare

1. What problem are they trying to solve?
2. What is the most optimal implementation?
3. Can we fix up everything?
4. Do we have any questions?

### Before Merge

1. Is this the most optimal implementation?
2. Is the code properly scoped and reusing existing logic?
3. Is the code properly typed?
4. Is the code hardened against abuse?
5. Do we have enough tests? Regression tests?
6. Are tests using fake timers where appropriate?
7. Do you see any follow-up refactors we should do?
8. Did any changes introduce security vulnerabilities?

## Escalation Triggers

Stop and ask for human judgment when:

- Problem cannot be reproduced or confirmed
- PR scope does not match stated problem
- Unresolved security or trust-boundary concerns
- Cannot verify behavior changes with meaningful tests
- Fixing findings requires architecture changes outside safe PR scope
- Security hardening requirements unresolved

## Verified merge rule for `openclaw/openclaw`

- If Verified status on `main` matters, do **not** rely on GitHub to create the final squash commit.
- A GitHub/server-created squash commit may be unsigned even when local git signing is configured correctly.
- Prefer a **locally created signed squash commit** after all review/prepare gates pass.
- If the PR branch contains unsigned commits, re-sign or recreate them before merging instead of carrying them into `main`.

## Quality Bar

- Do not trust PR code by default
- Keep types strict (no `any` in implementation)
- Validate external inputs (CLI, env vars, network, tool output)
- Fix root causes, not local symptoms
- Identify canonical sources of truth
- Evaluate security impact and abuse paths
- Add meaningful tests (fake timers where appropriate)

## Repository Integration

This skill integrates with:

- **review-pr** → `.agents/skills/review-pr/`
- **prepare-pr** → `.agents/skills/prepare-pr/`
- **merge-pr** → `.agents/skills/merge-pr/`
- **Wrapper scripts** → `scripts/pr-review`, `scripts/pr-prepare`, `scripts/pr-merge`

## Agent Configuration

See `agents/openai.yaml` for the OpenAI/ChatGPT agent interface configuration.

## Guardrails

- Read-only during review phase
- Push to PR head only, never to `main`
- Artifacts required between phases
- Checkpoint questions mandatory before transitions
- Script-first contract enforced
- End state verification (MERGED, not CLOSED)

## Working Rule

> **Skills execute workflow. Maintainers provide judgment.**
>
> Always pause between skills to evaluate technical direction, not just command success.

---

For full details, see `SKILL.md`.
