# Signed-Tag Partial npm Release Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recover a partially published npm release through the existing OIDC-trusted workflow without overwriting published versions, weakening signed-tag gates, or losing provenance.

**Architecture:** Add one small release-context parser for stable and recovery tags, then make `.github/workflows/release-npm.yml` consume its outputs. Recovery tags re-verify both the recovery tag and base release tag, allow only operational release-file changes, assert the expected partial npm state, and publish only macOS plus the wrapper.

**Tech Stack:** Node.js 24, `node:test`, GitHub Actions YAML, Bash, npm trusted publishing over GitHub OIDC.

---

## File Map

- Create `scripts/release-npm-context.mjs`: parse stable/recovery tags and emit injection-safe GitHub Actions outputs.
- Modify `scripts/publish-npm-test.mjs`: unit-test tag parsing and lock the workflow's recovery security contract.
- Modify `.github/workflows/release-npm.yml`: verify recovery lineage/path safety, assert registry state, and conditionally publish missing packages.
- Modify `docs/reference/releasing.md`: document when and how a signed recovery tag may be used.

### Task 1: Lock the release-tag context contract

**Files:**
- Create: `scripts/release-npm-context.mjs`
- Modify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Add failing parser tests**

Import `parseReleaseTag` from the new module and add these tests to `scripts/publish-npm-test.mjs`:

```js
test('parseReleaseTag preserves stable releases', () => {
  assert.deepEqual(parseReleaseTag('v0.2.3'), {
    releaseMode: 'normal',
    releaseTag: 'v0.2.3',
    npmVersion: '0.2.3',
    recoveryAttempt: null
  });
});

test('parseReleaseTag derives the base version from signed recovery tags', () => {
  assert.deepEqual(parseReleaseTag('v0.2.3-recovery.1'), {
    releaseMode: 'recovery',
    releaseTag: 'v0.2.3',
    npmVersion: '0.2.3',
    recoveryAttempt: 1
  });
});

test('parseReleaseTag rejects malformed and unrelated prerelease tags', () => {
  for (const tag of ['v0.2', 'v0.2.3-rc.1', 'v0.2.3-recovery.0', 'v01.2.3', 'recovery-v0.2.3']) {
    assert.throws(() => parseReleaseTag(tag), /stable vX.Y.Z tag or vX.Y.Z-recovery.N/);
  }
});
```

- [ ] **Step 2: Run the tests and confirm RED**

Run:

```bash
node --test scripts/publish-npm-test.mjs
```

Expected: FAIL because `scripts/release-npm-context.mjs` or `parseReleaseTag` does not exist.

- [ ] **Step 3: Implement the minimal parser and CLI output**

Create `scripts/release-npm-context.mjs` with:

```js
#!/usr/bin/env node
import { pathToFileURL } from 'node:url';

const VERSION = '(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)';
const STABLE_TAG = new RegExp(`^v(${VERSION})$`);
const RECOVERY_TAG = new RegExp(`^v(${VERSION})-recovery\\.([1-9]\\d*)$`);

export function parseReleaseTag(tag) {
  const stable = STABLE_TAG.exec(tag);
  if (stable) {
    return {
      releaseMode: 'normal',
      releaseTag: tag,
      npmVersion: stable[1],
      recoveryAttempt: null
    };
  }

  const recovery = RECOVERY_TAG.exec(tag);
  if (recovery) {
    return {
      releaseMode: 'recovery',
      releaseTag: `v${recovery[1]}`,
      npmVersion: recovery[1],
      recoveryAttempt: Number.parseInt(recovery[5], 10)
    };
  }

  throw new Error(`Release tag ${JSON.stringify(tag)} must be a stable vX.Y.Z tag or vX.Y.Z-recovery.N tag.`);
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && moduleUrl === pathToFileURL(argv1).href;
}

if (isMainModule()) {
  const context = parseReleaseTag(process.argv[2] ?? '');
  process.stdout.write([
    `release_mode=${context.releaseMode}`,
    `release_tag=${context.releaseTag}`,
    `npm_version=${context.npmVersion}`,
    `recovery_attempt=${context.recoveryAttempt ?? ''}`,
    ''
  ].join('\n'));
}
```

