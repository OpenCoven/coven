# Documentation Wave C: Daemon, CLI, and Reference Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish canonical daemon operations, CLI command, automation/JSON, and API usage guidance, then reduce remaining local public pages without weakening normative contracts.

**Architecture:** Add focused canonical daemon health, remote-access, and cloud-host pages; refresh CLI and API usage; add an automation/JSON guide; and strengthen canonical guards. The dependent cleanup keeps `API-CONTRACT.md`, lifecycle, safety, auth, adapter, settings, stream, and authority contracts local while converting public operational pages to concise canonical pointers.

**Tech Stack:** Fumadocs MDX, OpenAPI prose, Node.js docs guards, Python API-contract guards, Rust source verification, pnpm.

---

### Task 1: Create the Wave C canonical worktree

**Files:**
- No file changes.

- [ ] **Step 1: Coordinate and claim**

```bash
cd "$HOME/Documents/GitHub/OpenCoven/coven"
coven claim status
(cd "$HOME/Documents/GitHub/OpenCoven/coven-docs" && coven claim status)
gh pr list --repo OpenCoven/coven --state open
gh pr list --repo OpenCoven/coven-docs --state open
git -C "$HOME/Documents/GitHub/OpenCoven/coven-docs" fetch origin main
git -C "$HOME/Documents/GitHub/OpenCoven/coven-docs" worktree add \
  -b docs/670-wave-c-daemon-cli-reference \
  /tmp/coven-docs-670-wave-c \
  origin/main
cd /tmp/coven-docs-670-wave-c
coven claim acquire issue-670
```

### Task 2: Add failing daemon and CLI coverage

**Files:**
- Modify: `/tmp/coven-docs-670-wave-c/scripts/check-daemon-docs.mjs`
- Modify: `/tmp/coven-docs-670-wave-c/scripts/check-cli-docs.mjs`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/meta.json`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/guide/meta.json`

- [ ] **Step 1: Add navigation**

Add the daemon health page:

```json
"health"
```

Add `"automation-json"` to guide navigation after `"deployments"`. Wave C
starts only after Wave A is merged.

- [ ] **Step 2: Extend daemon required pages**

Add:

```js
const requiredPages = [
  'index',
  'lifecycle',
  'configuration',
  'health',
  'socket-api',
  'security',
  'observability',
  'recovery-upgrades',
];
```

Require `daemon/index.mdx` to link `/docs/daemon/health`.

- [ ] **Step 3: Extend CLI/source-backed assertions**

Add:

```js
[
  'coven run copilot',
  'coven sessions search',
  'coven sessions show',
  'coven sessions events',
  'coven sessions log',
  'coven reset',
  'coven maintenance',
  '--permission',
  '--add-dir',
  '--stream-json',
  '--stream-json-input',
  'coven config paths --json',
]
```

- [ ] **Step 4: Run guards and confirm failure**

```bash
pnpm run check:daemon-docs
pnpm run check:cli-docs
```

Expected: failures for missing pages, links, or command mentions.

- [ ] **Step 5: Commit**

```bash
git add scripts/check-daemon-docs.mjs scripts/check-cli-docs.mjs \
  content/docs/daemon/meta.json content/docs/guide/meta.json
git commit -s -m "test: require canonical daemon operations guidance" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Add canonical daemon health and configuration guidance

**Files:**
- Create: `/tmp/coven-docs-670-wave-c/content/docs/daemon/health.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/index.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/configuration.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/observability.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/recovery-upgrades.mdx`

- [ ] **Step 1: Create `health.mdx`**

Document the `/api/v1/health` compatibility handshake and operational blocks:

```mdx
## Compatibility

Require `apiVersion: "coven.daemon.v1"` before assuming response shapes.
Capabilities advertise availability; they do not grant authorization.

## Operational health

Interpret `eventWriter` and `storage` independently. Storage warnings include
retention lag and free-space pressure. Below the 256 MiB safety watermark the
daemon refuses SQLite open/write work; the 4 MiB threshold is the critical
replacement trigger for already-open storage.
```

Link to API, observability, recovery, and the source contract.

- [ ] **Step 2: Complete configuration facts**

Add `daemon.lock`, `daemon-serve.lock`, `daemon-recovery.log`,
`privacy.toml`, supported environment overrides, unsupported knob names, and
`coven config paths --json`.

- [ ] **Step 3: Update navigation and recovery links**

Add health cards/links from `index.mdx`, `observability.mdx`, and
`recovery-upgrades.mdx`.

- [ ] **Step 4: Validate and commit**

```bash
pnpm run check:daemon-docs
pnpm run check:links
git add content/docs/daemon scripts/check-daemon-docs.mjs
git commit -s -m "docs: canonicalize daemon health and configuration" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4: Add canonical remote and cloud-host operation

