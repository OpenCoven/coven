# OpenCoven Public Roadmap

_Last updated: 2026-05-09_

This roadmap is the public progress ledger for **OpenCoven**, **Coven**, and **comux**.

It is intentionally written as a community-facing map, not an internal promise sheet. Items move when they are designed, implemented, tested, released, or deliberately cut. Dates are avoided unless a release is already scheduled.

## North star

OpenCoven is building a local-first agent workspace where autonomous coding harnesses can work inside explicit rooms:

- **Coven** is the runtime substrate: project-scoped harness sessions, PTYs, logs, and local APIs.
- **comux** is the cockpit: visible panes, worktrees, agent lanes, rituals, review, and merge flow.
- **OpenMeow / OpenClaw integrations** are intake and orchestration surfaces that can hand work into the same local runtime without hiding what happened.

The simple promise:

> One project. Any harness. Visible work.

## How to read this roadmap

- **Shipped** means the work exists in public code or public package/release artifacts.
- **Now** means active stabilization or near-term implementation.
- **Next** means planned after the current stabilization slice.
- **Later** means directionally important, but not allowed to distract from the local-first MVP.
- **Lab** means experimental work we are exploring in public when possible, but not treating as a stable promise yet.

## Current snapshot

### Coven

**Status:** early public MVP, usable by adventurous local-first developers.

Shipped:

- Public `OpenCoven/coven` repo.
- Rust CLI command named `coven`.
- Beginner-friendly `coven` / `coven tui` entrypoint.
- `coven doctor` setup checks.
- Local daemon lifecycle: `coven daemon start/status/stop`.
- Project-root and cwd boundary guard.
- Built-in Codex and Claude Code harness adapters.
- PTY-backed `coven run codex|claude <prompt>` sessions.
- SQLite-backed session metadata and event log.
- Session browser and rituals: **Rejoin**, **View Log**, **Summon**, **Archive**, **Sacrifice**.
- Scriptable and human session output: `coven sessions`, `--plain`, and `--all`.
- Local HTTP-over-Unix-socket API for clients.
- Published npm wrapper packages:
  - `@opencoven/cli`
  - `@opencoven/cli-macos`
  - `@opencoven/cli-linux-x64`
- External OpenClaw bridge package kept outside OpenClaw core.
- Architecture, operational model, product spec, brand docs, and MVP plan.

Now:

- Harden the daemon API as a versioned public contract.
- Add compatibility tests for the external OpenClaw plugin against versioned daemon responses.
- Improve first-run onboarding and failure recovery for missing harness CLIs.
- Keep the public docs aligned with the actual CLI/API surface.

Next:

- Stabilize a named Coven API version for comux and other local clients.
- Add stronger real-world smoke tests for daemon restart, attach, kill, archive, summon, and sacrifice flows.
- Improve install verification across macOS and Linux package paths.
- Turn the MVP checklist into linked GitHub issues/milestones.

Later:

- Generic command adapter after enough real usage.
- Additional harness adapters such as Hermes, Aider, Gemini, OpenCode, or user-defined local harnesses.
- Policy/approval hooks for sensitive actions.
- Richer session artifacts and attachments.
- Optional cloud/team collaboration only after the local runtime is boringly reliable.

### comux

**Status:** early public product, useful as a standalone terminal cockpit and becoming the first visual Coven client.

Shipped:

- Public `comux` npm package and CLI command.
- tmux cockpit for visible parallel work.
- Git worktree isolation per agent lane.
- Agent launcher registry with multiple coding CLIs.
- Multi-select agent launches.
- Pane menu for inspect, merge, PR, attach, and cleanup flows.
- File browser, code preview, and diff-oriented review affordances.
- Project sidebar, pane visibility controls, and reopen flows.
- Rituals for repeatable project setups.
- Lifecycle hook docs and generated hook reference.
- Docs site and public README/spec/smoke docs.
- Coven session visibility and launch integration through the local bridge path.
- OpenClaw repair ritual direction started publicly.

Now:

- Stabilize the Coven session UX in comux: list, open, launch, attach/rejoin, and unavailable states.
- Keep comux useful without Coven installed.
- Continue dogfooding comux-on-comux for branch/worktree hygiene.
- Tighten review/merge flows so agent output stays explicit and inspectable.

Next:

- Promote a crisp `comux + Coven` demo loop:
  1. Open project in comux.
  2. Launch a Coven-backed Codex or Claude session.
  3. Watch it as a visible pane/session.
  4. Inspect files and diffs.
  5. Merge, PR, archive, or clean up explicitly.
