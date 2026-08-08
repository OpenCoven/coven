# Documentation Wave A: Install and Platform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the remaining public installation, platform, deployment, and onboarding guidance into `coven-docs`, then reduce the corresponding `coven` pages and runtime links to canonical pointers.

**Architecture:** Land a canonical `coven-docs` PR first, organized around install methods, platform notes, deployments, onboarding, and uninstall/debugging. After that PR merges, land a dependent `coven` cleanup that preserves the local state-layout and legacy-TUI contracts while converting public pages, tests, and runtime help to `docs.opencoven.ai`.

**Tech Stack:** Fumadocs MDX, Node.js documentation guards, Rust CLI smoke tests, GitHub CLI, pnpm, Cargo.

---

### Task 1: Create isolated Wave A worktrees

**Files:**
- No file changes.

- [ ] **Step 1: Check for duplicate work**

Run:

```bash
cd "$HOME/Documents/GitHub/OpenCoven/coven"
coven claim status
(cd "$HOME/Documents/GitHub/OpenCoven/coven-docs" && coven claim status)
gh pr list --repo OpenCoven/coven --state open
gh pr list --repo OpenCoven/coven-docs --state open
```

Expected: no active claim or open PR implementing issue `#670` Wave A.

- [ ] **Step 2: Create the canonical worktree and claim**

Run:

```bash
git -C "$HOME/Documents/GitHub/OpenCoven/coven-docs" fetch origin main
git -C "$HOME/Documents/GitHub/OpenCoven/coven-docs" worktree add \
  -b docs/670-wave-a-install-platform \
  /tmp/coven-docs-670-wave-a \
  origin/main
cd /tmp/coven-docs-670-wave-a
coven claim acquire issue-670
```

Expected: worktree at `/tmp/coven-docs-670-wave-a` and an active `issue-670` claim.

### Task 2: Add failing canonical install coverage

**Files:**
- Modify: `/tmp/coven-docs-670-wave-a/scripts/check-cli-docs.mjs`
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/guide/meta.json`

- [ ] **Step 1: Extend the guide navigation**

Add `platforms` and `deployments` after `install`:

```json
{
  "title": "Guide",
  "description": "Start with Coven's local runtime",
  "root": true,
  "icon": "LuBookOpen",
  "pages": [
    "getting-started",
    "install",
    "platforms",
    "deployments",
    "concepts",
    "architecture"
  ]
}
```

- [ ] **Step 2: Add exact install assertions**

In `scripts/check-cli-docs.mjs`, add `install-debugging` and `uninstall` to
`requiredPages`, then read the three guide pages and require these source-backed
phrases:

```js
const guideRoot = join(docsRoot, 'guide');
const requiredGuidePages = ['install', 'platforms', 'deployments'];
const requiredInstallMentions = [
  'npm install -g @opencoven/cli',
  'cargo install --path crates/coven-cli',
  'Apple Silicon',
  'glibc',
  'PowerShell',
  'WSL2',
  'Linux filesystem',
  'launchd',
  'systemd',
  'Docker',
  'Podman',
  'Nix',
  'Raspberry Pi',
  'COVEN_HOME',
  'coven doctor',
  'coven daemon status',
];
```

Iterate over `requiredGuidePages`, fail if a page is absent or lacks
`read_when:`, and fail when any `requiredInstallMentions` value is absent from
the joined guide source.

- [ ] **Step 3: Run the guard and confirm it fails**

Run:

```bash
cd /tmp/coven-docs-670-wave-a
pnpm run check:cli-docs
```

Expected: failure naming missing `platforms.mdx`, `deployments.mdx`, or required
install phrases.

- [ ] **Step 4: Commit the failing guard**

```bash
git add scripts/check-cli-docs.mjs content/docs/guide/meta.json
git commit -s -m "test: require canonical platform install guidance" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Canonicalize install methods and platform behavior

**Files:**
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/guide/install.mdx`
- Create: `/tmp/coven-docs-670-wave-a/content/docs/guide/platforms.mdx`
- Create: `/tmp/coven-docs-670-wave-a/content/docs/guide/deployments.mdx`
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/cli/install.mdx`