**Files:**
- Create: `/tmp/coven-docs-670-wave-c/content/docs/daemon/remote-access.mdx`
- Create: `/tmp/coven-docs-670-wave-c/content/docs/daemon/cloud-host-runbook.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/security.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/lifecycle.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/index.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/daemon/meta.json`
- Modify: `/tmp/coven-docs-670-wave-c/scripts/check-daemon-docs.mjs`

- [ ] **Step 1: Add remote-operation navigation and guard coverage**

Add `"remote-access"` and `"cloud-host-runbook"` after `"security"` in
`content/docs/daemon/meta.json`. Add both names to `requiredPages` in
`scripts/check-daemon-docs.mjs`, and require `daemon/index.mdx` to link:

```text
/docs/daemon/remote-access
/docs/daemon/cloud-host-runbook
```

- [ ] **Step 2: Write remote-access guidance**

State:

```mdx
- Prefer SSH and run the client as the same user that owns `COVEN_HOME`.
- Keep Unix sockets and Windows named pipes local.
- Unix-only TCP fallback must bind loopback unless an explicit trusted host is
  required.
- `--allow-host` narrows accepted Host/Origin values; it is not authentication.
- Do not expose the daemon directly through Tailscale or a public interface.
```

- [ ] **Step 3: Write the cloud-host runbook**

Include systemd user service setup, loopback binding, SSH/Tailscale access to
the host rather than the daemon port, persistent `COVEN_HOME`, and verification
with `coven daemon status --json`.

- [ ] **Step 4: Validate and commit**

```bash
pnpm run check:daemon-docs
pnpm run check:links
git add content/docs/daemon scripts/check-daemon-docs.mjs
git commit -s -m "docs: add daemon remote and cloud-host runbooks" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: Refresh CLI, automation, and API usage

**Files:**
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/cli/index.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/cli/daemon.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/cli/doctor.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/cli/run.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/cli/sessions.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/cli/repo-workflow.mdx`
- Create: `/tmp/coven-docs-670-wave-c/content/docs/guide/automation-json.mdx`
- Modify: `/tmp/coven-docs-670-wave-c/content/docs/reference/api.mdx`

- [ ] **Step 1: Add complete current command coverage**

Document:

```bash
coven run copilot "review this repository"
coven sessions search "query"
coven sessions show SESSION_ID
coven sessions events SESSION_ID
coven sessions log SESSION_ID
coven reset --list-features
coven maintenance status --json
coven config paths --json
```

Cover `--permission`, `--add-dir`, `--stream-json`,
`--stream-json-input`, and `--json` using source-verified spelling.

- [ ] **Step 2: Add automation/JSON guidance**

Create `automation-json.mdx` with:

```bash
coven doctor --json
coven daemon status --json
coven sessions --json
```

Explain stable structured fields versus human prose, process exit status,
same-user local IPC, and the boundary between CLI automation and the daemon
API.

- [ ] **Step 3: Correct API routing prose**

Ensure `reference/api.mdx` states:

```text
POST /api/v1/actions
```

with the action identifier in JSON field `action`. Do not document
`POST /api/v1/actions/{id}`.

- [ ] **Step 4: Validate and commit**

```bash
pnpm run check:cli-docs
pnpm run check:links
pnpm build
git add content/docs/cli content/docs/guide content/docs/reference/api.mdx \
  scripts/check-cli-docs.mjs
git commit -s -m "docs: complete canonical CLI and API usage" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: Merge the canonical Wave C PR

**Files:**
- No new changes.

- [ ] **Step 1: Push and open**

```bash
git push -u origin docs/670-wave-c-daemon-cli-reference
gh pr create --repo OpenCoven/coven-docs \
  --title "docs: complete daemon CLI and API guidance" \
  --body "Implements Wave C of OpenCoven/coven#670. Adds canonical daemon health, remote access, cloud-host operation, CLI automation, session inspection, and API usage guidance. The dependent coven cleanup must merge second."
