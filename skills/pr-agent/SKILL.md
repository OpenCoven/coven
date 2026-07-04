---
name: pr-agent
description: >
  Coven PR Readiness Agent for assembling agent-ready OpenCoven pull requests.
  Use when preparing PR context, verification, risk, rollback, and handoff
  material for OpenCoven/coven.
version: 1.0.0
tags: [github, pr, readiness, opencoven, coven]
---

# Coven PR Readiness Agent

This skill prepares OpenCoven pull requests for maintainer review. It does not
own merge authority; it produces the readiness packet a maintainer or another
agent can trust.

## Readiness Packet

Build the PR around five concrete sections:

- Context: user problem, linked issue, constraints, and prior decisions.
- Implementation: files changed, behavioral surface, and compatibility notes.
- Verification: commands run, manual checks, skipped checks, and exact failures.
- Risk and Rollback: regression risk, data/security impact, and rollback path.
- Agent Handoff: current state, follow-ups, and known gaps.

## Context Bundle

Before drafting or updating a PR, gather:

- `git status --short`
- `git diff --stat`
- Linked issue or product request
- Changed docs, tests, scripts, and Rust crates
- Any local proof output needed by the PR body

Do not create or update a PR until the context bundle is internally consistent
and unrelated dirty work is identified.

## Template Assembly

Use `.github/pull_request_template.md` as the output contract. Keep headings
stable so automation and reviewers can scan the packet quickly.

The PR body must include:

- `Closes #` when there is a linked issue
- Summary and files changed
- Implementation notes with user-visible behavior
- Verification matrix with exact commands
- Risk level and rollback plan
- Agent handoff notes for remaining work

## Verification Matrix

Default repository gates:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 scripts/check-secrets.py
```

For docs-only or script-only changes, record the narrower focused checks and say
why the full gate was not necessary.

## Guardrails

- Preserve unrelated dirty work.
- Prefer the smallest complete PR.
- Separate required fixes from optional follow-ups.
- Call out skipped checks explicitly.
- Do not create or update a PR until verification evidence is present or the gap
  is named.