- [ ] **Step 4: Run focused tests and confirm GREEN**

Run:

```bash
node --test scripts/publish-npm-test.mjs
```

Expected: all tests pass.

- [ ] **Step 5: Commit the parser contract**

```bash
git add scripts/release-npm-context.mjs scripts/publish-npm-test.mjs
git commit -s -m "test: define npm recovery tag context"
```

### Task 2: Verify recovery tags and inert recovery changes

**Files:**
- Modify: `.github/workflows/release-npm.yml`
- Modify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Add failing workflow-contract assertions**

Add a test that requires:

```js
test('release workflow fail-closes signed recovery tags', () => {
  const workflow = readFileSync(
    new URL(['..', '.github', 'workflows', 'release-npm.yml'].join('/'), import.meta.url),
    'utf8'
  );
  assert.match(workflow, /node scripts\/release-npm-context\.mjs "\$GITHUB_REF_NAME"/);
  assert.match(workflow, /outputs:[\s\S]*release_mode:[\s\S]*release_tag:[\s\S]*npm_version:/);
  assert.match(workflow, /git verify-tag "\$RELEASE_TAG"/);
  assert.match(workflow, /git merge-base --is-ancestor "\$BASE_COMMIT_SHA" "\$TAGGED_COMMIT_SHA"/);
  assert.match(workflow, /git diff --name-only "\$BASE_COMMIT_SHA\.\.\$TAGGED_COMMIT_SHA"/);
  assert.match(workflow, /Refusing recovery: changed path/);
});
```

- [ ] **Step 2: Run the focused tests and confirm RED**

Run `node --test scripts/publish-npm-test.mjs`.

Expected: FAIL because the workflow does not expose release context or recovery validation.

- [ ] **Step 3: Add release-context outputs and dual tag verification**

In `verify-tag`, add job outputs sourced from a `release-context` step:

```yaml
outputs:
  release_mode: ${{ steps.release-context.outputs.release_mode }}
  release_tag: ${{ steps.release-context.outputs.release_tag }}
  npm_version: ${{ steps.release-context.outputs.npm_version }}
```

The `release-context` step runs:

```yaml
- name: Derive stable or recovery release context
  id: release-context
  run: node scripts/release-npm-context.mjs "$GITHUB_REF_NAME" >> "$GITHUB_OUTPUT"
```

Refactor the existing signed-tag shell block so it verifies the pushed tag exactly as today. When `release_mode` is `recovery`, also:

```bash
git verify-tag "$RELEASE_TAG"
BASE_TAG_OBJECT_SHA=$(git rev-parse "$RELEASE_TAG^{tag}")
BASE_TAG_PAYLOAD=$(gh api "/repos/$GITHUB_REPOSITORY/git/tags/$BASE_TAG_OBJECT_SHA")
BASE_VERIFIED=$(jq -r '.verification.verified' <<<"$BASE_TAG_PAYLOAD")
BASE_OBJECT_TYPE=$(jq -r '.object.type // ""' <<<"$BASE_TAG_PAYLOAD")
BASE_COMMIT_SHA=$(jq -r '.object.sha // ""' <<<"$BASE_TAG_PAYLOAD")
if [ "$BASE_VERIFIED" != "true" ]; then
  echo "::error::Base release tag $RELEASE_TAG is not GitHub-verified."
  exit 1
fi
if [ "$BASE_OBJECT_TYPE" != "commit" ] || [ -z "$BASE_COMMIT_SHA" ]; then
  echo "::error::Base release tag $RELEASE_TAG does not target a commit."
  exit 1
fi
if ! git merge-base --is-ancestor "$BASE_COMMIT_SHA" "$TAGGED_COMMIT_SHA"; then
  echo "::error::Recovery tag $TAG_NAME does not descend from $RELEASE_TAG."
  exit 1
fi
```

Allow only these changed paths after the base tag:

