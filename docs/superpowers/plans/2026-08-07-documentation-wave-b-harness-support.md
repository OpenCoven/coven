# Documentation Wave B: Harness and Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `coven-docs` authoritative for harness setup, troubleshooting, diagnostics, environment, permissions, paths, community support, issue filing, and memory import, then remove the duplicate public help pages from `coven`.

**Architecture:** Expand existing harness and troubleshooting pages, add one canonical support hub and one memory-import page, and enforce them with existing documentation guards. After the canonical PR merges, delete public help leaves, reduce harness/model overviews to pointers, and retain `docs/HARNESS-ADAPTERS.md` plus implementation-specific adapter notes.

**Tech Stack:** Fumadocs MDX, Node.js documentation guards, YAML issue templates, Rust CLI source verification, pnpm.

---

### Task 1: Create the Wave B canonical worktree

**Files:**
- No file changes.

- [ ] **Step 1: Coordinate and claim**

```bash
cd "$HOME/Documents/GitHub/OpenCoven/coven"
coven claim status
gh pr list --repo OpenCoven/coven --state open
gh pr list --repo OpenCoven/coven-docs --state open
git -C "$HOME/Documents/GitHub/OpenCoven/coven-docs" fetch origin main
git -C "$HOME/Documents/GitHub/OpenCoven/coven-docs" worktree add \
  -b docs/670-wave-b-harness-support \
  /tmp/coven-docs-670-wave-b \
  origin/main
cd /tmp/coven-docs-670-wave-b
coven claim acquire issue-670
```

Expected: no duplicate Wave B work and an active claim.

### Task 2: Add failing harness, support, and memory guards

**Files:**
- Modify: `/tmp/coven-docs-670-wave-b/scripts/check-harness-docs.mjs`
- Modify: `/tmp/coven-docs-670-wave-b/scripts/check-memory-models-docs.mjs`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/reference/meta.json`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/memory-models/meta.json`

- [ ] **Step 1: Add canonical navigation entries**

Add `"support"` to `content/docs/reference/meta.json` after
`"troubleshooting"`. Add `"memory-import"` to
`content/docs/memory-models/meta.json` after `"memory"`.

- [ ] **Step 2: Strengthen harness assertions**

Add these exact source-backed requirements:

```js
const builtInHarnesses = ['codex', 'claude', 'coven-code', 'copilot'];
const optInRecipes = ['grok', 'hermes'];
```

Require all built-ins in the main support table, require `grok` and `hermes`
to be described as installable recipes, and reject wording that calls either
one bundled.

- [ ] **Step 3: Require memory import**

Add `'memory-import'` to `requiredPages` and add:

```js
[
  'coven memory import',
  'coven memory restore',
  '--json',
  '--source openclaw',
  '--openclaw-root',
  'preview',
]
```

to `requiredMentions`.

- [ ] **Step 4: Run guards and confirm failure**

```bash
pnpm run check:harness-docs
pnpm run check:memory-models-docs
```

Expected: failures for missing `support.mdx`, `memory-import.mdx`, or required
phrases.

- [ ] **Step 5: Commit**

```bash
git add scripts/check-harness-docs.mjs scripts/check-memory-models-docs.mjs \
  content/docs/reference/meta.json content/docs/memory-models/meta.json
git commit -m "test: require canonical harness support guidance" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Expand canonical harness setup and recovery

**Files:**
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/harnesses/installing.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/harnesses/troubleshooting.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/harnesses/provider-auth.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/harnesses/project-roots.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/harnesses/working-directories.mdx`

- [ ] **Step 1: Document the supported harness classes**

Use this exact distinction:

```mdx
## Bundled harness ids

`codex`, `claude`, `coven-code`, and `copilot` are built-in adapters.

## Trusted installable recipes

`grok` and `hermes` are opt-in recipes installed through `coven adapter
install`; they are not bundled defaults.
```

Keep OpenClaw documented as an external bridge rather than a built-in harness.

- [ ] **Step 2: Add deterministic recovery**

Document:

```bash
coven doctor
codex login
claude doctor
copilot login
coven engine install
coven adapter install grok
coven adapter install hermes
```

Explain same-shell PATH/auth checks, project-root refusal, working-directory
validation, and PTY requirements.

- [ ] **Step 3: Run and commit**

```bash
pnpm run check:harness-docs
git add content/docs/harnesses scripts/check-harness-docs.mjs
git commit -m "docs: complete canonical harness recovery guidance" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: harness guard passes.

### Task 4: Add the canonical support hub

**Files:**
- Create: `/tmp/coven-docs-670-wave-b/content/docs/reference/support.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/reference/troubleshooting.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/cli/doctor.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/daemon/configuration.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/daemon/security.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/daemon/observability.mdx`

- [ ] **Step 1: Create `support.mdx`**

Use frontmatter and these sections:

```mdx
## Start with diagnostics

