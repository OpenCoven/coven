---
title: "Coven patch OpenClaw rescue loop design specification"
description: "Design spec for the Coven Rescue Loop: a beginner-friendly `coven patch openclaw` workflow that repairs broken OpenClaw checkouts via Codex or Claude Code."
---

# Coven Patch OpenClaw Design

Date: 2026-05-04
Status: draft for review
Owner: OpenCoven / Coven

## Purpose

Coven needs a public, beginner-friendly way to help developers repair a local OpenClaw source checkout when OpenClaw itself is unreliable, misconfigured, or blocked by a broken agent lane.

The first product slice is a **Coven Rescue Loop** for OpenClaw source repos:

```sh
coven patch openclaw
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
coven patch openclaw --repo ~/Documents/GitHub/openclaw/openclaw
```

The promise is simple:

> If OpenClaw breaks, Coven gives you a predictable repair room.

This is not an OpenClaw replacement. It is a standardized local harness workflow that can use Codex, Claude Code, or later harnesses to produce a reviewed, verified patch against a local OpenClaw repo.

## Goals

- Make OpenClaw patching immediately achievable for non-expert users.
- Provide an intuitive interactive flow for beginners.
- Provide a fast single-shot flow for advanced users.
- Keep Coven as the harness substrate and repair-room authority.
- Avoid relying on a healthy OpenClaw runtime to fix OpenClaw.
- Produce PR-ready local changes: summary, diff, verification output, and next-step guidance.
- Preserve user control: ask before harness launch in guided mode, never commit by default, and never push.

## Non-goals

- Do not bundle Coven into OpenClaw core.
- Do not create a general-purpose OpenClaw config repair system in v0.
- Do not auto-commit, auto-push, or auto-open PRs in v0.
- Do not support arbitrary shell harnesses before launch policy is explicit.
- Do not require comux or the OpenClaw plugin for the first rescue flow.
- Do not promise that Coven can fix every OpenClaw bug automatically.

## Primary users

### Beginner / distressed user

A user has a local OpenClaw source checkout and a clear symptom, but does not know the repo's test commands or where to patch. They need a guided repair flow that asks focused questions, launches a harness safely, verifies results, and explains what happened.

### Maintainer / power user

A maintainer knows the bug and wants a fast, repeatable command that starts a harness repair session with the right repo context and verification expectations.

### Demonstrator / advocate

A maintainer wants to show that Coven can repair OpenClaw through a standardized harness loop, using a real incident as proof that OpenClaw can become more resilient instead of less trusted.

## User experience

### Guided beginner flow

Command:

```sh
coven patch openclaw
```

Flow:

1. Detect or ask for the OpenClaw source repo.
2. Confirm the selected repo path and current git state.
3. Ask what is broken, with examples.
4. Offer harness choices based on `coven doctor` detection:
   - Codex
   - Claude Code
   - later: additional adapters
5. Explain what Coven will do:
   - create a supervised harness session;
   - keep execution scoped to the selected repo;
   - ask the harness to investigate root cause, patch, add tests, and verify;
   - leave changes uncommitted.
6. Ask for confirmation before launching the harness.
7. Stream or summarize session progress.
8. Run the selected verification gate or ask before running expensive gates.
9. Show result:
   - changed files;
   - verification commands and status;
   - concise summary;
   - next steps to review, commit, or open a PR.

The beginner flow should feel like a rescue wizard, not a command generator.

### Fast advanced flow

Command:

```sh
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
```

Optional flags:

```sh
coven patch openclaw "fix failing auth test" --repo ~/src/openclaw --harness codex
coven patch openclaw "fix UI regression" --harness claude --verify pnpm-check
coven patch openclaw "fix CLI panic" --non-interactive
```

Behavior:

- If a prompt is supplied, skip symptom collection.
- If `--repo` is supplied, skip repo discovery confirmation unless interactive safety requires it.
- If `--harness` is supplied, use that harness if available.
- Still do not commit or push by default.
- In non-interactive mode, fail closed when required information is missing.