```

- [ ] **Step 2: Wait for all checks and merge**

```bash
gh pr checks --repo OpenCoven/coven-docs --watch
```

Expected: all required checks pass.

### Task 7: Update local contract and pointer guards

**Files:**
- Modify: `/tmp/coven-670-wave-c/scripts/check-api-contract-docs.py`
- Modify: `/tmp/coven-670-wave-c/scripts/check-api-contract-docs-test.py`
- Modify: `/tmp/coven-670-wave-c/scripts/cli-docs-test.mjs`
- Modify: `/tmp/coven-670-wave-c/scripts/onboarding-docs-test.mjs`

- [ ] **Step 1: Create the cleanup worktree**

```bash
cd /tmp/coven-docs-670-wave-c
coven claim release issue-670
git -C "$HOME/Documents/GitHub/OpenCoven/coven" fetch origin main
git -C "$HOME/Documents/GitHub/OpenCoven/coven" worktree add \
  -b docs/670-wave-c-daemon-cli-reference-cleanup \
  /tmp/coven-670-wave-c \
  origin/main
cd /tmp/coven-670-wave-c
coven claim acquire issue-670
```

- [ ] **Step 2: Preserve mandatory API guardrails**

Keep assertions that `docs/API.md` contains:

```text
/api/v1/health
coven.daemon.v1
capabilities are not authorization
/api/v1/api-version
legacy
```

Change long-form daemon/CLI page assertions to canonical pointer assertions.

- [ ] **Step 3: Run and confirm failure**

```bash
python3 scripts/check-api-contract-docs.py
python3 -m unittest scripts/check-api-contract-docs-test.py
node scripts/cli-docs-test.mjs
node scripts/onboarding-docs-test.mjs
```

Expected: failures until the local public pages are reduced.

- [ ] **Step 4: Commit**

```bash
git add scripts/check-api-contract-docs.py scripts/check-api-contract-docs-test.py \
  scripts/cli-docs-test.mjs scripts/onboarding-docs-test.mjs
git commit -s -m "test: enforce local contract and canonical usage split" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 8: Reduce remaining daemon, CLI, guide, and API usage pages

**Files:**
- Modify: `/tmp/coven-670-wave-c/docs/API.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/index.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/configuration.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/diagnostics.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/health.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/lifecycle.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/logs.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/socket-api.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/remote-access.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/cloud-host-runbook.md`
- Modify: `/tmp/coven-670-wave-c/docs/daemon/upgrades.md`
- Modify: `/tmp/coven-670-wave-c/docs/guides/core-access.md`
- Modify: `/tmp/coven-670-wave-c/docs/guides/index.md`
- Modify: `/tmp/coven-670-wave-c/docs/guides/automation-json.md`
- Modify: `/tmp/coven-670-wave-c/docs/guides/multi-agent-worktrees.md`
- Modify: `/tmp/coven-670-wave-c/docs/guides/session-operations.md`
- Modify: `/tmp/coven-670-wave-c/docs/guides/troubleshooting-core-access.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/api.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/api-actions.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/api-capabilities.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/api-contract.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/api-events.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/api-sessions.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-archive.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-attach.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-claim.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-config.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-coven.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-daemon.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-doctor.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-engine.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-executor.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-kill.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-logs.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-maintenance.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-observe.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-patch.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-reset.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-run.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-sacrifice.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-sessions.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-summon.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-vacuum.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-ward.md`
- Modify: `/tmp/coven-670-wave-c/docs/reference/cli-wt.md`
- Modify: `/tmp/coven-670-wave-c/README.md`

- [ ] **Step 1: Keep normative documents untouched**

Do not reduce:

```text
docs/API-CONTRACT.md
docs/AUTH.md
docs/SAFETY-MODEL.md
docs/SESSION-LIFECYCLE.md
docs/HARNESS-ADAPTERS.md
docs/SETTINGS.md
docs/STREAM-JSON.md
docs/daemon/auth-posture.md
docs/daemon/coven-home.md
docs/daemon/safety-model.md
docs/daemon/trust-boundary.md
docs/daemon/session-handoff.md
docs/development/cli-core-functionality.md
```

- [ ] **Step 2: Reduce operational pages to pointers**

Use exact canonical destinations:

```text
daemon health -> https://docs.opencoven.ai/docs/daemon/health
daemon configuration -> https://docs.opencoven.ai/docs/daemon/configuration
daemon diagnostics and logs -> https://docs.opencoven.ai/docs/daemon/observability
daemon remote access -> https://docs.opencoven.ai/docs/daemon/remote-access
cloud host -> https://docs.opencoven.ai/docs/daemon/cloud-host-runbook
daemon upgrades -> https://docs.opencoven.ai/docs/daemon/recovery-upgrades
automation JSON -> https://docs.opencoven.ai/docs/guide/automation-json
CLI usage -> https://docs.opencoven.ai/docs/cli
API usage -> https://docs.opencoven.ai/docs/reference/api
```

