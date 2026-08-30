# Issue #670 Docs Program — Status and Decision Record (2026-08-30)

**Scope:** Verified status of
[OpenCoven/coven#670](https://github.com/OpenCoven/coven/issues/670)
(docs: progressive disclosure, single-source docs, and E2E certification)
against `main` at commit `1364cec9dbaf1e2aca2e4544dec0e1ce807d859c`
(2026-08-30T06:31:53-05:00), its child issues, and the `OpenCoven/coven-docs`
site repo. Facts and evidence only; no code changes are proposed by this
record.

**Verdict in one line:** The executable-help half of #670 and the packaged E2E
lane are on `main`; the canonical journey and docs CI live and run in
`OpenCoven/coven-docs`; the single-source cleanup of this repository
(README shrink, remaining local public pages, CI ownership enforcement) and the
#779 certification matrix are the remaining program work.

---

## Program structure (evidence)

Maintainer comment on #670 (BunsDev, 2026-08-20T23:16:48Z) tracks execution as
Beads epic `coven-v8l` with children:

| Child | Beads | Title | State (2026-08-30) |
| --- | --- | --- | --- |
| #774 | `coven-v8l.1` | feat(cli): add progressive help disclosure and help contract | open — implementation merged on main |
| #775 | `coven-v8l.2` | docs: reshape canonical first-session and troubleshooting journey | open — canonical journey live in coven-docs |
| #776 | `coven-v8l.3` | docs: remove duplicate local public documentation | open — partially done, largest remaining slice |
| #777 | `coven-v8l.4` | test(cli): add packaged first-session E2E journey | **closed** — merged via #835 |
| #778 | `coven-v8l.5` | ci(docs): add canonical docs build, link, and browser journey | open — workflow live in coven-docs |
| #779 | `coven-v8l.6` | test: certify Coven end-to-end from packaged artifact through recovery and release evidence | open — matrix not yet evidenced |

GitHub Project 3 / Project 8 rollup state is not verifiable here: the task
constraints forbid GraphQL, and the REST v3 API exposes no project state for
this token. Beads state is cited only from the maintainer comment above.

## What exists on `main` today (with evidence)

### #774 — progressive help: merged, issue still open

PR #834 `feat(cli): add progressive public help`
(`OpenCoven:finish/670-progressive-help` → `main`) merged 2026-08-25T12:08:08Z
as commit `3724f26455a9488e80eaa9d8379ee7833f69e52d` (+1828/−15 across 8
files). Delivered on main:

- `crates/coven-cli/src/help.rs` — default top-level help lists exactly eight
  commands (doctor, setup, run, sessions, attach, daemon, status, help) via
  `TOP_LEVEL_AFTER_HELP`; `coven help --all` renders six public groups
  (`HELP_GROUPS`, 39 commands total); `coven help --all --json` emits the
  machine-readable contract.
- Coverage enforcement: help.rs fails when any visible command lacks public
  metadata (`public help metadata is missing visible command(s)`,
  `public command ... is missing an about string`).
- `crates/coven-cli/tests/help_disclosure.rs` (+464) asserts the curated
  top-level command list and that internal commands (`chat`, `config`) are not
  listed.
- `scripts/export-cli-help-contract.mjs` (+279) and
  `scripts/export-cli-help-contract-test.mjs` (+323) export a deterministic
  JSON help contract (schemaVersion 1) and reject duplicate commands, internal
  leakage (`process-supervisor`, `serve`), non-canonical docs URLs (must be
  stable `https://docs.opencoven.ai/docs/...`), ANSI escapes, and
  machine-specific paths.
- `scripts/test-cli-prepublish.mjs` chains `onboarding-docs-test.mjs`,
  `cli-docs-test.mjs`, and the contract test into the npm prepublish gate.

Local verification for this record (no cargo/python in the executing
environment): `node --test scripts/cli-docs-test.mjs` 7/7 pass;
`node --test scripts/onboarding-docs-test.mjs` 16/16 pass; `git diff --check`
clean. Rust-side tests (`cargo test`) were not run locally (no toolchain) and
are covered by CI.

### #777 — packaged first-session E2E: merged and closed

PR #835 `test(cli): add packaged first-session E2E journey`
(`OpenCoven:finish/777-packaged-journey` → `main`) merged 2026-08-25T13:47:31Z
(+2686/−177, 13 files); issue #777 closed 2026-08-25. Delivered on main:

- `scripts/user-journey-e2e.mjs` — hermetic journey through the installed npm
  wrapper with isolated `COVEN_HOME`: bare-runner `coven doctor` fails closed
  with first-run guidance, deterministic fake harness install, daemon
  lifecycle, a real packaged `coven run codex ...` turn, sessions/show/events/
  log inspection, archive/summon/sacrifice, bounded `--cwd` rejection, and
  daemon cleanup/shutdown. Curated top-level surface re-checked in the
  packaged artifact (`CURATED_COMMANDS`).
- CI lanes in `.github/workflows/ci.yml`: `npm-onboarding-pr` (PRs touching
  npm packaging; linux-x64 + windows) and `npm-onboarding-main` (push;
  macos-26 arm64, macos-15-intel x64, ubuntu x64, windows) both run
  `node scripts/test-cli-prepublish.mjs --skip-build --skip-secrets-scan`,
  which drives `runPackagedUserJourney`. This is the issue's "required PR
  lane"; the push matrix covers macOS arm64/x64, Linux x64, Windows x64.

### #775 / #778 — canonical journey and docs CI: live in `OpenCoven/coven-docs`

`OpenCoven/coven-docs` (Fumadocs + MDX, default branch `main`, last push
2026-08-26T10:01:53Z):

- `content/docs/guide/getting-started.mdx` follows the issue's user story:
  preflight (doctor) → connect a harness → run a first session → inspect the
  result → lifecycle actions → `Continue` next steps pointing at
  `/docs/cli/setup`, `/docs/guide/install`, `/docs/guide/concepts`,
  `/docs/cli/sessions`, and `/docs/reference/troubleshooting` (recovery route).
- `scripts/check-cli-docs.mjs` enforces canonical coverage: required pages
  (cli index, install, install-debugging, interactive, doctor, setup, daemon,
  run, sessions, observe, hub-scheduler, engine-auth, repo-workflow,
  patch-openclaw, pc, uninstall) and required guide pages (getting-started,
  install, platforms, deployments), plus per-page required command mentions.
- `.github/workflows/docs.yml` runs on every PR and push to `main`:
  `pnpm check:source-drift` freshness gate, Chrome install, `pnpm verify`
  (= `check` [typecheck, content/link/anchor guards, api-runner tests] +
  Next build) + `test:smoke`, a generated-tree cleanliness gate, and evidence
  artifact upload. `scripts/smoke-docs.mjs` drives real Chromium routes: `/`,
  `/docs`, `/docs/guide/getting-started`, `/docs/cli/setup`,
  `/docs/guide/ecosystem`, `/docs/reference/api`, with screenshots.
  Workflows present: `docs.yml`, `docs-source-drift.yml`, `docs-live.yml`.
- Recent coven-docs merges: #56 (2026-08-24, "refactor: certify and redesign
  the Coven documentation release surface"), #72 (2026-08-24, production
  sentinel fix), #74 (2026-08-26, "docs: reconcile Coven CLI source drift").