## Command design

### `coven patch openclaw`

Interactive default. Best for first-time users.

### `coven patch openclaw <issue>`

Fast default. Uses the supplied issue text as the repair brief.

### Core flags

- `--repo <path>`: explicit OpenClaw source repo path.
- `--harness <codex|claude>`: choose a harness.
- `--verify <gate>`: choose verification profile.
- `--non-interactive`: fail instead of prompting.
- `--dry-run`: show planned repair steps without launching.
- `--keep-session`: preserve harness session for attach/replay.

### Verification profiles

Initial OpenClaw profiles:

- `auto`: inspect repo scripts and changed files, then choose a targeted gate.
- `pnpm-check`: run `pnpm check`.
- `targeted-test`: run a harness-selected targeted test command.
- `diff-only`: run `git diff --check` only; allowed only with an explicit warning.

The default should be `auto`.

## Architecture

Coven remains the local runtime authority.

```text
coven patch openclaw
  -> patch workflow planner
  -> repo detector / git state inspector
  -> harness adapter selection
  -> Coven supervised PTY session
  -> verification runner
  -> result summarizer
```

The patch workflow is a CLI-level orchestration layer on top of existing Coven primitives. It should not bypass the daemon's project-root, cwd, or harness validation rules.

### Components

#### Patch command module

Parses `coven patch openclaw`, handles interactive prompts, and builds a `PatchOpenClawRequest`.

#### OpenClaw repo detector

Determines whether a path is an OpenClaw source checkout. Detection should prefer explicit `--repo`, then current directory ancestry, then common local paths only if safe to inspect.

Minimum detection signals:

- `.git` exists;
- `package.json` exists;
- package name or repo metadata indicates OpenClaw;
- expected OpenClaw directories/scripts are present.

If detection is ambiguous, ask.

#### Git state inspector

Captures a safe summary before launching:

- branch name;
- HEAD commit;
- dirty file list;
- untracked file list;
- whether changes already exist.

If the repo is dirty, the beginner flow should explain that Coven will preserve existing changes and ask whether to continue. v0 should not auto-stash.

#### Repair brief builder

Turns user symptom text plus repo context into a harness prompt. The prompt should require:

- root-cause investigation before fixes;
- smallest targeted patch;
- tests where meaningful;
- verification command output;
- no commits or pushes;
- no destructive git actions;
- respect for existing uncommitted changes.

#### Harness session launcher

Uses existing Coven harness adapters to launch Codex or Claude Code in the selected repo root. No shell interpolation for prompt execution.

#### Verification runner

Runs verification after the harness returns or when the user asks. The runner should default to safe targeted gates and clearly label expensive commands.

For OpenClaw v0, preferred order:

1. `git diff --check`
2. targeted tests named by the harness or inferred from changed files
3. `pnpm check` when the user accepts or advanced mode requested it

#### Result summarizer

Produces a final local report:

- status: patched / blocked / verification failed;
- files changed;
- verification run;
- known limitations;
- next commands;
- reminder that nothing was committed or pushed.

## Data model

Add patch-session metadata on top of normal Coven sessions.

Suggested fields:

```text
patchTarget: "openclaw"
repoRoot: absolute canonical path
issue: user-provided repair brief
harnessId: codex | claude
verificationProfile: auto | pnpm-check | targeted-test | diff-only
status: planning | running | verifying | patched | blocked | failed
startedAt
completedAt
changedFiles[]
verificationCommands[]
verificationStatus
```

This metadata should be stored locally with session metadata and event history. It should avoid storing secrets, full environment dumps, private URLs, or credential material.

## Error handling

- Missing OpenClaw repo: ask for `--repo` or show examples.
- Dirty repo: explain and ask before proceeding in guided mode.
- Harness unavailable: run `coven doctor` guidance and offer installed alternatives.
- Harness exits without patch: mark blocked and preserve session logs.
- Verification fails: show failure, changed files, and attach/retry guidance.
- User cancels: stop before launch or terminate the session if already running.
- Non-interactive missing input: exit with structured error and no side effects.

