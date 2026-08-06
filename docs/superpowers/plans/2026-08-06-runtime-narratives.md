# Runtime Narrative Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the current user-facing documentation accurately describe Coven's managed interactive UI, Windows local IPC, package/runtime boundaries, adapter maturity, and session cancellation behavior.

**Architecture:** Treat the Rust implementation as the authority for behavioral claims and use one consistent transport vocabulary: Unix sockets on Unix-like hosts and owner-only named pipes on Windows. Keep historical plans and release notes intact; update only active guides and their cross-links so users encounter the same capability boundaries from entry points, reference pages, and architecture diagrams.

**Tech Stack:** Markdown, Mintlify-compatible front matter and Mermaid diagrams, existing Python API-contract documentation checks.

---

## File structure

| File group | Responsibility |
| --- | --- |
| `README.md`, `docs/GETTING-STARTED.md`, `docs/start/coven-tui.md`, `docs/start/onboarding.md`, `docs/install/windows.md`, `docs/platforms/windows.md` | Explain the managed `coven-code` default UI, legacy fallback, Windows setup, and package/runtime boundaries to users beginning a session. |
| `docs/daemon/{configuration,coven-home,index,socket-api,safety-model,auth-posture,trust-boundary}.md` | Define the platform-specific local IPC transport and same-user trust model. |
| `docs/{API,AUTH,API-CONTRACT,ARCHITECTURE,CONCEPTS,OPERATIONAL-MODEL,SAFETY-MODEL,index}.md` and `docs/{concepts,reference}/**` | Keep canonical API, security, feature, and topology references aligned with the daemon contract. |
| `docs/HARNESS-ADAPTERS.md`, `docs/concepts/{architecture,features}.md` | Separate bundled defaults, trusted opt-in recipes, and genuinely future adapters. |
| `docs/reference/cli-kill.md` | Describe the Unix-only direct CLI command and the daemon-owned Windows cancellation route. |

### Task 1: Reconcile interactive entry points and Windows distribution

**Files:**
- Modify: `docs/GETTING-STARTED.md:48-91`
- Modify: `docs/start/coven-tui.md:1-35`
- Modify: `docs/start/onboarding.md:10-17`
- Modify: `docs/install/windows.md:10-45`
- Modify: `docs/platforms/windows.md:8-44`
- Modify: `docs/ARCHITECTURE.md:189-219`

- [ ] **Step 1: Replace the stale default-UI descriptions**

  State in the Getting Started and Windows guides that bare `coven`, `coven chat`,
  and `coven tui` open the managed `coven-code` interactive UI. Replace
  prompt-first launcher and slash-command-palette language with:

  ```markdown
  The default command opens the managed Coven interactive UI, powered by
  `coven-code`. On a first interactive run, Coven offers to install the pinned
  engine when it is not already available.

  The older in-process TUI is a temporary compatibility fallback only. Set
  `COVEN_LEGACY_TUI=1` explicitly when migration troubleshooting requires it;
  it is deprecated and will be removed in a future release.
  ```

  Keep explicit `coven doctor`, `coven daemon`, and `coven run` examples unchanged.
  Change the legacy TUI and onboarding pages so they describe that compatibility
  route rather than presenting it as the default UI.

- [ ] **Step 2: Correct beginner platform claims**

  In `docs/GETTING-STARTED.md`, replace the Unix-only prerequisite with
  cross-platform wording and list all three bundled defaults:

  ```markdown
  - A supported local runtime: macOS, Linux, or Windows x64.
  - At least one bundled harness CLI on `PATH`: `codex`, `claude`, or `copilot`.
  ```

  In `docs/platforms/windows.md`, add a Windows IPC note after the state section:

  ```markdown
  Native Windows runs the daemon over an owner-only named pipe for its selected
  `COVEN_HOME`; it does not create `<COVEN_HOME>/coven.sock`. Use `coven daemon
  status` to inspect the active daemon endpoint. WSL2 remains a separate
  Unix-like environment and uses its own Unix socket.
  ```

