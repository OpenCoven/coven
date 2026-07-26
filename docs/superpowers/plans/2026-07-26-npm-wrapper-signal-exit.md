# npm Wrapper Signal Exit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve SIGINT/SIGTERM failure semantics through the npm wrapper instead of reporting interrupted runs as successful.

**Architecture:** Keep the existing signal-forwarding handlers. When the child reports signal termination, POSIX removes the matching handler and re-raises; Windows computes the conventional numeric exit code.

**Tech Stack:** Node.js ESM, `node:test`, child processes, npm packaging smoke tests.

---

### Task 1: Add a behavioral regression test

**Files:**
- Modify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Build an isolated fake native package**

Import `spawn`, `once`, and the required temporary-filesystem helpers. Add a
helper that copies the wrapper into a temporary ESM package, creates the
current platform's optional dependency, and symlinks its `bin/coven` to
`process.execPath`.

- [ ] **Step 2: Exercise both forwarded signals**

For POSIX, launch:

```js
spawn(process.execPath, [
  wrapperPath,
  '-e',
  'console.log("ready"); setInterval(() => {}, 1_000);'
]);
```

Wait for `ready`, send SIGINT or SIGTERM to the wrapper, await its exit, and
assert `code === null` and `exitSignal === signal`. Clean the fixture and kill
any surviving child in `finally`.

- [ ] **Step 3: Run the test and verify RED**

Run:

```sh
node --test --test-name-pattern='preserves child signal termination' scripts/publish-npm-test.mjs
```

Expected: the current wrapper exits with code 0 and no terminating signal.

### Task 2: Fix termination propagation

**Files:**
- Modify: `npm/coven/bin/coven.js`
- Modify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Add the platform-aware exit path**

Import Node OS constants and replace the signal branch with:

```js
if (signal) {
  if (process.platform === 'win32') {
    const signalNumber = osConstants.signals[signal];
    process.exit(signalNumber === undefined ? 1 : 128 + signalNumber);
  }
  process.removeAllListeners(signal);
  process.kill(process.pid, signal);
  return;
}
```

- [ ] **Step 2: Guard the packaged Windows fallback**

Add assertions that the wrapper contains the Windows platform branch,
`osConstants.signals[signal]`, and `128 + signalNumber`.

- [ ] **Step 3: Run the focused tests and verify GREEN**

```sh
node --test --test-name-pattern='preserves child signal termination|Windows signal fallback' scripts/publish-npm-test.mjs
```

Expected: both tests pass.

### Task 3: Run npm and repository verification

**Files:**
- Verify: `npm/coven/bin/coven.js`
- Verify: `scripts/publish-npm-test.mjs`

- [ ] **Step 1: Run npm guardrails**

```sh
node --test scripts/onboarding-docs-test.mjs scripts/pr-readiness-test.mjs scripts/publish-npm-test.mjs
node scripts/test-cli-prepublish.mjs --target=macos --skip-build --skip-secrets-scan
```

Expected: all tests and the packaged-wrapper smoke pass.

- [ ] **Step 2: Run repository gates**

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

Expected: all commands exit 0.

- [ ] **Step 3: Commit and push**

```sh
git add npm/coven/bin/coven.js scripts/publish-npm-test.mjs docs/superpowers
git commit -m "fix: preserve npm wrapper signal exits"
git push -u origin fix/495-npm-signal-exit
```

- [ ] **Step 4: Open the PR**

Create a PR with `Closes #495`, the observed pre-fix exit-0 evidence, and the
post-fix SIGINT/SIGTERM results. Resolve review conversations only after their
concerns are addressed.