## Safety and trust

- Never commit or push in v0.
- Never run outside the selected repo root.
- Never use `sh -c` for harness prompt execution.
- Never intentionally store secrets or full env dumps in event logs.
- Keep custom harness commands out of v0.
- Make all state-changing steps visible in guided mode.
- Leave the final diff for the human to review.

## Testing strategy

### Unit tests

- CLI argument parsing for guided and fast forms.
- OpenClaw repo detection success, ambiguity, and failure.
- dirty git state summaries.
- repair brief construction.
- verification profile selection.
- structured error behavior for non-interactive mode.

### Integration tests

- Launch against a fixture repo with a safe fake harness.
- Simulate harness success with changed files and passing verification.
- Simulate harness failure with no patch.
- Simulate verification failure after patch.
- Ensure dirty pre-existing changes are detected and not clobbered.

### Manual smoke tests

- `coven patch openclaw --dry-run`
- `coven patch openclaw "fix a fixture bug" --harness codex`
- `coven patch openclaw "fix a fixture bug" --harness claude`
- Verify final report and git status.

## Public messaging

Lead with reliability and control:

> Coven gives OpenClaw users a predictable repair room: choose a repo, choose a harness, get a verified patch.

Avoid framing that attacks OpenClaw. The story is:

> OpenClaw can be strengthened by a standardized external harness loop, especially when one built-in lane is unhealthy.

## v0 acceptance criteria

- `coven patch openclaw --dry-run` works in an OpenClaw checkout.
- `coven patch openclaw` provides an interactive beginner flow.
- `coven patch openclaw <issue> --repo <path> --harness codex` launches a supervised Codex repair session.
- Claude Code can be selected when installed.
- The workflow refuses ambiguous or missing repos in non-interactive mode.
- Existing uncommitted changes are detected and preserved.
- The workflow produces a final report with changed files and verification status.
- No v0 path commits, pushes, or modifies files outside the selected repo root.

## Phasing

### Phase 1: CLI rescue loop

Implement `coven patch openclaw`, repo detection, prompt building, harness launch, verification, and final reporting.

### Phase 2: Recipes

Add named recipes for repeated failure classes, such as Codex invalidated OAuth profile order.

### Phase 3: Installed OpenClaw repair

Add a separate `coven doctor openclaw` or `coven repair openclaw-install` path for local installed Gateway/config issues.

### Phase 4: OpenClaw plugin integration

Expose the same repair loop through the external `@opencoven/coven` OpenClaw plugin when OpenClaw is healthy enough to delegate work.

## Open questions deferred from v0

- Whether Coven should open PRs after explicit approval.
- Whether comux should become the preferred visual review surface for patch sessions.
- How recipe definitions should be distributed and versioned.
- How to safely support custom harnesses or user-provided commands.

## Amendment: single Supreme orchestrator (2026-05-05)

Status: accepted, supersedes the bare CLI orchestration described above.

### Motivation

The v0 implementation routes `coven patch openclaw` through a direct CLI function that opens SQLite, launches a PTY, and writes events on its own. That conflicts with the operational model, which names the Rust daemon as the single local authority for process launch, PTY lifecycle, and event persistence. It also makes every invocation a one-shot — there is no shared identity across sessions, and resumption, status, or attach must rediscover state each time.

We adopt a single persistent **Supreme** orchestrator that spans all Coven sessions, with **per-task Leads** spawned to drive individual repair runs.

### Roles

- **Supreme**: one local singleton, hosted by the existing Coven daemon. Owns the registry of active and historical Leads, repo locks, harness pool, and verification gating. Every `coven patch openclaw` invocation becomes a request to the Supreme.
- **Lead**: one per patch run. Owns a single `PatchOpenClawRequest` from intake through verification and final report. Reports up to the Supreme; never spawns its own Leads.

### Lifecycle