- [ ] **Step 1: Expand the install landing page**

Keep the existing frontmatter and add:

```mdx
## Choose an install route

| Route | Best for | Verification |
| --- | --- | --- |
| `npm install -g @opencoven/cli` | Published platform wrapper | `coven doctor` |
| `cargo install --path crates/coven-cli` | Source checkout contributors | `coven --version` |
| `cargo build --workspace` | Active development | `cargo test --workspace --locked` |

Use the npm wrapper for normal installs. Use source routes only when the
published package does not cover the target or when contributing to Coven.
```

Link to `/docs/guide/platforms`, `/docs/guide/deployments`,
`/docs/cli/install-debugging`, and `/docs/cli/uninstall`.

- [ ] **Step 2: Create the platform page**

Create `content/docs/guide/platforms.mdx` with frontmatter and sections:

```mdx
## macOS

- Apple Silicon uses the native macOS package.
- Use `launchd` only when you need login-time daemon startup.

## Linux

- Published Linux packages target glibc-based x64 systems.
- Use `systemd --user` for persistent same-user daemon operation.

## Windows

- Run install and verification from PowerShell.
- The daemon uses an owner-only named pipe selected from `COVEN_HOME`.
- WSL2 is a separate Linux environment; do not mix Windows and WSL state.

## WSL2

- Keep repositories and `COVEN_HOME` on the Linux filesystem when possible.
- Avoid `/mnt/c` for daemon-heavy workloads.

## Raspberry Pi

- Use a 64-bit operating system.
- Build from source when no published package matches the architecture.
```

Include `coven doctor`, `coven daemon status`, and links to daemon
configuration and install debugging.

- [ ] **Step 3: Create the deployment page**

Create `content/docs/guide/deployments.mdx` with:

```mdx
## Headless and cloud hosts

Use SSH as the same operating-system user that owns `COVEN_HOME`. Keep daemon
IPC local; do not expose it directly.

## Docker and Podman

Mount a persistent `COVEN_HOME`, keep the daemon and client in the same trust
boundary, and verify with `coven doctor`.

## Nix

Treat Nix as an environment/build route, not a separate runtime contract.

## Service managers

Use `launchd` on macOS and `systemd --user` on Linux. Both must preserve the
same `COVEN_HOME` and PATH used by the interactive shell.
```

Link remote-access concerns to `/docs/daemon/security`.

- [ ] **Step 4: Expand the CLI install reference**

In `content/docs/cli/install.mdx`, add exact npm, Cargo, and source-checkout
commands, plus a platform package note. Do not promise support for an
architecture unless a package exists in the current release workflow.

- [ ] **Step 5: Run the guard**

```bash
pnpm run check:cli-docs
```

Expected: `CLI docs check passed.`

- [ ] **Step 6: Commit**

```bash
git add content/docs/guide content/docs/cli/install.mdx
git commit -s -m "docs: canonicalize install and platform guidance" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4: Canonicalize onboarding, debugging, updates, and uninstall

**Files:**
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/guide/getting-started.mdx`
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/cli/interactive.mdx`
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/cli/install-debugging.mdx`
- Modify: `/tmp/coven-docs-670-wave-a/content/docs/cli/uninstall.mdx`
- Modify: `/tmp/coven-docs-670-wave-a/scripts/check-cli-docs.mjs`

- [ ] **Step 1: Add source-backed onboarding behavior**

Document that the default interactive path uses the managed `coven-code`
engine, while `COVEN_LEGACY_TUI=1` is a deprecated compatibility escape hatch.
Keep first-session verification:

```bash
coven doctor
coven daemon start
coven run codex "explain this repo in 5 bullets"
coven sessions
```

- [ ] **Step 2: Add debugging and update recovery**

Ensure `install-debugging.mdx` includes:

```bash
# Unix-like shells
npm view @opencoven/cli version
which -a coven
rustup update stable
cargo build --workspace
```

Add the native PowerShell equivalent:

```powershell
Get-Command -All coven
```

Explain PATH refresh, wrapper/native mismatch, Windows/WSL separation, and
rollback by reinstalling a verified prior package version rather than copying
binaries between platforms. Label `which -a coven` as Unix-like-only.