`docs/API.md` must remain a short pointer plus the required handshake,
capability/authorization, and legacy-route guardrails.

Use these command-page mappings:

```text
cli-coven -> /docs/cli/interactive
cli-daemon -> /docs/cli/daemon
cli-doctor -> /docs/cli/doctor
cli-run -> /docs/cli/run
cli-archive, cli-attach, cli-kill, cli-sacrifice, cli-sessions, cli-summon -> /docs/cli/sessions
cli-observe, cli-logs, cli-vacuum -> /docs/cli/observe
cli-executor -> /docs/cli/hub-scheduler
cli-engine -> /docs/cli/engine-auth
cli-claim, cli-maintenance, cli-ward, cli-wt -> /docs/cli/repo-workflow
cli-patch -> /docs/cli/patch-openclaw
cli-config -> /docs/daemon/configuration
cli-reset -> /docs/cli
```

Use these guide mappings:

```text
guides/index, guides/core-access -> /docs/guide/getting-started
guides/session-operations -> /docs/cli/sessions
guides/automation-json -> /docs/guide/automation-json
guides/multi-agent-worktrees -> /docs/cli/repo-workflow
guides/troubleshooting-core-access -> /docs/reference/troubleshooting
```

In `README.md`, replace the local `cli-logs.md` pruning link with
`https://docs.opencoven.ai/docs/cli/observe` and the local
`cli-reset.md` link with `https://docs.opencoven.ai/docs/cli`.

- [ ] **Step 3: Correct version-sensitive local wording**

In `docs/reference/future-harnesses.md`, remove a pinned Hermes version and use:

```md
Hermes is a trusted installable recipe. Verify the current recipe and release
before documenting a version number.
```

- [ ] **Step 4: Validate and commit**

```bash
python3 scripts/check-api-contract-docs.py
python3 -m unittest scripts/check-api-contract-docs-test.py
node scripts/cli-docs-test.mjs
node scripts/onboarding-docs-test.mjs
python3 scripts/check-secrets.py
git diff --check
git add -A
python3 scripts/check-coven-privacy.py --staged
git commit -s -m "docs: point daemon CLI and API usage canonical" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: all commands pass.

### Task 9: Final cross-repository audit and goal closure

**Files:**
- Update after merge, outside the Git branch: primary Coven checkout `.copilot/goals.md`

- [ ] **Step 1: Search for residual canonicality violations**

Run:

```bash
local_public_doc_link='\]\((?:https://github\.com/OpenCoven/coven/blob/main/)?docs/(?:install|platforms|harnesses|help|daemon|reference/cli)[^)]*\)|\]\(/(?:install|platforms|harnesses|help|daemon|reference/cli)[^)]*\)'
rg -n "$local_public_doc_link" \
  README.md CONTRIBUTING.md docs/index.md docs/GETTING-STARTED.md \
  docs/TROUBLESHOOTING.md .github/ISSUE_TEMPLATE
```

Expected: no matches. Current public entry points must not send users to local
copies. Retained normative contracts, guard scripts, and historical
`docs/superpowers/**` records are intentionally outside this search.

- [ ] **Step 2: Run complete repository gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
COVEN_NPM_DRY_RUN_VERSION=999.0.0 \
  node scripts/test-cli-prepublish.mjs --target=macos
python3 scripts/check-secrets.py
git diff --check
```

- [ ] **Step 3: Push and open the final cleanup PR**

```bash
git push -u origin docs/670-wave-c-daemon-cli-reference-cleanup
gh pr create --repo OpenCoven/coven \
  --title "docs: complete canonical documentation migration" \
  --body "Completes Wave C and closes #670. Depends on the merged canonical coven-docs Wave C PR. Retains normative source contracts while making docs.opencoven.ai authoritative for daemon operation, CLI usage, automation, and public API guidance."
```

- [ ] **Step 4: Merge and clean up**

After full CI and review, squash merge with `Closes #670`, release the claim,
delete remote branches, and remove the Wave C worktrees. Then, from the primary
Coven checkout, move `documentation-single-source` from `## active` to
`## done` in `.copilot/goals.md`, set the completion date, and record the six
merged Wave A/B/C PRs and issue `#670`.