Run `coven doctor`, `coven daemon status --json`, and the failing command with
the smallest reproducible project.

## Collect a safe report

Include platform, install route, Coven version, command, expected behavior, and
redacted diagnostics. Do not include provider tokens, private prompts, raw
environment dumps, or sensitive absolute paths.

## Community and issue filing

- GitHub issues: https://github.com/OpenCoven/coven/issues
- Discord: https://discord.gg/opencoven
```

Link to troubleshooting, doctor, observability, and security pages.

- [ ] **Step 2: Move environment, path, permission, and diagnostics facts**

Add:

- `NO_COLOR`, `COLORTERM`, `TERM`, `COVEN_HOME`, and
  `coven config paths --json` to configuration/doctor guidance;
- same-user ownership and permission repair guidance to security;
- diagnostic bundle/redaction rules to observability;
- harness missing, daemon startup, and stuck-session routes to troubleshooting.

- [ ] **Step 3: Validate and commit**

```bash
pnpm run check:links
pnpm build
git add content/docs/reference content/docs/cli/doctor.mdx \
  content/docs/daemon
git commit -m "docs: add canonical support and diagnostics hub" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: build and links pass.

### Task 5: Add the canonical memory-import workflow

**Files:**
- Create: `/tmp/coven-docs-670-wave-b/content/docs/memory-models/memory-import.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/memory-models/index.mdx`
- Modify: `/tmp/coven-docs-670-wave-b/content/docs/memory-models/memory.mdx`

- [ ] **Step 1: Write the workflow**

Document this sequence:

```bash
coven memory import --source openclaw --openclaw-root /path/to/openclaw
coven memory import --source openclaw --openclaw-root /path/to/openclaw --json
coven memory restore
```

State that import previews before applying, private bundle content must not be
committed, and restore is logical rather than a destructive filesystem rewind.

- [ ] **Step 2: Link the page**

Add `/docs/memory-models/memory-import` from `index.mdx` and `memory.mdx`.

- [ ] **Step 3: Validate and commit**

```bash
pnpm run check:memory-models-docs
pnpm run check:links
pnpm build
git add content/docs/memory-models scripts/check-memory-models-docs.mjs
git commit -m "docs: canonicalize memory import and restore" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: Merge the canonical Wave B PR

**Files:**
- No new changes.

- [ ] **Step 1: Push and open**

```bash
git push -u origin docs/670-wave-b-harness-support
gh pr create --repo OpenCoven/coven-docs \
  --title "docs: canonicalize harness and support guidance" \
  --body "Implements Wave B of OpenCoven/coven#670. Adds canonical harness recovery, support, diagnostics, issue-filing, environment, permissions, paths, and memory-import guidance. The dependent coven cleanup must merge second."
```

- [ ] **Step 2: Wait for checks and merge**

```bash
gh pr checks --repo OpenCoven/coven-docs --watch
```

Expected: all required checks pass before squash merge.

### Task 7: Add failing local ownership checks

**Files:**
- Modify: `/tmp/coven-670-wave-b/scripts/onboarding-docs-test.mjs`
- Modify: `/tmp/coven-670-wave-b/scripts/cli-docs-test.mjs`

- [ ] **Step 1: Create the cleanup worktree**

```bash
cd /tmp/coven-docs-670-wave-b
coven claim release issue-670
git -C "$HOME/Documents/GitHub/OpenCoven/coven" fetch origin main
git -C "$HOME/Documents/GitHub/OpenCoven/coven" worktree add \
  -b docs/670-wave-b-harness-support-cleanup \
  /tmp/coven-670-wave-b \
  origin/main
cd /tmp/coven-670-wave-b
coven claim acquire issue-670
```

- [ ] **Step 2: Replace obsolete substantive-help assertions**

Remove deleted help leaves from `criticalDocs`. Add:

```js
test('public support routes point to canonical docs', () => {
  assert.match(readRepoFile('docs/help/index.md'), /https:\/\/docs\.opencoven\.ai\/docs\/reference\/support/);
  assert.match(readRepoFile('.github/ISSUE_TEMPLATE/bug-report.yml'), /https:\/\/docs\.opencoven\.ai\/docs\/reference\/support/);
});
```

In `cli-docs-test.mjs`, require harness/model index pages to link to
`https://docs.opencoven.ai/docs/harnesses` and
`https://docs.opencoven.ai/docs/memory-models/provider-boundary`.