```bash
while IFS= read -r changed_path; do
  case "$changed_path" in
    .github/workflows/release-npm.yml|\
    scripts/release-npm-context.mjs|\
    scripts/publish-npm-test.mjs|\
    docs/reference/releasing.md|\
    docs/superpowers/specs/2026-08-01-release-partial-publish-recovery-design.md|\
    docs/superpowers/plans/2026-08-01-release-partial-publish-recovery.md)
      ;;
    *)
      echo "::error::Refusing recovery: changed path $changed_path is not release-only."
      exit 1
      ;;
  esac
done < <(git diff --name-only "$BASE_COMMIT_SHA..$TAGGED_COMMIT_SHA")
```

- [ ] **Step 4: Run focused tests and confirm GREEN**

Run `node --test scripts/publish-npm-test.mjs`.

Expected: all tests pass.

- [ ] **Step 5: Commit signed recovery verification**

```bash
git add .github/workflows/release-npm.yml scripts/publish-npm-test.mjs
git commit -s -m "feat: verify signed npm recovery tags"
```

### Task 3: Publish only the missing packages

**Files:**
- Modify: `.github/workflows/release-npm.yml`
- Modify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Add failing publication-routing tests**

Add assertions requiring `npm-dry-run` and `npm-publish` to depend on `verify-tag`, use `needs.verify-tag.outputs.npm_version`, verify the four-package partial state in recovery mode, and gate Linux/Windows publish steps with:

```yaml
if: needs.verify-tag.outputs.release_mode == 'normal'
```

Require the macOS publish step to remain ungated so it runs in both modes.

- [ ] **Step 2: Run focused tests and confirm RED**

Run `node --test scripts/publish-npm-test.mjs`.

Expected: FAIL because publication still uses `github.ref_name` and always attempts Linux/Windows.

- [ ] **Step 3: Thread verified context into publish jobs**

Change job dependencies to:

```yaml
npm-dry-run:
  needs: [build-platform, verify-tag]

npm-publish:
  needs: [build-platform, npm-dry-run, verify-tag]
```

Set every `COVEN_NPM_VERSION` to `${{ needs.verify-tag.outputs.npm_version }}`.

Before publishing, add this recovery-only registry check:

```yaml
- name: Confirm expected partial npm state
  if: needs.verify-tag.outputs.release_mode == 'recovery'
  env:
    NPM_VERSION: ${{ needs.verify-tag.outputs.npm_version }}
  run: |
    set -euo pipefail
    expect_published() {
      local package_name="$1"
      local actual
      actual=$(npm view "$package_name@$NPM_VERSION" version --json | jq -r '.')
      if [ "$actual" != "$NPM_VERSION" ]; then
        echo "::error::Expected $package_name@$NPM_VERSION to be published; found ${actual:-missing}."
        exit 1
      fi
    }
    expect_missing() {
      local package_name="$1"
      local output
      if output=$(npm view "$package_name@$NPM_VERSION" version 2>&1); then
        echo "::error::Expected $package_name@$NPM_VERSION to be absent before recovery."
        exit 1
      fi
      if ! grep -q 'E404' <<<"$output"; then
        printf '%s\n' "$output"
        echo "::error::Could not prove $package_name@$NPM_VERSION is absent."
        exit 1
      fi
    }
    expect_published @opencoven/cli-linux-x64
    expect_published @opencoven/cli-windows
    expect_missing @opencoven/cli-macos
    expect_missing @opencoven/cli
```

Gate the Linux and Windows real-publish steps to normal mode. Leave the macOS command unchanged except for the verified version output; it publishes macOS and then the wrapper.

- [ ] **Step 4: Run focused tests and confirm GREEN**

Run `node --test scripts/publish-npm-test.mjs`.

Expected: all tests pass.

- [ ] **Step 5: Commit recovery publication routing**

```bash
git add .github/workflows/release-npm.yml scripts/publish-npm-test.mjs
git commit -s -m "fix: recover partial npm publication"
```

### Task 4: Document the operator path