1. CLI invocation builds a `PatchOpenClawRequest` exactly as today (interactive or fast).
2. CLI ensures the Supreme is running (`coven daemon status` → start if absent in interactive mode; fail closed in `--non-interactive`).
3. CLI submits the request to the Supreme over the local socket.
4. Supreme assigns a Lead identity, stores the patch metadata as a session row, and spawns the Lead worker scoped to the selected repo root.
5. Lead drives the harness PTY, writes output events through the daemon, runs verification, and produces the final report.
6. CLI streams Lead progress (or summarizes on exit) through the same daemon API used by `coven attach`.
7. On Lead exit, Supreme retains the Lead record so it appears in `coven sessions` and can be re-attached if `--keep-session` was set.

### Concurrency rules

- Supreme holds at most one active Lead per `repoRoot`. A second concurrent request against the same repo is rejected with a structured error in `--non-interactive` mode and offered an attach in interactive mode.
- Across repos, multiple Leads may run, bounded by a daemon-side cap (default 4) that protects host load.
- Verification gates run inside the Lead, never inside the Supreme, to keep the Supreme responsive.

### Failure handling

- If the Supreme is unreachable: interactive mode offers `coven daemon start`; non-interactive mode exits with a structured `daemon_unavailable` error.
- If a Lead crashes mid-run: Supreme records `failed`, preserves prior events, and never auto-restarts. The user re-runs `coven patch openclaw` to start a fresh Lead.
- If the daemon dies with Leads in flight: on next start, Supreme marks those Leads `interrupted` so status output is honest.

### Surface area

New local API endpoints, scoped narrowly:

- `POST /patch/openclaw` — submit a `PatchOpenClawRequest`. Returns `{ leadId, sessionId }`.
- `GET /patch/leads` — list active and recent Leads with repo, harness, status.
- `GET /patch/leads/:id` — Lead detail including verification status.

`POST /patch/openclaw` is built on the existing `POST /sessions` machinery; it does not introduce a new launch path. It validates `repoRoot` against the same canonicalization and harness allowlist rules.

### CLI changes

- `coven patch openclaw` posts to the Supreme by default. The current direct-launch code path is retired once the Supreme path passes integration tests.
- `coven sessions` continues to list all sessions, with patch sessions identified by their `patch_metadata` event.
- Add `coven patch leads` as a thin alias that filters `coven sessions` to patch Leads.

### Migration

- Phase A: wire `POST /patch/openclaw` in the daemon, keep the direct-launch fallback in the CLI behind an internal flag for tests.
- Phase B: switch the default CLI path to Supreme; keep the direct path only for tests that exercise the Lead worker module without the daemon.
- Phase C: remove the direct fallback once integration tests cover the Supreme path end to end.

### Non-goals for this amendment

- No remote Supreme. The Supreme is local-only; remote orchestration is out of scope.
- No multi-tenant Supreme. One Supreme per local user, bound to `COVEN_HOME`.
- No automatic Lead restart or self-healing.
- No new Supreme-only persistence schema in v0; Leads reuse session/event tables with a `patch_metadata` event marker.

## Amendment: stored OpenClaw repository location (2026-05-24)

Status: accepted for the current CLI rescue loop.

### Motivation

`coven patch openclaw` should not require users to run the command from inside an OpenClaw checkout every time. Once Coven has validated an OpenClaw repo, it can safely remember the canonical path in its local store and reuse it later.

### Behavior

Repository resolution now follows this precedence:

1. Explicit `--repo <path>`.
2. OpenClaw source checkout found by walking upward from the current directory.
3. Stored OpenClaw repository path from the local Coven store.

When a real patch session launches, Coven stores the selected canonical repo path under the `openclaw` repository id in `<COVEN_HOME>/coven.sqlite3`. `--dry-run` remains side-effect free and does not update the stored path.

### Scope

This is intentionally a small registry, not a general repo manager. The initial stored target is OpenClaw only; future patch targets can add their own repository ids when they get dedicated patch flows.
