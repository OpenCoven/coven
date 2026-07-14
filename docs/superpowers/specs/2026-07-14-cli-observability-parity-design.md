# CLI observability parity with Cave and the daemon API — design

- **Date:** 2026-07-14
- **Issue:** [#366](https://github.com/OpenCoven/coven/issues/366)
- **Status:** approved for implementation (autopilot objective; assumptions stated inline)

## Problem

CovenCave and the daemon socket API expose user-relevant read surfaces that the
CLI cannot reach. A terminal-only user or a script cannot see what Cave sees
without hand-rolled `curl --unix-socket` calls — which is literally what
`docs/HUB-OPERATIONS.md` instructs today. The gaps, from the parity audit:

| Surface                | API route                                    | CLI before this design           |
| ---------------------- | -------------------------------------------- | -------------------------------- |
| Ecosystem overview     | `GET /api/v1/overview`                       | none — and endpoint returns hardcoded zero counts |
| Familiars roster       | `GET /api/v1/familiars`                      | none                             |
| Skills inventory       | `GET /api/v1/skills`                         | none                             |
| Memory files           | `GET /api/v1/memory`                         | none                             |
| Research log           | `GET /api/v1/research`                       | none                             |
| Coven Calls ledger     | `GET /api/v1/coven-calls[/:id]`              | none                             |
| Hub status/nodes/jobs/routing | `GET /api/v1/hub/*`                   | none (docs say use `curl`)       |
| Session detail         | `GET /api/v1/sessions/:id`                   | no non-interactive inspect       |
| Session events         | `GET /api/v1/sessions/:id/events`            | interactive `attach` replay only |
| Session log            | `GET /api/v1/sessions/:id/log`               | interactive replay only          |

Related UX findings:

- README Commands Reference omits shipped commands (`kill`, `vacuum`,
  `completions`, `sessions search`, `claim release|heartbeat|canary`).
- `docs/reference/cli.md` front matter names a `view` command that does not exist.
- `coven --help` lists a long flat command list with no workflow orientation
  for first-run users.

## Goals

1. Every read surface Cave renders is reachable from the CLI, for humans
   (tables/prose) and machines (`--json`).
2. `GET /api/v1/overview` reports real counts instead of zeros, so Cave and the
   CLI both benefit.
3. Session detail, events, and log become scriptable without a PTY.
4. Docs and help teach the surface: README reference completeness, hub ops via
   CLI, onboarding examples in `--help`.
5. Tests cover parsing, rendering, and CLI↔API parity for every new command.

## Non-goals (deliberate)

- **Travel, scheduler, actions, executor dispatch write paths** — these are
  machine-to-machine protocol surfaces (`coven.executor.v1`, travel sync,
  control-plane actions). Human CLI wrappers would invite misuse; ops docs
  cover the protocol. Revisit on demand.
- **`PUT /familiars/:id/icon`** — a Cave glyph-picker concern, not a terminal one.
- **TUI chat panes for familiars/skills/etc.** — the chat UI is the engine's
  surface (`coven-code`); this design keeps the substrate CLI scriptable and
  leaves engine UX to the engine.
- **`GET /cast-codes`** — cast codes are chat-input affordances; the chat UI
  surfaces them contextually.
- **Fixing the shadowed `GET /capabilities` route** (the
  `crate::capabilities::get_all` arm is unreachable behind the control-plane
  literal arm). Changing which body that route returns is a client-visible
  behavior change and deserves its own issue and contract note, not a rider on
  a parity PR.

## Design

### D1 — Render through the in-process API handler

New CLI read commands call `api::handle_request_with_body` directly (the same
router the daemon serves), then either print the body (`--json`,
pretty-printed) or parse it to render a human view. This makes `--json`
value-equal to the daemon API by construction, with one source of truth for
shapes.

- *Alternative considered:* calling `cockpit_sources`/`hub` functions directly —
  rejected: CLI and API shapes drift independently.
- *Alternative considered:* connecting to the daemon socket — rejected: these
  routes read files/SQLite fresh per request, the daemon adds a liveness
  dependency for offline reads, and `coven sessions` already established the
  direct-read pattern.
- *Devil's advocate:* could in-process reads disagree with a running daemon?
  No: the audited handlers (`overview`, `familiars`, `skills`, `memory`,
  `research`, `coven-calls`, `hub/*`, `sessions*` GETs) hold no daemon state;
  they re-read the store or `~/.coven` files per request.

### D2 — Command surface

```
coven status [--json]                      # composite: daemon health + overview counts + hub summary
coven familiars [--json]                   # GET /api/v1/familiars
coven skills [--json]                      # GET /api/v1/skills
coven memory [--json]                      # GET /api/v1/memory
coven research [--json]                    # GET /api/v1/research
coven calls [<id>] [--json]                # GET /api/v1/coven-calls[/:id]
coven hub status|nodes|jobs|routing [--json] [--state <s>]   # GET /api/v1/hub/*
coven sessions show <id> [--json]          # GET /api/v1/sessions/:id
coven sessions events <id> [--json] [--after-seq N] [--limit N]
coven sessions log <id> [--json]
```

Naming notes (devil's advocate round):

- `coven status` vs `coven daemon status`: the daemon command reports one
  process; `coven status` is the ecosystem dashboard and *includes* the daemon
  line. Precedent: `git status`, `gh status`. `overview` is an alias so the
  API-name crowd can type what they read.
- `coven calls` reads as "Coven Calls" naturally; single positional id gives
  the detail view like the API's `/:id`.
- `coven memory` does not clash with the separate `coven-memory` binary.
- Session subcommands are explicit words (`show`, `events`, `log`) rather than
  a bare id positional, so flags never fight positional parsing. All three
  accept unique id prefixes via the existing `resolve_session_ref`.
- `hub jobs --state` mirrors the API's `?state=` filter instead of inventing a
  new spelling.

### D3 — JSON contracts

- Leaf commands print the **exact API body**, pretty-printed (repo convention:
  `serde_json::to_string_pretty`, like `sessions --json`, `adapter list --json`).
- `coven status --json` is a **CLI-level composition** documented as such:
  `{ "health": <GET /api/v1/health body>, "overview": <GET /api/v1/overview body> }`.
  Precedent: `coven daemon status --json` already defines a CLI-owned JSON
  shape. Composing the two stable bodies avoids inventing a third contract for
  the same data.
- Human views parse the same body they would print — no second read path.

### D4 — Real overview counts

`overview_response` computes from the same sources the sibling routes use:

- `total_familiars` = familiars list length; `active_familiars` = distinct roster familiars referenced by an open session (`running`/`active`).
- `skills_count` = skills list length; `average_skill_score` = rounded mean of
  skill scores (0 when empty — scores are stubbed at 0.0 today, so this stays 0
  until scoring lands, which is honest).
- `research_iterations` = research row count; `last_research_delta` = final
  row's delta rounded to i32.
- `open_sessions` unchanged.

The DTO keeps its exact field set and snake_case serialization — no shape
change for Cave, only real numbers. Failures reading any source degrade to 0
rather than failing the endpoint (dashboard semantics: partial data beats a
500; the leaf endpoints still surface their own read errors loudly).

### D5 — Human rendering

Fixed-width table lines matching `format_session_line` conventions; theme
colors via `theme::` helpers only where already idiomatic (status tokens);
plain output stays pipe-friendly (no ANSI when not a TTY — existing
`theme::mode()` behavior). Empty states teach next steps, matching the
`sessions` empty-state convention (e.g. no familiars → point at
`~/.coven/familiars.toml` docs).

### D6 — Onboarding & help UX

- Top-level `after_help` on the clap `Cli`: a four-line "common first steps"
  block (`doctor` → `run codex "…"` → `sessions` → `status`).
- Near-miss suggestions already iterate the live clap command list, so new
  commands are covered automatically; add typo tests (`familars`, `stauts`,
  `overveiw` alias resolution) to pin it.
- README Commands Reference gains the new commands **and** the shipped-but-
  undocumented ones (`kill`, `vacuum`, `completions`, `sessions search`,
  `claim release|heartbeat|canary`).
- `docs/reference/cli.md` drops the phantom `view` command from its
  description; new reference page `docs/reference/cli-observe.md` covers the
  read-path commands; `docs/HUB-OPERATIONS.md` leads with `coven hub status`
  and keeps `curl` as the raw-protocol alternative.

### D7 — Code placement

New module `crates/coven-cli/src/observe.rs` owns command handlers and pure
render functions (main.rs is already 4k+ lines; renderers as pure
`&Value -> String` functions keep them unit-testable). `main.rs` gains only
enum variants + dispatch lines. Sessions subcommands land next to the existing
`SessionsCommand::Search` arm.

## Testing

- **Parse tests:** `Cli::parse_from` for every new command/flag combination,
  including conflicts (`--json` with positional id, `--after-seq` without
  `events` is rejected by clap structure).
- **Render tests:** pure renderers fed fixture JSON assert table headers,
  rows, and empty-state hints.
- **Parity tests:** seed a temp `COVEN_HOME` (familiars.toml, skills dir,
  memory tree, research.tsv, coven-calls.json, hub + session store rows);
  assert CLI JSON output equals the API body for each route.
- **Overview test:** seeded home yields real counts through
  `GET /api/v1/overview` (extends the existing route test that pinned zeros).
- **Near-miss tests:** new-command typos resolve as suggestions.
- Gates: `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --locked`, `python scripts/check-secrets.py`.

## Rollout

Single PR, commits staged per checkpoint (overview fix → status → cockpit
commands → hub → sessions detail → docs/UX). No migrations, no daemon protocol
changes, additive CLI surface only.
