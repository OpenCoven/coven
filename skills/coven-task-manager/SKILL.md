---
name: coven-task-manager
description: Keep Coven task boards fresh by auditing stale, blocked, active, review, and completed work; use for scheduled task hygiene, task triage, and dynamic task-management runs.
---

# Coven Task Manager

Use this skill when asked to manage, refresh, audit, triage, or summarize Coven tasks, or when a scheduled automation asks for task-board freshness.

## Sources

Start with the Cave task board from the skill directory:

```bash
node ./task-manager.mjs report
```

The helper reads `~/.coven/cave-board.json` and writes `~/.coven/task-manager/freshness-report.md` by default.

If the canonical repo path is different, run the helper from this skill directory:

```bash
node ./task-manager.mjs report --coven-home ~/.coven
```

## Workflow

1. Load the task board and build a freshness report.
2. Inspect the report sections in this order: `Stale Running`, `Needs Human`, `Ready For Review`, then `Next Actions`.
3. Read `Thread Coordination` before touching individual cards. Treat it as the concurrency control surface for simultaneous sessions.
4. For every active/review/blocked thread, build a small ledger: card id/title, familiar, session id, repo/branch/worktree if known, last evidence checked, current state, and one next action.
5. Resolve collisions before dispatching new work:
   - If multiple cards share a session, resume that session once and update all linked cards from the same evidence.
   - If one familiar has multiple active lanes, choose the primary lane and mark the rest as waiting, review, or blocked with a reason.
   - If multiple lanes touch the same repo or branch, verify branch/worktree ownership before allowing parallel writes.
   - Prefer resuming a viable linked session over starting a new thread; start a fresh thread only when no current session can be resumed.
6. For each task that needs action, gather concrete evidence before changing state: linked sessions, git branches, PRs, CI, user messages, or task notes.
7. Keep task state fresh:
   - Move stale running work only when evidence shows it is blocked, ready for review, done, or abandoned.
   - Keep blocked tasks explicit: include the blocker, owner, and smallest next unblock action.
   - Move review tasks only after the actual review/CI state is checked.
   - Mark work done only when merge, delivery, or acceptance evidence exists.
8. Update the freshness report after meaningful changes.

## Guardrails

- Do not delete, archive, or bulk-close tasks unless the user explicitly asks.
- Do not invent blockers or progress.
- Do not mark a task done from memory alone; verify current state first.
- Preserve user-written task notes. Append concise evidence instead of replacing useful context.
- If evidence is missing, leave the task in place and write the missing check as the next action.
- Do not spawn parallel work for the same repo/branch/session just because several cards look stale; reconcile the existing thread ledger first.

## Default Automations

Install the default Codex automation set with:

```bash
node ./task-manager.mjs install-default-automations --status PAUSED
```

The templates are:

- `coven-task-freshness-daily` — daily sweep of stale, blocked, review, and active work.
- `coven-task-blocked-escalation` — weekday blocked-task escalation.
- `coven-task-weekly-cleanup` — weekly summary and cleanup recommendations.

Use `--status ACTIVE` only when the user wants the automations enabled immediately.

## Local Market Install

To make the skill visible to the local Cave Skills market, symlink it into Coven home:

```bash
node ./task-manager.mjs install-local --status PAUSED
```

The Cave market reads `~/.coven/skills/*/metadata.json` through the Coven daemon.