**Files:**
- Modify: `docs/reference/releasing.md`
- Modify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Add a failing docs contract**

Require the releasing guide to include `vX.Y.Z-recovery.N`, state that the recovery tag must be newly signed and descend from the original tag, forbid moving tags, and require an already-partial Linux/Windows-present plus macOS/wrapper-missing registry state.

- [ ] **Step 2: Run focused tests and confirm RED**

Run `node --test scripts/publish-npm-test.mjs`.

Expected: FAIL because the recovery runbook is absent.

- [ ] **Step 3: Add the concise recovery runbook**

Document this exact operator sequence:

```bash
git fetch origin main --tags
git switch main
git pull --ff-only origin main
git config gpg.format ssh
git tag -s vX.Y.Z-recovery.N -m "Recover partial vX.Y.Z npm publication"
git push origin vX.Y.Z-recovery.N
```

State that this is permitted only when the original signed release is partial in the exact supported package pattern and that the workflow rejects product-code drift or an unexpected registry state.

- [ ] **Step 4: Run docs and release tests**

Run:

```bash
node --test scripts/publish-npm-test.mjs
node --test scripts/onboarding-docs-test.mjs
```

Expected: all tests pass.

- [ ] **Step 5: Commit the runbook**

```bash
git add docs/reference/releasing.md scripts/publish-npm-test.mjs
git commit -s -m "docs: add partial npm recovery runbook"
```

### Task 5: Verify and deliver the patch

**Files:**
- Verify all files changed by Tasks 1-4.

- [ ] **Step 1: Run release-facing tests**

```bash
node --test scripts/publish-npm-test.mjs
node --test scripts/onboarding-docs-test.mjs
node scripts/test-cli-prepublish.mjs
```

Expected: all pass.

- [ ] **Step 2: Run repository-required gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
git diff --check
```

Expected: all pass with zero warnings or findings.

- [ ] **Step 3: Review branch scope**

```bash
git status --short --branch
git diff --stat origin/main...HEAD
git log --show-signature --oneline origin/main..HEAD
```

Expected: only the design, plan, release workflow, helper, focused test, and releasing guide are changed; every commit is signed off.

- [ ] **Step 4: Push and open the PR**

Push `fix/release-partial-publish-recovery`, open a scoped PR with the verification matrix and rollback notes, and wait for all required checks and review threads.

- [ ] **Step 5: Merge only after fresh green proof**

Squash-merge with an explicit conventional subject and signed-off body. Verify `origin/main` contains the merge and the PR is `MERGED`.

### Task 6: Execute and prove the recovery release

**Files:**
- Use staged assets under `/private/tmp/coven-v0.2.3-release.uGaJJ2/out/`.

- [ ] **Step 1: Recheck the exact npm partial state**

Verify Linux/Windows are `0.2.3` and wrapper/macOS are still absent at `0.2.3`.

- [ ] **Step 2: Create the immutable signed recovery tag**

From fresh `origin/main`, create and locally verify `v0.2.3-recovery.1`, then push it once. Never move or reuse it.

- [ ] **Step 3: Wait for the complete release workflow**

Watch the new `Release npm packages` run to terminal success. Verify the publish job skipped Linux/Windows, passed the partial-state gate, and published macOS plus wrapper with provenance.

- [ ] **Step 4: Verify the public npm surface**

Confirm `@opencoven/cli`, `@opencoven/cli-macos`, `@opencoven/cli-linux-x64`, and `@opencoven/cli-windows` all report version/dist-tag `0.2.3`, and inspect provenance for the two recovered packages.

- [ ] **Step 5: Create and verify the GitHub Release**

Create `Coven v0.2.3` at the original `v0.2.3` tag with the three native archives and `SHA256SUMS`. Include both the original failed workflow and successful recovery workflow as evidence, then verify asset names, sizes, digests, and public URLs.

- [ ] **Step 6: Release coordination state and clean up**

Release `release-v0.2.3`, remove the task worktree/branch after merge, and verify the canonical checkout remains clean and synchronized.