- [ ] **Step 3: Correct the architecture distribution snapshot**

  Replace the stale future-tense Windows package bullet in
  `docs/ARCHITECTURE.md` with an explicit three-way distinction:

  ```markdown
  The Rust source supports native Windows, including the owner-only named-pipe
  daemon transport. `@opencoven/cli-windows` is the published Windows x64
  platform package. A wrapper install selects only the platform package version
  declared by that wrapper release, so package publication does not imply that
  every direct CLI path is portable.
  ```

  Retain the list of package names and avoid hard-coding mutable npm version
  numbers.

- [ ] **Step 4: Inspect the edited entry-point diff**

  Run:

  ```bash
  git diff --check -- docs/GETTING-STARTED.md docs/start/coven-tui.md docs/start/onboarding.md docs/install/windows.md docs/platforms/windows.md docs/ARCHITECTURE.md
  ```

  Expected: no whitespace errors.

- [ ] **Step 5: Commit the entry-point reconciliation**

  ```bash
  git add docs/GETTING-STARTED.md docs/start/coven-tui.md docs/start/onboarding.md docs/install/windows.md docs/platforms/windows.md docs/ARCHITECTURE.md
  git commit -s -m "docs: reconcile interactive and Windows narratives" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
  ```

### Task 2: Establish cross-platform local IPC in daemon and API references

**Files:**
- Modify: `README.md:282-333`
- Modify: `docs/daemon/configuration.md:17-30`
- Modify: `docs/daemon/coven-home.md:20-40`
- Modify: `docs/daemon/index.md:10-40`
- Modify: `docs/daemon/socket-api.md:1-45,125-155`
- Modify: `docs/reference/api.md:1-45`
- Modify: `docs/reference/api-contract.md:57-75`
- Modify: `docs/API.md:1-35,64-84`
- Modify: `docs/API-CONTRACT.md:35-75`

- [ ] **Step 1: Add the canonical transport statement**

  Introduce this exact rule near the first transport mention in each daemon and
  API reference:

  ```markdown
  Coven serves the same versioned HTTP API over same-user local IPC: a Unix
  socket at `<covenHome>/coven.sock` on Unix-like hosts, or an owner-only named
  pipe for the selected `COVEN_HOME` on Windows. The health response and
  `coven daemon status` report the active endpoint; clients must not construct
  a Windows pipe path from the Unix socket convention.
  ```

  Update diagrams and health-response prose to label the endpoint `local IPC`
  rather than hard-code `<covenHome>/coven.sock`.

- [ ] **Step 2: Keep Unix examples explicitly platform-scoped**

  Precede every `curl --unix-socket`, Node `socketPath`, and Rust `UnixStream`
  example in `docs/daemon/socket-api.md` with:

  ```markdown
  These examples apply on Unix-like hosts. Windows clients must use a
  named-pipe-capable local IPC client and the endpoint reported by the daemon.
  ```

  Do not replace a valid Unix example with an invented Windows command.

- [ ] **Step 3: Update storage and daemon tables**

  Replace single `coven.sock` rows with two platform-specific rows:

  ```markdown
  | Unix-like hosts | `$COVEN_HOME/coven.sock` | Same-user Unix socket. |
  | Windows | daemon-reported named pipe | Owner-only local IPC endpoint; not a filesystem socket. |
  ```

  Keep the SQLite and `daemon.json` rows unchanged.

- [ ] **Step 4: Run the API documentation contract checks**

  Run:

  ```bash
  python3 scripts/check-api-contract-docs-test.py
  python3 scripts/check-api-contract-docs.py
  ```

  Expected: both commands exit successfully; route and payload documentation
  remains compatible with the checked API contract.

- [ ] **Step 5: Commit the transport reconciliation**

  ```bash
  git add README.md docs/daemon/configuration.md docs/daemon/coven-home.md docs/daemon/index.md docs/daemon/socket-api.md docs/reference/api.md docs/reference/api-contract.md docs/API.md docs/API-CONTRACT.md
  git commit -s -m "docs: describe cross-platform daemon IPC" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
  ```

