---
title: "Windows runner artifact binding implementation plan"
summary: "Bind the independent runner and native containment helper without substituting the selected subject."
read_when:
  - Implementing the Windows independent runner artifact boundary
source_adjacent_reason: "Defines the versioned artifact encoding beside its source-owned runner and audit producer."
---

# Windows Runner Artifact Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Define one canonical versioned artifact for the Node runner and Rust Windows containment helper, preserving independent subject identity.

**Architecture:** The closed job/result runner fields remain unchanged. For the future Windows runner version `0.2.0-windows-job-v1`, `runner.artifactSha256` identifies canonical manifest bytes containing the exact runner/helper hashes and immutable common source commit. These are artifact metadata, not authenticated provenance or native containment proof. Existing POSIX byte binding and all Windows execution refusals remain unchanged in this component.

**Tech Stack:** Existing Node Buffer, crypto SHA-256, JCS canonicalizer and built-in tests.

The native helper producer must build from that pinned source and independently verify native image/host/ABI identity. The Windows artifact consumer must recompute this binding before executing private copies. The full #1378 outcome still requires the creation-time Job Object launcher, exact private executable handle protection, parent/controller/descendant cleanup, bounded minimized I/O, native fixture evidence and CI. No support is enabled merely by constructing this metadata.

## Task 1: Reproduce the missing explicit binding

**Files:** Modify `conformance/automations/runner/conformance.test.mjs`.

- [x] Append the complete tests below. They compute expected hashes with the test's independent crypto/canonicalization helpers, cover role/byte/source substitution and refuse mutable or malformed source/input values with fixed errors.

```js

test("Windows runner artifact binds canonical runner/helper identity separately from the subject", async () => {
  const { createWindowsRunnerArtifact } = await import("./conformance.mjs");
  const runner = Buffer.from("runner-fixture");
  const helper = Buffer.from("helper-fixture");
  const sourceCommit = "1".repeat(40);
  const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
  const artifact = createWindowsRunnerArtifact(runner, helper, sourceCommit);
  const expected = {
    schemaVersion: "coven.automations.windows-runner-artifact.v1",
    sourceCommit,
    runner: { fileName: "conformance.mjs", sha256: sha256(runner) },
    containmentHelper: {
      fileName: "coven-conformance-supervisor.exe",
      abi: "coven.automations.windows-job-supervisor.v1",
      sha256: sha256(helper),
    },
  };
  assert.equal(artifact.version, "0.2.0-windows-job-v1");
  assert.equal(artifact.bytes.toString("utf8"), canonicalize(expected));
  assert.equal(artifact.sha256, sha256(artifact.bytes));
  assert.deepEqual(JSON.parse(artifact.bytes), expected);
  const repeated = createWindowsRunnerArtifact(Buffer.from(runner), Buffer.from(helper), sourceCommit);
  assert.deepEqual(repeated, artifact);
});

test("Windows runner artifact rejects byte, role and source ambiguity", async () => {
  const { createWindowsRunnerArtifact } = await import("./conformance.mjs");
  const source = "1".repeat(40);
  const bind = (runner, helper, commit = source) =>
    createWindowsRunnerArtifact(Buffer.from(runner), Buffer.from(helper), commit).sha256;
  const original = bind("runner", "helper");
  assert.notEqual(bind("Runner", "helper"), original);
  assert.notEqual(bind("runner", "Helper"), original);
  assert.notEqual(bind("helper", "runner"), original);
  assert.notEqual(bind("runner", "helper", "2".repeat(40)), original);
  assert.notEqual(bind("a", "bc"), bind("ab", "c"));
});

test("Windows runner artifact refuses invalid inputs without echoing them", async () => {
  const { createWindowsRunnerArtifact } = await import("./conformance.mjs");
  const bytes = Buffer.from("fixture");
  const source = "1".repeat(40);
  for (const args of [
    [null, bytes, source], [bytes, null, source],
    ["PRIVATE-RUNNER", bytes, source], [bytes, "PRIVATE-HELPER", source],
    [Buffer.alloc(0), bytes, source], [bytes, Buffer.alloc(0), source],
    [bytes, bytes, "main"], [bytes, bytes, "PRIVATE-SOURCE"],
    [bytes, bytes, "A".repeat(40)], [bytes, bytes, undefined],
  ]) {
    assert.throws(() => createWindowsRunnerArtifact(...args),
      { message: "Windows runner artifact input is invalid" });
  }
});
```

- [x] Run `node --test --test-name-pattern='Windows runner artifact' conformance/automations/runner/conformance.test.mjs`. Expect failure because `createWindowsRunnerArtifact` does not exist.

## Task 2: Implement the canonical artifact

**Files:** Modify `conformance/automations/runner/conformance.mjs`.

- [x] Insert this function after `digestBytes`:

```js
// The Windows execution boundary is a versioned two-file artifact. The subject
// remains independently bound by subjectArtifact.sha256; no wrapper substitution.
export function createWindowsRunnerArtifact(runnerBytes, helperBytes, sourceCommit) {
  if (
    !Buffer.isBuffer(runnerBytes) || runnerBytes.length === 0 ||
    !Buffer.isBuffer(helperBytes) || helperBytes.length === 0 ||
    !isStringMatching(sourceCommit, SOURCE_COMMIT)
  ) {
    throw new JobError("Windows runner artifact input is invalid");
  }
  const manifest = {
    schemaVersion: "coven.automations.windows-runner-artifact.v1",
    sourceCommit,
    runner: { fileName: "conformance.mjs", sha256: digestBytes(runnerBytes) },
    containmentHelper: {
      fileName: "coven-conformance-supervisor.exe",
      abi: "coven.automations.windows-job-supervisor.v1",
      sha256: digestBytes(helperBytes),
    },
  };
  const bytes = Buffer.from(canonicalize(manifest), "utf8");
  return { version: "0.2.0-windows-job-v1", bytes, sha256: digestBytes(bytes) };
}

```

- [x] Rerun the focused command; all three binding tests must pass.
- [x] Run `node --test conformance/automations/runner/conformance.test.mjs conformance/automations/runner/audit.test.mjs` to retain existing artifact checks, Linux/macOS execution and explicit Windows refusal.

## Task 3: Continue the native platform boundary

- [x] Document the artifact encoding and its current non-execution scope in `conformance/automations/runner/README.md`.
- [ ] Record RED/GREEN proof on #1378 and the execution ledger, preserving all remaining native requirements.
- [ ] Implement the Rust launcher and integrate this binding in both producer and consumer before enabling Windows. A helper is not an implementation subject; `subjectArtifact.sha256` must continue identifying the actual target bytes. Require native CI before support or completion claims.
- [ ] Run the full required repository guards, source review and signed/DCO/exact-head protected delivery flow before any commit/PR. Execute inline within the active goal, keeping #857 gates and full #858 acceptance unchanged.