- [ ] **Step 3: Add uninstall behavior**

Ensure `uninstall.mdx` says to run `coven daemon stop`, remove the npm or Cargo
installation, and preserve `~/.coven` by default. Destructive state removal
must be an explicit final step.

- [ ] **Step 4: Strengthen the guard**

Add required mentions:

```js
[
  'COVEN_LEGACY_TUI=1',
  'npm view @opencoven/cli version',
  'which -a coven',
  'Get-Command -All coven',
  'rustup update stable',
  'coven daemon stop',
  'cargo uninstall coven-cli',
]
```

- [ ] **Step 5: Validate and commit**

```bash
pnpm run check:cli-docs
pnpm run check:links
pnpm build
git add content/docs/guide/getting-started.mdx content/docs/cli scripts/check-cli-docs.mjs
git commit -s -m "docs: complete canonical onboarding lifecycle" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: all commands pass.

### Task 5: Merge the canonical Wave A PR

**Files:**
- No new file changes.

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin docs/670-wave-a-install-platform
gh pr create --repo OpenCoven/coven-docs \
  --title "docs: canonicalize install and platform guidance" \
  --body "Implements Wave A of OpenCoven/coven#670. Adds canonical install methods, platform notes, deployments, onboarding, debugging, updates, and uninstall guidance. The dependent coven cleanup must merge after this PR."
```

- [ ] **Step 2: Wait for review and required checks**

Run:

```bash
gh pr checks --repo OpenCoven/coven-docs --watch
```

Expected: all required checks pass.

- [ ] **Step 3: Merge**

Use a squash commit with the repository-required Copilot trailer.

### Task 6: Create the dependent `coven` cleanup worktree

**Files:**
- No file changes.

- [ ] **Step 1: Release the docs claim and acquire it in `coven`**

```bash
cd /tmp/coven-docs-670-wave-a
coven claim release issue-670
git -C "$HOME/Documents/GitHub/OpenCoven/coven" fetch origin main
git -C "$HOME/Documents/GitHub/OpenCoven/coven" worktree add \
  -b docs/670-wave-a-install-platform-cleanup \
  /tmp/coven-670-wave-a \
  origin/main
cd /tmp/coven-670-wave-a
coven claim acquire issue-670
```

### Task 7: Change stale local documentation guards first

**Files:**
- Modify: `/tmp/coven-670-wave-a/scripts/onboarding-docs-test.mjs`
- Modify: `/tmp/coven-670-wave-a/crates/coven-cli/tests/smoke.rs`

- [ ] **Step 1: Replace substantive platform-page assertions**

Replace `platformDocs` with canonical pointer expectations:

```js
const platformDocs = [
  'docs/platforms/macos.md',
  'docs/platforms/linux.md',
  'docs/platforms/windows.md',
  'docs/platforms/wsl2.md',
  'docs/platforms/headless.md',
  'docs/platforms/cloud-vm.md',
  'docs/platforms/raspberry-pi.md',
];

for (const path of platformDocs) {
  assert.match(readRepoFile(path), /https:\/\/docs\.opencoven\.ai\/docs\/guide\/(?:platforms|deployments)/);
}
```

Require `docs/install/coven-home.md` to point to
`https://docs.opencoven.ai/docs/daemon/configuration`. The detailed
source-adjacent state-layout contract remains at `docs/daemon/coven-home.md`.

- [ ] **Step 2: Update the Rust smoke expectation**

Change the expected doctor line to:

```rust
"Install docs: https://docs.opencoven.ai/docs/guide/install",
```

- [ ] **Step 3: Run and verify failure**

```bash
node scripts/onboarding-docs-test.mjs
cargo test -p coven-cli --test smoke \
  doctor_missing_harness_prints_cross_platform_setup_loop -- --nocapture
```

Expected: failure because local pages and runtime output still use repository
URLs and substantive content.

- [ ] **Step 4: Commit the guard changes**