### Task 3: Align trust and architecture documentation with local IPC

**Files:**
- Modify: `docs/AUTH.md:1-105`
- Modify: `docs/daemon/auth-posture.md:1-8`
- Modify: `docs/daemon/safety-model.md:10-52`
- Modify: `docs/daemon/trust-boundary.md:1-10`
- Modify: `docs/SAFETY-MODEL.md:35-64,108-117`
- Modify: `docs/OPERATIONAL-MODEL.md:15-65`
- Modify: `docs/CONCEPTS.md:105-155`
- Modify: `docs/concepts/architecture.md:15-42`
- Modify: `docs/concepts/features.md:10-28`
- Modify: `docs/concepts/runtime-topology.md:1-10`
- Modify: `docs/index.md:85-95,140-150`

- [ ] **Step 1: Replace generic Unix-only trust language**

  Update generic trust statements to:

  ```markdown
  The daemon accepts same-user local IPC only: a filesystem-permission-protected
  Unix socket on Unix-like hosts or an owner-only named pipe on Windows. It does
  not bind TCP by default.
  ```

  Keep Unix-specific filesystem permission, SSH, and `curl --unix-socket`
  instructions labeled as Unix-only. Do not change the external OpenClaw
  plugin's Unix-socket hardening requirements; label them as that plugin's
  current Unix integration contract.

- [ ] **Step 2: Update architecture diagrams and summaries**

  Change generic Mermaid edge labels from `HTTP over Unix socket` to
  `HTTP over same-user local IPC` and use `Local IPC` for generic endpoint
  nodes. Preserve Unix socket wording in pages explicitly documenting macOS,
  Linux, WSL2, cloud VM, or remote tunneling.

- [ ] **Step 3: Verify no generic canonical page still promises a Unix-only daemon**

  Run:

  ```bash
  rg -n 'HTTP over Unix socket|local Unix socket|<covenHome>/coven\.sock' README.md docs/{API,AUTH,API-CONTRACT,ARCHITECTURE,CONCEPTS,OPERATIONAL-MODEL,SAFETY-MODEL}.md docs/daemon/{auth-posture,index,safety-model,socket-api,trust-boundary}.md docs/concepts/{architecture,features,runtime-topology}.md docs/index.md
  ```

  Expected: remaining matches are explicitly Unix-scoped examples or plugin
  constraints, never the generic cross-platform daemon contract.

- [ ] **Step 4: Commit the trust-boundary updates**

  ```bash
  git add docs/AUTH.md docs/daemon/auth-posture.md docs/daemon/safety-model.md docs/daemon/trust-boundary.md docs/SAFETY-MODEL.md docs/OPERATIONAL-MODEL.md docs/CONCEPTS.md docs/concepts/architecture.md docs/concepts/features.md docs/concepts/runtime-topology.md docs/index.md
  git commit -s -m "docs: align local IPC trust boundaries" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
  ```

### Task 4: Reconcile adapter maturity and cancellation behavior

**Files:**
- Modify: `docs/HARNESS-ADAPTERS.md:1-45`
- Modify: `docs/ARCHITECTURE.md:35-75`
- Modify: `docs/concepts/architecture.md:28-42`
- Modify: `docs/concepts/features.md:10-28`
- Modify: `docs/reference/cli-kill.md:1-55`

- [ ] **Step 1: Publish one adapter maturity matrix**

  Add the following concise status table to `docs/HARNESS-ADAPTERS.md` after
  the opening overview:

  ```markdown
  | Maturity | Harnesses | Installation |
  | --- | --- | --- |
  | Bundled compatibility default | Codex, Claude Code, GitHub Copilot CLI | Install the harness CLI; no adapter recipe is needed. |
  | Trusted opt-in recipe | Hermes 1.0.3, OpenCode 0.1.1 | Install the upstream CLI, then install the named Coven recipe. |
  | Experimental opt-in recipe | Grok Build 1.0.0 | Install the upstream CLI, then install the recipe; its promotion checklist remains open. |
  | Future adapter direction | Aider, Gemini, Cline, custom adapters | No bundled or trusted recipe claim. |
  ```

  Preserve the existing Hermes documentation's exact 1.0.3 trusted-recipe
  wording and do not promote it to a bundled default.