- [ ] **Step 3: Run and confirm failure**

```bash
node scripts/onboarding-docs-test.mjs
node scripts/cli-docs-test.mjs
```

Expected: failures because local pages and the issue template still use local
public routes.

- [ ] **Step 4: Commit**

```bash
git add scripts/onboarding-docs-test.mjs scripts/cli-docs-test.mjs
git commit -m "test: require canonical harness support links" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 8: Remove duplicate help leaves and reduce harness/model pages

**Files:**
- Modify: `/tmp/coven-670-wave-b/docs/help/index.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/community.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/daemon-wont-start.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/diagnostics-bundle.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/environment.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/filing-issues.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/harness-not-found.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/memory-import.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/paths.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/permissions.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/session-stuck.md`
- Delete: `/tmp/coven-670-wave-b/docs/help/troubleshooting.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/claude-code.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/codex.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/copilot-cli.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/index.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/installing.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/project-root.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/provider-auth.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/troubleshooting.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/what-is-a-harness.md`
- Modify: `/tmp/coven-670-wave-b/docs/harnesses/working-directory.md`
- Modify: `/tmp/coven-670-wave-b/docs/models/index.md`
- Modify: `/tmp/coven-670-wave-b/docs/models/provider-boundary.md`
- Modify: `/tmp/coven-670-wave-b/docs/models/why-coven-does-not-own-credentials.md`
- Modify: `/tmp/coven-670-wave-b/.github/ISSUE_TEMPLATE/bug-report.yml`
- Modify: inbound links reported by `rg 'docs/help/|/help/' docs README.md .github scripts`

- [ ] **Step 1: Preserve local contracts**

Do not reduce:

```text
docs/HARNESS-ADAPTERS.md
docs/harnesses/grok-build.md
docs/harnesses/hermes.md
docs/harnesses/openclaw.md
docs/harnesses/opencode.md
docs/harnesses/custom.md
docs/reference/harness-adapters.md
```

- [ ] **Step 2: Replace index pages with pointers**

`docs/help/index.md` must point to:

```md
https://docs.opencoven.ai/docs/reference/support
https://docs.opencoven.ai/docs/reference/troubleshooting
https://docs.opencoven.ai/docs/cli/doctor
```

Harness public pages point to these corresponding routes:

```text
index -> https://docs.opencoven.ai/docs/harnesses
what-is-a-harness -> https://docs.opencoven.ai/docs/harnesses/what-is-a-harness
installing -> https://docs.opencoven.ai/docs/harnesses/installing
provider-auth -> https://docs.opencoven.ai/docs/harnesses/provider-auth
project-root -> https://docs.opencoven.ai/docs/harnesses/project-roots
working-directory -> https://docs.opencoven.ai/docs/harnesses/working-directories
codex -> https://docs.opencoven.ai/docs/harnesses/codex
claude-code -> https://docs.opencoven.ai/docs/harnesses/claude-code
copilot-cli -> https://docs.opencoven.ai/docs/harnesses/copilot
troubleshooting -> https://docs.opencoven.ai/docs/harnesses/troubleshooting
```

All three model/provider pages point to
`https://docs.opencoven.ai/docs/memory-models/provider-boundary`.

- [ ] **Step 3: Retarget inbound links**

Update the bug-report template and current documentation links to canonical
support routes. Historical implementation plans may retain descriptive text,
but links used as current instructions must become canonical.

- [ ] **Step 4: Validate and commit**

```bash
node scripts/onboarding-docs-test.mjs
node scripts/cli-docs-test.mjs
python3 scripts/check-secrets.py
git diff --check
git add -A
python3 scripts/check-coven-privacy.py --staged
git commit -m "docs: point harness and support guidance canonical" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: all commands pass.

### Task 9: Validate and merge Wave B cleanup

**Files:**
- No additional changes expected.

- [ ] **Step 1: Run the full applicable gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
COVEN_NPM_DRY_RUN_VERSION=999.0.0 \
  node scripts/test-cli-prepublish.mjs --target=macos
python3 scripts/check-secrets.py
git diff --check
```

- [ ] **Step 2: Push and open**

```bash
git push -u origin docs/670-wave-b-harness-support-cleanup
gh pr create --repo OpenCoven/coven \
  --title "docs: point harness and support guidance canonical" \
  --body "Implements Wave B cleanup for #670 after the canonical coven-docs Wave B merge. Retains normative adapter contracts while removing duplicate public harness, help, diagnostics, and memory-import guidance."
```

- [ ] **Step 3: Merge after review and full CI**

Release `issue-670` and remove both Wave B worktrees after merge.