```bash
git add scripts/onboarding-docs-test.mjs crates/coven-cli/tests/smoke.rs
git commit -s -m "test: require canonical install entry points" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 8: Reduce local install, platform, and onboarding pages

**Files:**
- Modify: `/tmp/coven-670-wave-a/crates/coven-cli/src/main.rs`
- Modify: `/tmp/coven-670-wave-a/docs/install/cargo.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/coven-home.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/development-channels.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/docker.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/from-source.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/headless-server.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/index.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/launchd.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/linux.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/macos.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/nix.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/npm.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/podman.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/raspberry-pi.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/systemd.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/uninstall.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/updating.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/windows.md`
- Modify: `/tmp/coven-670-wave-a/docs/install/wsl2.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/cloud-vm.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/headless.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/linux.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/macos.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/raspberry-pi.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/windows.md`
- Modify: `/tmp/coven-670-wave-a/docs/platforms/wsl2.md`
- Modify: `/tmp/coven-670-wave-a/docs/start/doctor.md`
- Modify: `/tmp/coven-670-wave-a/docs/start/first-session.md`
- Modify: `/tmp/coven-670-wave-a/docs/start/quickstart.md`
- Modify: `/tmp/coven-670-wave-a/docs/start/onboarding.md`
- Modify: `/tmp/coven-670-wave-a/docs/start/showcase.md`
- Keep substantive: `/tmp/coven-670-wave-a/docs/start/coven-tui.md`
- Do not modify: `/tmp/coven-670-wave-a/docs/daemon/coven-home.md`

- [ ] **Step 1: Update runtime help**

Change `doctor_next_steps` to:

```rust
"Install docs: https://docs.opencoven.ai/docs/guide/install".to_string(),
```

- [ ] **Step 2: Replace public pages with concise pointers**

Use this shape with the page's existing title and the target assigned below:

```md
---
title: "Windows installation"
description: "Pointer to canonical Windows and WSL installation guidance."
---

Canonical guidance: **https://docs.opencoven.ai/docs/guide/platforms**

Source-adjacent state layout remains in [`../daemon/coven-home.md`](../daemon/coven-home.md).
```

Use `/docs/guide/install` for install methods, `/docs/guide/platforms` for
OS-specific pages, `/docs/guide/deployments` for headless/cloud/container
pages, `/docs/cli/uninstall` for uninstall, and
`/docs/guide/getting-started` for onboarding pages. Point
`docs/install/coven-home.md` to `/docs/daemon/configuration`.

- [ ] **Step 3: Fix the retained legacy-TUI links**

In `docs/start/coven-tui.md`, replace broken `/SESSION-LIFECYCLE` links with
the retained local contract:

```md
../SESSION-LIFECYCLE.md
```

- [ ] **Step 4: Validate**

```bash
node scripts/onboarding-docs-test.mjs
cargo test -p coven-cli --test smoke \
  doctor_missing_harness_prints_cross_platform_setup_loop -- --nocapture
python3 scripts/check-secrets.py
git diff --check
```

Expected: all commands pass.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/tests/smoke.rs \
  scripts/onboarding-docs-test.mjs docs/install docs/platforms docs/start
git commit -s -m "docs: point install and platform guidance canonical" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 9: Validate and merge the dependent cleanup

**Files:**
- No additional file changes expected.

- [ ] **Step 1: Run required gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
COVEN_NPM_DRY_RUN_VERSION=999.0.0 \
  node scripts/test-cli-prepublish.mjs --target=macos
python3 scripts/check-secrets.py
git diff --check
git add -A
python3 scripts/check-coven-privacy.py --staged
```

Expected: all commands pass.

- [ ] **Step 2: Push and open the dependent PR**

```bash
git push -u origin docs/670-wave-a-install-platform-cleanup
gh pr create --repo OpenCoven/coven \
  --title "docs: point install and platform guidance canonical" \
  --body "Implements Wave A cleanup for #670. Depends on the merged canonical coven-docs Wave A PR. Preserves local state-layout and legacy-TUI contracts while moving public install, platform, deployment, and onboarding guidance to docs.opencoven.ai."
```

- [ ] **Step 3: Merge only after full CI and review**

After merge, release `issue-670`, remove both Wave A worktrees, and delete the
local branches.