- [ ] **Step 2: Update diagrams and capability cards**

  In the architecture and concept pages, create separate adapter nodes or
  labels for:

  ```text
  Bundled: Codex / Claude / Copilot
  Trusted recipes: Hermes / OpenCode
  Experimental recipe: Grok Build
  Future: Aider / Gemini / Cline / custom
  ```

  Remove Hermes from every generic "future adapter" label. Keep provider
  credentials and adapter invocation behavior unchanged.

- [ ] **Step 3: Document the Windows `coven kill` boundary**

  Add this section after the Usage block in `docs/reference/cli-kill.md`:

  ```markdown
  ## Platform support

  `coven kill` is currently implemented only on Unix-like hosts because the
  direct CLI request path uses a Unix socket. On Windows, do not expect this
  command to cancel a session. A Windows-capable local integration must ask the
  daemon that owns the session to cancel it through
  `POST /api/v1/sessions/:id/kill` over the daemon's owner-only named pipe.
  ```

  Retain the existing behavior description for successful Unix calls.

- [ ] **Step 4: Search for remaining stale maturity and UI claims**

  Run:

  ```bash
  rg -n 'prompt-first TUI|slash-command palette|Hermes / Aider|Hermes.*future|once the next Windows-enabled release|Windows in progress' README.md docs --glob '*.md' -g '!docs/MVP-PLAN.md' -g '!docs/PRODUCT-SPEC.md' -g '!docs/release-notes-unified-cli.md' -g '!docs/superpowers/**'
  ```

  Expected: no matches in active documentation. Historical release notes,
  product/MVP plans, and design records remain intentionally excluded.

- [ ] **Step 5: Commit adapter and cancellation documentation**

  ```bash
  git add docs/HARNESS-ADAPTERS.md docs/ARCHITECTURE.md docs/concepts/architecture.md docs/concepts/features.md docs/reference/cli-kill.md
  git commit -s -m "docs: clarify adapter maturity and cancellation" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
  ```

### Task 5: Run scoped documentation validation and prepare the issue update

**Files:**
- Modify: no additional files expected

- [ ] **Step 1: Confirm the intended documentation-only diff**

  Run:

  ```bash
  git diff origin/main...HEAD --check
  git diff --name-only origin/main...HEAD
  ```

  Expected: the design specification and only the documentation files named in
  Tasks 1-4 have changed.

- [ ] **Step 2: Run repository-required documentation-safe checks**

  Run:

  ```bash
  python3 scripts/check-api-contract-docs-test.py
  python3 scripts/check-api-contract-docs.py
  python scripts/check-secrets.py
  git add -A
  python3 scripts/check-coven-privacy.py --staged
  ```

  Expected: every command exits successfully. Do not run build or Rust test
  suites for this documentation-only change unless a documentation check
  exposes a source-contract failure.

- [ ] **Step 3: Review the complete diff against the five acceptance criteria**

  Run:

  ```bash
  git diff origin/main...HEAD -- README.md docs
  ```

  Confirm the diff:

  1. identifies `coven-code` as the managed default UI and the legacy TUI as
     deprecated opt-in;
  2. documents Unix sockets and Windows named pipes without inventing a stable
     Windows pathname;
  3. distinguishes Windows source capability, published platform package, and
     unsupported direct commands;
  4. classifies Codex, Claude Code, and Copilot as bundled; Hermes 1.0.3 and
     OpenCode 0.1.1 as trusted opt-in recipes; Grok Build 1.0.0 as
     experimental; and Aider, Gemini, Cline, and custom adapters as future
     direction, while retaining the OpenClaw plugin's explicit Unix-only
     trust-anchor constraint; and
  5. states the Windows direct-kill limitation and daemon cancellation route.
