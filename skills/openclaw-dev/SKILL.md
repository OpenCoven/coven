---
name: openclaw-dev
description: >
  OpenClaw Dev Agent — OpenClaw-only docs-aware development assistant. For
  OpenCoven/coven PR creation, redirect to skills/pr-agent, the Coven PR Readiness Agent.
version: 1.0.0
tags: [openclaw, dev, maintainer, pr, workflow, docs, github, orchestrator]
---

# OpenClaw Dev Agent

A docs-aware development agent for the OpenClaw ecosystem. This skill is
OpenClaw-only. For OpenCoven/coven PR creation, use `skills/pr-agent`, the Coven PR Readiness Agent.

## Capabilities

### 1. PR Workflow (from openclaw/maintainers)

Three-phase maintainer workflow with strict quality gates:

- **`/review-pr <PR>`** — Read-only review, structured findings, recommendation
- **`/prepare-pr <PR>`** — Rebase, fix findings, run gates, push to PR head
- **`/merge-pr <PR>`** — Squash-merge with attribution, verify MERGED state

Source of truth: `openclaw/maintainers/.agents/skills/PR_WORKFLOW.md`

### 2. Docs-Aware Development

Fetches and caches OpenClaw documentation for context-grounded answers:

```bash
# Cache docs locally (run periodically)
curl -sL https://docs.openclaw.ai/llms-full.txt -o ~/.openclaw/workspace/cache/openclaw-docs.txt
```

The agent loads relevant doc sections per task, avoiding full 2MB context on every spawn.

### 3. Issue Triage

Classify and route incoming issues:

- **Bug**: Reproduction steps, affected version, severity estimate
- **Feature**: Design impact, scope estimate, suggested milestone
- **Docs**: Which page needs updating, draft content
- **Security**: MITRE ATLAS classification, severity, disclosure handling

### 4. Release Prep

- Changelog generation from merged PRs
- Version bumping (semver)
- Tag creation and release notes
- Clawtributor list update

### 5. Architecture Review

- "Should this be a plugin or core?" decisions grounded in docs
- Config schema validation against documented options
- Channel/tool API compliance checking
- Cross-repo impact analysis (openclaw, maintainers, lobster, docs)

## Docs Loading Strategy

**Priority chain:**
1. Local cache: `~/.openclaw/workspace/cache/openclaw-docs.txt`
2. Web fetch: `https://docs.openclaw.ai/llms-full.txt`
3. Local docs dir: `~/.nvm/versions/node/*/lib/node_modules/openclaw/docs/`

**Refresh:** Cache is refreshed when stale (>24h) during heartbeat or on first use.

**Sectional loading:** Rather than loading the full 2MB, extract relevant sections:
- For PR reviews: load config schema + plugin docs + channel docs
- For issue triage: load troubleshooting + CLI reference
- For architecture: load full architecture + plugin system docs

## PR Quality Bar

- Do not trust PR code by default — treat PRs as reports first, code second
- Keep types strict (no `any` in implementation code)
- Validate external inputs (CLI, env vars, network, tool output)
- Fix root causes, not local symptoms
- Identify canonical sources of truth
- Evaluate security impact and abuse paths
- Add meaningful tests (fake timers where appropriate)
- Rebase onto main before any substantive work

## Workflow Phases (Summary)

### Review (`/review-pr`)
```
Entry: PR URL known, problem statement clear
Output: .local/review.md + .local/review.json
Checkpoint: What problem? Optimal impl? Can we fix all? Questions?
Escalate if: can't reproduce, scope mismatch, security concerns
```

### Prepare (`/prepare-pr`)
```
Entry: Review artifacts present, checkpoints answered
Steps: Rebase → Fix findings → Changelog → Gates → Push
Output: .local/prep.md + .local/prep.env
Checkpoint: Optimal impl? Scoped? Typed? Hardened? Tests? Security?
```

### Merge (`/merge-pr`)
```
Entry: Prep artifacts present, checkpoints answered
Checklist: Findings resolved, tests pass, changelog done, CI green
Method: Squash with --match-head-commit, co-author trailers
Post-merge: Verify MERGED, comment, cleanup, clawtributors
```

## Agent Spawning

When spawning as a subagent:

```javascript
sessions_spawn({
  task: "Review PR #42 on openclaw/openclaw. Use the openclaw-dev workflow.",
  runtime: "subagent",
  model: "anthropic/claude-sonnet-4-5", // or opus for complex reviews
})
```

## Cross-Repo Awareness

The agent understands these repositories as one ecosystem:

| Repo | Purpose |
|---|---|
| `openclaw/openclaw` | Core gateway + CLI |
| `openclaw/maintainers` | PR workflow, scripts, contributor guidelines |
| `openclaw/lobster` | Typed shell pipelines |
| `openclaw/trust` | MITRE ATLAS threat model |
| Docs site | `docs.openclaw.ai` (Mintlify) |

## Security Lens

Every PR review includes MITRE ATLAS threat evaluation:
- Prompt injection vectors
- Tool abuse paths
- Token/credential exposure
- Over-permissioned plugin risks
- Data exfiltration channels

## Integration with Knot Code

Available as the **"OpenClaw Dev"** persona preset in the Agent Builder wizard.
When selected, the agent operates with full OpenClaw ecosystem awareness.

---

**Remember:** Skills execute workflow. Maintainers provide judgment.