### Historical context (per the issue text)

The 2026-08-07 three-wave design
(`docs/superpowers/specs/2026-08-07-final-documentation-single-source-audit-design.md`)
and its wave plans (`docs/superpowers/plans/2026-08-07-documentation-wave-{a,b,c}-*.md`,
96 unchecked tasks total) remain the historical audit plan. Parts were
absorbed earlier: commit `6c267f871f45caa1e66e8d91c2a26573b158b347`
(`docs: establish canonical public documentation links (#668)`, 2026-08-07)
removed 1958 lines across 97 files and left `docs/GETTING-STARTED.md`,
`docs/CONCEPTS.md`, and `docs/TROUBLESHOOTING.md` as 8-line canonical
pointers. The wave-plan checkboxes were never updated in-file and their
remaining substance (README, residual local pages, enforcement) is still open
work under #776.

## Verdict against #670 acceptance criteria

| # | Acceptance criterion | Verdict | Evidence |
| --- | --- | --- | --- |
| 1 | Default top-level help exposes at most eight core commands plus help | **Met** | `help.rs` `TOP_LEVEL_AFTER_HELP` (7 core + help); `help_disclosure.rs` asserts the exact curated list |
| 2 | `coven help --all` includes every public command without internal commands | **Met** | `HELP_GROUPS` (6 groups, 39 commands); missing-metadata hard error in help.rs; contract test rejects `process-supervisor`/`serve` leakage |
| 3 | Canonical getting started covers install, readiness, one recorded run, inspection before advanced next steps | **Met (coven-docs)** | `content/docs/guide/getting-started.mdx` structure 1–4 + Continue |
| 4 | Every core command maps to a stable canonical documentation route | **Met** | `HELP_GROUPS` docs paths; contract test requires stable `docs.opencoven.ai` URLs; coven-docs `check-cli-docs.mjs` enforces the pages exist |
| 5 | Public-doc directories contain only approved pointers or source-adjacent exceptions, enforced by CI | **Not met** | No ownership-enforcement guard exists in `scripts/` or `.github/workflows/ci.yml`; `cli-docs-test.mjs` checks only specific routes. Only 3 local pages are pointers today |
| 6 | README is a concise landing page rather than a second manual | **Not met** | `README.md` on main is 867 lines / 47,904 bytes with Commands Reference, Local API, Architecture, Repository Structure, Configuration, FAQ, and Troubleshooting sections |
| 7 | Packaged CLI E2E proves doctor, daemon, run, inspect, lifecycle actions, failure guidance, shutdown | **Met** | `scripts/user-journey-e2e.mjs` + `test-cli-prepublish.mjs`; CI lanes `npm-onboarding-pr`/`npm-onboarding-main` (#835) |
| 8 | `coven-docs` PR CI builds, validates links, and browser-tests the primary journey | **Met (coven-docs)** | `docs.yml` verify + smoke (Chromium) on PR and push |
| 9 | Delivery tracked in Beads and Project 3; certification evidence under Project 8 | **Partially verified** | Beads IDs confirmed via maintainer comment (2026-08-20); Project state unverifiable under REST-only constraints |

## What remains

1. **#776 (critical path, this repo).** Shrink `README.md` to a landing page;
   finish the local public-page reduction that #668 started (the three
   2026-08-07 wave plans hold the residual page list); add the CI guard that
   enforces the "only approved pointers or source-adjacent exceptions" rule
   from `docs/DOCS-MAINTENANCE.md`. Its recorded blockers (`coven-v8l.1`,
   `coven-v8l.2`) are satisfied on the ground — help (#834) and the canonical
   journey (coven-docs) have landed.
2. **#779 certification.** Execute and evidence the certification matrix
   (hermetic lane evidence exists via #777; real providers, remote hosts,
   destructive recovery, interactive surfaces, and deployed-site checks
   remain). `scripts/certify-release.sh` (PR #851 merged 2026-08-28T21:48:01Z)
   is the release-certification helper; #805 (release authorization), #803,
   #804, #807, #808 feed it.
3. **Issue close-out.** #774, #775, and #778 are still open although their
   implementation surfaces are merged/live; they need maintainer verification
   and closure (Beads `coven-v8l.1`, `.2`, `.5`), then #670 and the
   `coven-v8l` epic can close.

## Critical path

#776 cleanup + ownership-enforcement CI → close-out of #774/#775/#778 →
#779 certification evidence (consumes #777 + deployed-site checks, gated by
#805) → #670 umbrella completion.

## Record method and limitations

- Investigated 2026-08-30 via GitHub REST only (`gh api`); GraphQL was not
  used, so GitHub Project rollup state is unverified.
- Beads state cited from the maintainer comment on #670; bead records were
  not read or modified.
- No Rust toolchain or Python in the executing environment; Rust/Python CI
  checks were not run locally. The two Node docs guards were run locally and
  pass; both are also run by CI's policy/prepublish lanes.