- Add public issues for rough edges discovered during dogfooding.
- Improve onboarding for tmux, agent CLI detection, and Coven availability.
- Make Discord updates easy to generate from shipped commits and roadmap issues.

Lab:

- Native macOS cockpit exploration.
- Desktop shortcuts and faster project/session switching.
- OpenMeow intake handoff into comux/Coven sessions.

### OpenClaw / OpenMeow integration path

**Status:** opt-in bridge direction, not bundled into OpenClaw core.

Shipped:

- Technical OpenClaw bridge spike completed and intentionally parked before merging into core.
- External `@opencoven/coven` plugin direction established so OpenClaw core stays clean.
- Local socket/API boundary makes Coven the authority layer.

Now:

- Treat the Coven API as the compatibility boundary.
- Add compatibility tests before encouraging broad plugin usage.
- Keep OpenMeow/OpenClaw copy honest: intake and orchestration sit above Coven; they do not replace the runtime substrate.

Next:

- Publicly document the supported plugin path once API versioning lands.
- Add a demo showing a task moving from intake to Coven runtime to comux review.

## Public milestones

### Milestone A — Local runtime foundation

Status: **mostly shipped**

- [x] Public repo and docs
- [x] `coven` CLI
- [x] Project-root safety
- [x] Codex and Claude adapters
- [x] PTY sessions
- [x] SQLite session/event ledger
- [x] Daemon lifecycle
- [x] Local sessions/events API
- [ ] Versioned API contract
- [x] Compatibility tests for external clients

### Milestone B — Visible cockpit foundation

Status: **shipped, stabilizing**

- [x] Public `comux` package
- [x] tmux panes
- [x] git worktrees
- [x] agent launcher registry
- [x] file browser / diff review
- [x] rituals
- [x] merge and PR-oriented pane menu
- [x] Coven session visibility
- [ ] Coven attach/rejoin UX polish
- [ ] documented end-to-end comux + Coven demo

### Milestone C — Transparent community loop

Status: **now**

- [x] Public roadmap document
- [ ] GitHub milestone labels for `roadmap`, `now`, `next`, `later`, `area:coven`, `area:comux`, `good first issue`, `help wanted`
- [ ] First public Discord roadmap post
- [ ] Weekly shipped/building/next update cadence
- [ ] Public issue board linked from Discord

### Milestone D — Harness expansion

Status: **next/later**

- [x] Future harness research started
- [x] Adapter contract documented
- [ ] Generic command adapter design from real usage
- [ ] Third harness proof
- [ ] Harness compatibility docs

### Milestone E — Intake to runtime to review

Status: **next/lab**

- [ ] OpenMeow/OpenClaw intake creates or requests a Coven task
- [ ] Coven owns the session and event log
- [ ] comux shows the session for review
- [ ] user explicitly merges, PRs, archives, or deletes work

## Discord transparency model

We should keep Discord updates lightweight and repeatable.

### Suggested channels

- `#roadmap` or a forum-style `Roadmap` channel for milestone threads.
- `#dev-updates` for weekly summaries.
- `#help-wanted` for scoped issues that community members can actually pick up.

### Weekly update template

```md
## OpenCoven weekly update — YYYY-MM-DD

### Shipped
- ...

### Building now
- ...

### Next up
- ...

### Help wanted
- ...

### Links
- Roadmap: https://github.com/OpenCoven/coven/blob/main/docs/ROADMAP.md
- Coven issues: https://github.com/OpenCoven/coven/issues
- comux issues: https://github.com/BunsDev/comux/issues
```

### Rules for honest updates

- Do not promise dates unless we are already in release mode.
- Link shipped work to commits, releases, issues, or docs.
- Mark experiments as **Lab** instead of pretending they are committed roadmap items.
- Separate **Coven runtime**, **comux cockpit**, and **OpenMeow/OpenClaw intake** so people understand the architecture.
- Prefer small public issues over giant vague tasks.
- Ask for help only when the task has a clear acceptance condition.

## First public Discord post

```md
We opened a public roadmap for OpenCoven/Coven/comux so progress is easier to follow.

The short version:
- Coven is the local runtime substrate: project-scoped Codex/Claude sessions, PTYs, logs, daemon API.
- comux is the visible cockpit: tmux panes, worktrees, rituals, review, merge/PR flows.
- The next serious focus is hardening the Coven API contract and polishing the comux + Coven demo loop.

Roadmap: https://github.com/OpenCoven/coven/blob/main/docs/ROADMAP.md
Coven: https://github.com/OpenCoven/coven
comux: https://github.com/BunsDev/comux

We’ll start posting lightweight shipped / building / next updates here so the work is easier to follow and easier to help with.
```
