import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { fileURLToPath } from "node:url";

const RUNNER = fileURLToPath(new URL("./conformance.mjs", import.meta.url));
const SHA_A = "a".repeat(64);
const SHA_B = "b".repeat(64);
const SHA_C = "c".repeat(64);
const SHA_D = "d".repeat(64);
const SHA_E = "e".repeat(64);
const STARTUP_DELAY_MS = 3_000;
const SYNCHRONIZED_SUITE_DELAY_MS = 12_500;
const FIXTURE_SELF_EXIT_MS = 60_000;
// Midpoint between the 10-second audit watchdog and fixture self-exit, not a
// product latency assertion. A missed force-kill must not pass via self-exit.
const FORCED_STOP_CUTOFF_MS = 35_000;

function canonicalize(value) {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalize).join(",")}]`;
  }
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalize(value[key])}`)
    .join(",")}}`;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function validJob(overrides = {}) {
  const job = {
    schemaVersion: "coven.automations.conformance-job.v1",
    resultId: "audit-2026-09-08T120000Z",
    decisionScope: { kind: "audit_only" },
    source: {
      repository: "https://github.com/OpenCoven/coven",
      commit: "1".repeat(40),
    },
    protocolArtifact: {
      bundleSchemaVersion: "coven.automations.bundle.v1",
      sourceCommit: "1".repeat(40),
      bundleSha256: SHA_A,
      contractContentSha256: SHA_B,
      fileCount: 19,
    },
    runner: {
      name: "coven-automations-conformance-runner",
      version: "0.1.0",
      artifactSha256: sha256(readFileSync(RUNNER)),
      vectorSetSha256: SHA_D,
    },
    subjectArtifact: {
      artifactId: "coven-cli",
      artifactVersion: "0.1.0",
      platform: { os: "linux", arch: "x86_64" },
      sha256: SHA_E,
    },
    environment: {
      os: "linux",
      arch: "x86_64",
      runtime: "node-22",
    },
    observedAt: "2026-09-08T12:00:00Z",
    suites: [
      {
        profile: "structural",
        suiteId: "schema-validation",
        vector: {
          schemaVersion: "coven.automations.conformance-vector.v1",
          vectorId: "schema-valid",
        },
      },
    ],
    ...overrides,
  };
  job.runner.vectorSetSha256 = sha256(canonicalize(job.suites));
  return job;
}

async function writeTarget(directory) {
  const path = join(directory, "target.mjs");
  await writeFile(
    path,
    `#!/usr/bin/env node
import { spawn } from "node:child_process";
import { appendFileSync, chmodSync, writeFileSync } from "node:fs";
const mode = process.env.COVEN_TEST_TARGET_MODE ?? "pass";
if (mode === "slow-startup") {
  await new Promise((resolve) => setTimeout(resolve, ${STARTUP_DELAY_MS}));
}
if (mode === "unavailable") process.exit(10);
if (
  mode === "require-runner-scratch" &&
  !process.env.COVEN_AUTOMATIONS_CONFORMANCE_SCRATCH
) {
  process.exit(11);
}
const operation = process.argv.at(-1);
if (operation === "capability") {
  if (mode === "replace-source") {
    writeFileSync(process.argv[1], \`#!/usr/bin/env node
const operation = process.argv.at(-1);
if (operation === "evaluate") {
  let input = "";
  for await (const chunk of process.stdin) input += chunk;
  const request = JSON.parse(input);
  process.stdout.write(JSON.stringify({
    schemaVersion: "coven.automations.conformance-suite-result.v1",
    suiteId: request.suiteId,
    status: "passed",
    evidence: { assertion: "substituted-target-ran" }
  }));
}
\`);
    chmodSync(process.argv[1], 0o755);
  }
  if (mode === "ignore-timeout" || mode === "descendant-timeout") {
    const record = (event) => appendFileSync(process.env.COVEN_TEST_LIFECYCLE_FILE, event + "\\n");
    process.on("SIGTERM", () => record("parent-sigterm"));
    record("parent-ready");
    setTimeout(() => {
      record("parent-self-exit");
      process.exit(10);
    }, ${FIXTURE_SELF_EXIT_MS});
    if (mode === "descendant-timeout") {
      spawn(process.execPath, ["-e", \`
        const { appendFileSync } = require("node:fs");
        const record = (event) => appendFileSync(process.env.COVEN_TEST_LIFECYCLE_FILE, event + "\\\\n");
        process.on("SIGTERM", () => record("child-sigterm"));
        record("child-ready");
        setTimeout(() => {
          record("child-self-exit");
          process.exit(10);
        }, ${FIXTURE_SELF_EXIT_MS});
      \`], {
        stdio: ["ignore", process.stdout, process.stderr]
      });
    }
    await new Promise(() => {});
  }
  if (mode === "malformed-capability") {
    process.stdout.write('{"credential":"SECRET-CAPABILITY-OUTPUT"');
    process.exit(0);
  }
  if (mode === "invalid-utf8-capability") {
    process.stdout.write(Buffer.from([0x7b, 0x22, 0xff, 0x22, 0x3a, 0x31, 0x7d]));
    process.exit(0);
  }
  process.stdout.write(JSON.stringify({
    schemaVersion: "coven.automations.conformance-target-capability.v1",
    profiles: mode === "slow-stateful-suite"
      ? [{
          profile: "scheduler_reliability",
          suites: ["startup-reconciliation-wake"],
        }]
      : mode === "slow-cancellation-suite"
        ? [{
            profile: "scheduler_reliability",
            suites: ["cancellation-timeout-arbitration"],
          }]
      : [{ profile: "structural", suites: ["schema-validation"] }]
  }));
  process.exit(0);
}
if (operation !== "evaluate") process.exit(3);
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (mode === "slow-stateful-suite" || mode === "slow-cancellation-suite") {
  await new Promise((resolve) => setTimeout(resolve, ${SYNCHRONIZED_SUITE_DELAY_MS}));
}
if (mode === "malformed") {
  process.stdout.write('{"credential":"SECRET-TARGET-OUTPUT"');
  process.exit(0);
}
if (mode === "early-exit") process.exit(0);
if (mode === "invalid-utf8") {
  const prefix = Buffer.from('{"schemaVersion":"coven.automations.conformance-suite-result.v1","suiteId":"' + request.suiteId + '","status":"passed","evidence":{"text":"');
  const suffix = Buffer.from('"}}');
  process.stdout.write(Buffer.concat([prefix, Buffer.from([0xff]), suffix]));
  process.exit(0);
}
if (mode === "deep-evidence") {
  const depth = 20_000;
  process.stdout.write(
    '{"schemaVersion":"coven.automations.conformance-suite-result.v1","suiteId":"' +
      request.suiteId +
      '","status":"passed","evidence":' +
      "[".repeat(depth) +
      "0" +
      "]".repeat(depth) +
      "}",
  );
  process.exit(0);
}
if (mode === "unsafe-evidence") {
  process.stdout.write('{"schemaVersion":"coven.automations.conformance-suite-result.v1","suiteId":"' + request.suiteId + '","status":"passed","evidence":{"count":9007199254740993}}');
  process.exit(0);
}
process.stdout.write(JSON.stringify({
  schemaVersion: "coven.automations.conformance-suite-result.v1",
  suiteId: request.suiteId,
  status: "passed",
  evidence: {
    assertion: "native-target-ran",
    count: 1,
    ...(mode === "slow-startup" ? { startupDelayMs: ${STARTUP_DELAY_MS} } : {})
  }
}));
`,
    { mode: 0o755 },
  );
  await chmod(path, 0o755);
  return path;
}

async function runRunner({
  job = validJob(),
  targetCommand,
  env = {},
}) {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-runner-"));
  const jobPath = join(directory, "job.json");
  const boundJob = structuredClone(job);
  if (boundJob.subjectArtifact && "sha256" in boundJob.subjectArtifact) {
    boundJob.subjectArtifact.sha256 = sha256(await readFile(targetCommand));
  }
  await writeFile(jobPath, `${JSON.stringify(boundJob)}\n`);
  const args = [
    RUNNER,
    "--job",
    jobPath,
    "--target-command",
    targetCommand,
  ];
  const result = spawnSync(process.execPath, args, {
      encoding: "utf8",
      env: { ...process.env, ...env },
      // Covers capability (10s), a synchronized suite (20s), cleanup, and even
      // the 60s fixture fallback when force-kill is broken. Only a hang guard.
      timeout: 90_000,
  });
  return result;
}

test("emits a valid audit-only result for an executed native target suite", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stderr, "");
  const report = JSON.parse(result.stdout);
  assert.equal(
    report.schemaVersion,
    "coven.automations.conformance-result.v1",
  );
  assert.deepEqual(report.statement.decisionScope, { kind: "audit_only" });
  assert.equal(report.statement.overallStatus, "passed");
  assert.deepEqual(report.statement.profileResults, [
    {
      profile: "structural",
      status: "passed",
      requiredSuites: ["schema-validation"],
      suiteResults: [
        {
          suiteId: "schema-validation",
          status: "passed",
          evidenceDigest: {
            algorithm: "sha256",
            canonicalization: "jcs-rfc8785",
            value:
              "afac845080050a6b189e452d960b54d5d2b493b0975d153fa9c0914ff32ff316",
          },
        },
      ],
    },
  ]);
  assert.deepEqual(report.statementDigest, {
    algorithm: "sha256",
    canonicalization: "jcs-rfc8785",
    value: sha256(canonicalize(report.statement)),
  });
  assert.equal("authentication" in report, false);
});

test("records an unavailable target as not applicable and fails the gate", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "unavailable" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target unavailable\n");
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "not_applicable");
  assert.deepEqual(report.statement.profileResults[0].suiteResults, [
    {
      suiteId: "schema-validation",
      status: "not_applicable",
    },
  ]);
});

test("turns malformed target output into a static privacy-safe failure", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "malformed" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  assert.equal(result.stdout.includes("SECRET-TARGET-OUTPUT"), false);
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "failed");
  assert.deepEqual(report.statement.profileResults[0].suiteResults, [
    {
      suiteId: "schema-validation",
      status: "failed",
    },
  ]);
});

test("distinguishes malformed capability output from an unavailable target", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "malformed-capability" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  assert.equal(result.stdout.includes("SECRET-CAPABILITY-OUTPUT"), false);
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "failed");
});

test("classifies non-UTF-8 capability output as invalid, not unavailable", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "invalid-utf8-capability" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "failed");
});

test("rejects target evidence outside the portable JCS number subset", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "unsafe-evidence" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "failed");
  assert.equal(
    "evidenceDigest" in
      report.statement.profileResults[0].suiteResults[0],
    false,
  );
});

test("refuses release eligibility because this runner is audit only", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob({
    decisionScope: {
      kind: "release_eligibility",
      policyBinding: {
        policyId: "release",
        policyVersion: "1",
        digest: SHA_A,
      },
    },
  });

  const result = await runRunner({
    job,
    targetCommand: target,
  });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: decision scope is not audit-only\n",
  );
  assert.equal(result.stdout, "");
});

test("rejects a vector-set digest that does not bind the requested suites", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob();
  job.runner.vectorSetSha256 = SHA_D;

  const result = await runRunner({
    job,
    targetCommand: target,
  });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: vector set binding is invalid\n",
  );
  assert.equal(result.stdout, "");
});

test("rejects a runner digest that does not bind the executing script", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob();
  job.runner.artifactSha256 = SHA_C;

  const result = await runRunner({
    job,
    targetCommand: target,
  });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: runner artifact binding is invalid\n",
  );
  assert.equal(result.stdout, "");
});

test("refuses a full-profile claim in the bounded runner slice", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob({
    suites: [
      {
        profile: "full",
        suiteId: "full-profile",
        vector: { vectorId: "unsupported-full-claim" },
      },
    ],
  });

  const result = await runRunner({
    job,
    targetCommand: target,
  });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: full profile is unsupported\n",
  );
  assert.equal(result.stdout, "");
});

test("requires exact subject-artifact metadata before executing", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob({
    subjectArtifact: {
      artifactId: "coven-cli",
      artifactVersion: "0.1.0",
      platform: { os: "linux", arch: "x86_64" },
    },
  });

  const result = await runRunner({
    job,
    targetCommand: target,
  });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: subject artifact is invalid\n",
  );
  assert.equal(result.stdout, "");
});

test("rejects normalized-but-invalid calendar timestamps", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob({ observedAt: "2026-02-31T12:00:00Z" });

  const result = await runRunner({
    job,
    targetCommand: target,
  });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: observed timestamp is invalid\n",
  );
  assert.equal(result.stdout, "");
});

test("requires the protocol bundle source commit to match the source binding", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob();
  job.protocolArtifact.sourceCommit = "2".repeat(40);

  const result = await runRunner({ job, targetCommand: target });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: protocol source binding is invalid\n",
  );
  assert.equal(result.stdout, "");
});

test("rejects relative target commands before artifact binding", async () => {
  const result = spawnSync(
    process.execPath,
    [
      RUNNER,
      "--job",
      "job.json",
      "--target-command",
      "relative-coven",
    ],
    { encoding: "utf8" },
  );

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: target command must be absolute\n",
  );
  assert.equal(result.stdout, "");
});

test("executes every target operation from the same staged subject bytes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const originalDigest = sha256(await readFile(target));

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "replace-source" },
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(sha256(await readFile(target)), originalDigest);
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.subjectArtifact.sha256, originalDigest);
});

test("provides runner-owned scratch storage to every target invocation", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "require-runner-scratch" },
  });

  assert.equal(result.status, 0, result.stderr);
});

test("allows startup beyond the old two-second limit without treating it as a hang", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const target = await writeTarget(directory);
  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "slow-startup" },
  });

  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "passed");
  assert.equal(
    report.statement.profileResults[0].suiteResults[0].evidenceDigest.value,
    sha256(canonicalize({
      assertion: "native-target-ran",
      count: 1,
      startupDelayMs: STARTUP_DELAY_MS,
    })),
  );
});

test("allows the stateful startup wake suite to use its synchronization budget", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob({
    suites: [
      {
        profile: "scheduler_reliability",
        suiteId: "startup-reconciliation-wake",
        vector: {
          schemaVersion: "coven.automations.startup-reconciliation-wake-vectors.v1",
          cases: [],
        },
      },
    ],
  });
  job.runner.vectorSetSha256 = sha256(canonicalize(job.suites));

  const result = await runRunner({
    job,
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "slow-stateful-suite" },
  });

  assert.equal(result.status, 0, result.stderr);
});

test("allows cancellation arbitration to use its synchronization budget", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob({
    suites: [
      {
        profile: "scheduler_reliability",
        suiteId: "cancellation-timeout-arbitration",
        vector: {
          schemaVersion: "coven.automations.cancellation-timeout-arbitration-vectors.v1",
          cases: [],
        },
      },
    ],
  });
  job.runner.vectorSetSha256 = sha256(canonicalize(job.suites));

  const result = await runRunner({
    job,
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "slow-cancellation-suite" },
  });

  assert.equal(result.status, 0, result.stderr);
});

test("force-kills a target that ignores the graceful timeout", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const target = await writeTarget(directory);
  const lifecycleFile = join(directory, "lifecycle.log");
  const startedAt = Date.now();

  const result = await runRunner({
    targetCommand: target,
    env: {
      COVEN_TEST_TARGET_MODE: "ignore-timeout",
      COVEN_TEST_LIFECYCLE_FILE: lifecycleFile,
    },
  });

  const elapsedMs = Date.now() - startedAt;
  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target unavailable\n");
  assert.ok(
    elapsedMs < FORCED_STOP_CUTOFF_MS,
    `hard timeout took ${elapsedMs}ms`,
  );
  // Readiness follows SIGTERM-handler installation. A callback need not be
  // scheduled within the 100ms grace, but self-exit must never complete the test.
  assert.deepEqual(
    (await readFile(lifecycleFile, "utf8")).trim().split("\n")
      .filter((event) => event !== "parent-sigterm"),
    ["parent-ready"],
  );
});

test("force-kills descendants that retain the target pipes", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const target = await writeTarget(directory);
  const lifecycleFile = join(directory, "lifecycle.log");
  const startedAt = Date.now();

  const result = await runRunner({
    targetCommand: target,
    env: {
      COVEN_TEST_TARGET_MODE: "descendant-timeout",
      COVEN_TEST_LIFECYCLE_FILE: lifecycleFile,
    },
  });

  const elapsedMs = Date.now() - startedAt;
  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target unavailable\n");
  assert.ok(
    elapsedMs < FORCED_STOP_CUTOFF_MS,
    `process-tree timeout took ${elapsedMs}ms`,
  );
  assert.deepEqual(
    (await readFile(lifecycleFile, "utf8")).trim().split("\n")
      .filter((event) => !["parent-sigterm", "child-sigterm"].includes(event)).sort(),
    ["child-ready", "parent-ready"],
  );
});

test("turns early target stdin closure into a static failed result", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob();
  job.suites[0].vector.payload = "x".repeat(8 * 1024 * 1024);
  job.runner.vectorSetSha256 = sha256(canonicalize(job.suites));

  const result = await runRunner({
    job,
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "early-exit" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  assert.doesNotThrow(() => JSON.parse(result.stdout));
});

test("rejects non-UTF-8 target output instead of replacement-decoding it", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "invalid-utf8" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  assert.equal(result.stdout.includes("\ufffd"), false);
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "failed");
});

test("rejects vector sets that cannot be represented as portable JCS", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const job = validJob();
  job.suites[0].vector.payload = "\ud800";
  job.runner.vectorSetSha256 = sha256(canonicalize(job.suites));

  const result = await runRunner({ job, targetCommand: target });

  assert.equal(result.status, 2);
  assert.equal(
    result.stderr,
    "invalid conformance job: vector set is not portable JCS\n",
  );
  assert.equal(result.stdout, "");
});

test("turns excessively deep target evidence into a failed result", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "deep-evidence" },
  });

  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target response invalid\n");
  const report = JSON.parse(result.stdout);
  assert.equal(report.statement.overallStatus, "failed");
});

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

test("Windows control accepts fragmented closed frames only after ACK and complete helper closure", async () => {
  const { createWindowsControlReader } = await import("./conformance.mjs");
  const nonce = "ab".repeat(16);
  const frame = (kind, extra = {}) => Buffer.from(JSON.stringify({ schemaVersion: "coven.automations.windows-job-supervisor.v1", nonce, kind, ...extra }) + "\n");
  const reader = createWindowsControlReader(nonce);
  const ready = frame("ready");
  for (const byte of ready) reader.push(Buffer.from([byte]), () => reader.requestAck());
  reader.push(frame("exited", { exitCode: 4294967295 }));
  // Terminal delivery can race the ACK write callback, but cannot become proof.
  assert.throws(() => reader.finish(0), { message: "Windows helper control is invalid" });
  const valid = createWindowsControlReader(nonce);
  valid.push(ready, () => { valid.requestAck(); valid.completeAck(); });
  valid.push(frame("exited", { exitCode: 4294967295 }));
  assert.deepEqual(valid.finish(0), { kind: "exited", exitCode: 4294967295 });
  for (const [failure, status] of [["launch_invalid", 2], ["cleanup_unproven", 3]]) {
    const refused = createWindowsControlReader(nonce);
    refused.push(frame("refused", { failure }));
    assert.deepEqual(refused.finish(status), { kind: "refused", failure });
  }
});

test("Windows control refuses premature, duplicate, trailing and contradictory evidence", async () => {
  const { createWindowsControlReader } = await import("./conformance.mjs");
  const nonce = "ab".repeat(16);
  const line = (kind, extra = {}) => JSON.stringify({ schemaVersion: "coven.automations.windows-job-supervisor.v1", nonce, kind, ...extra }) + "\n";
  const invalid = (action) => {
    const reader = createWindowsControlReader(nonce);
    assert.throws(() => action(reader), { message: "Windows helper control is invalid" });
    assert.throws(() => reader.finish(0), { message: "Windows helper control is invalid" });
  };
  invalid(reader => reader.push(Buffer.from(line("exited", { exitCode: 0 }))));
  invalid(reader => reader.push(Buffer.from(line("ready") + line("exited", { exitCode: 0 }))));
  invalid(reader => reader.push(Buffer.from(line("ready") + line("ready"))));
  invalid(reader => reader.push(Buffer.from(line("refused", { failure: "io_refused" }) + line("ready"))));
  invalid(reader => { reader.push(Buffer.from(line("ready")), () => { reader.requestAck(); reader.completeAck(); }); reader.push(Buffer.from(line("exited", { exitCode: 0 }) + "x")); reader.finish(0); });
  invalid(reader => reader.finish(0));
  invalid(reader => { reader.push(Buffer.from(line("refused", { failure: "cleanup_unproven" }))); reader.finish(2); });
  invalid(reader => { reader.push(Buffer.from(line("ready")), () => { reader.requestAck(); reader.completeAck(); }); reader.push(Buffer.from(line("exited", { exitCode: 0 }))); reader.finish(2); });
});

test("Windows control rejects ambiguous JSON, private fields, oversized data and invalid UTF-8", async () => {
  const { createWindowsControlReader } = await import("./conformance.mjs");
  const nonce = "ab".repeat(16);
  const ready = { schemaVersion: "coven.automations.windows-job-supervisor.v1", nonce, kind: "ready" };
  for (const bytes of [
    Buffer.from(JSON.stringify({ ...ready, private: "sensitive fixture" }) + "\n"),
    Buffer.from(JSON.stringify({ ...ready, nonce: "cd".repeat(16) }) + "\n"),
    Buffer.from(JSON.stringify(ready).replace('"kind":"ready"', '"kind":"ready","kind":"ready"') + "\n"),
    Buffer.from(JSON.stringify(ready).replace('"kind":"ready"', '"kind":"ready","\\u006bind":"ready"') + "\n"),
    Buffer.alloc(256, 0x20), Buffer.alloc(1024 * 1024, 0x20), Buffer.from([0xff, 10]),
    Buffer.from(JSON.stringify({ ...ready, kind: "exited", exitCode: -1 }) + "\n"),
    Buffer.from(JSON.stringify({ ...ready, kind: "refused", failure: "sensitive fixture" }) + "\n"),
  ]) {
    const reader = createWindowsControlReader(nonce);
    assert.throws(() => reader.push(bytes), { message: "Windows helper control is invalid" });
    assert.throws(() => reader.finish(0), { message: "Windows helper control is invalid" });
  }
});


test("Windows control permits the exact wire cap and buffers one terminal until ACK completion", async () => {
  const { createWindowsControlReader } = await import("./conformance.mjs");
  const nonce = "ab".repeat(16);
  const line = kind => JSON.stringify({ schemaVersion: "coven.automations.windows-job-supervisor.v1", nonce, kind });
  const padded = line("ready").padEnd(255, " ");
  const reader = createWindowsControlReader(nonce);
  reader.push(Buffer.from(padded));
  reader.push(Buffer.from("\n"), () => reader.requestAck());
  reader.push(Buffer.from(JSON.stringify({ schemaVersion: "coven.automations.windows-job-supervisor.v1", nonce, kind: "exited", exitCode: 0 }) + "\n"));
  reader.completeAck();
  assert.deepEqual(reader.finish(0), { kind: "exited", exitCode: 0 });
  const oversized = createWindowsControlReader(nonce);
  assert.throws(() => oversized.push(Buffer.from(padded + " \n")), { message: "Windows helper control is invalid" });
});

test("Windows helper consumer services ACK while a large body is backpressured", async () => {
  const { invokeWindowsHelper } = await import("./conformance.mjs");
  const directory = await mkdtemp(join(tmpdir(), "coven-windows-consumer-"));
  try {
    const fixture = join(directory, "helper.mjs");
    await writeFile(fixture, `
      import { readSync, writeSync } from 'node:fs';
      let header; let bytes = Buffer.alloc(0); let count = 0; let ended = false; let ackBeforeEOF = false;
      const frame = (kind, extra = {}) => writeSync(3, JSON.stringify({schemaVersion:header.schemaVersion,nonce:header.nonce,kind,...extra})+'\\n');

      process.stdin.on('data', chunk => {
        if (header) { count += chunk.length; return; }
        bytes = Buffer.concat([bytes, chunk]); const end = bytes.indexOf(10); if (end < 0) return;
        header = JSON.parse(bytes.subarray(0,end)); count = bytes.length-end-1; bytes = Buffer.alloc(0);
        process.stdin.pause(); frame('ready');
        const ack = Buffer.alloc(36); let received = 0;
        while(received < ack.length) { const count = readSync(3, ack, received, ack.length-received); if(count===0)process.exit(3); received += count; }
        if(ack.toString()!=='GO '+header.nonce+'\\n')process.exit(3);
        ackBeforeEOF = !ended; process.stdin.resume();
      });
      process.stdin.on('end', () => { ended = true; writeSync(1, JSON.stringify({ackBeforeEOF,bodyBytes:count})); frame('exited',{exitCode:23}); process.exit(0); });
    `);
    const result = await invokeWindowsHelper({ helper: process.execPath, target: fixture, directory,
      subjectSha256: SHA_A, args: [], input: Buffer.alloc(4 * 1024 * 1024, 0x41), timeoutMs: 30000 });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 23);
    assert.deepEqual(JSON.parse(result.stdout), { ackBeforeEOF: true, bodyBytes: 4 * 1024 * 1024 });
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("Windows helper consumer rejects execution without valid terminal and helper status", async () => {
  const { invokeWindowsHelper } = await import("./conformance.mjs");
  const directory = await mkdtemp(join(tmpdir(), "coven-windows-consumer-invalid-"));
  try {
    const fixture = join(directory, "helper.mjs");
    await writeFile(fixture, `
      import { readSync, writeSync } from 'node:fs';
      let bytes=''; let header;
      const frame = (kind,extra={}) => writeSync(3,JSON.stringify({schemaVersion:header.schemaVersion,nonce:header.nonce,kind,...extra})+'\\n');
      process.stdin.on('data', chunk => { bytes+=chunk; if(header||!bytes.includes('\\n'))return; header=JSON.parse(bytes.split('\\n')[0]);
        if(process.env.COVEN_WINDOWS_FIXTURE_MODE==='refused'){frame('refused',{failure:'launch_invalid'});process.exit(2);}
        if(process.env.COVEN_WINDOWS_FIXTURE_MODE==='premature'){frame('exited',{exitCode:0});process.exit(0);}
        frame('ready');
        const ack=Buffer.alloc(36); let count=0; while(count<36){const size=readSync(3,ack,count,36-count);if(size===0)process.exit(3);count+=size;} frame('exited',{exitCode:0});process.exit(2);
      });
    `);
    for (const mode of ["refused", "premature", "status-mismatch"]) {
      const result = await invokeWindowsHelper({ helper: process.execPath, target: fixture, directory,
        subjectSha256: SHA_A, args: [], input: Buffer.alloc(0), timeoutMs: 30000,
        env: { ...process.env, COVEN_WINDOWS_FIXTURE_MODE: mode } });
      assert.ok(result.error);
      assert.equal(result.status, null);
      assert.equal(result.stdout, "");
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});


test("Windows helper consumer releases owned streams and loop reference when kill never closes", async () => {
  const { invokeWindowsHelper } = await import("./conformance.mjs");
  const child = new EventEmitter();
  const streams = Array.from({ length: 4 }, () => new PassThrough());
  [child.stdin, child.stdout, child.stderr] = streams;
  child.stdio = streams;
  let kills = 0; let unrefs = 0;
  child.kill = () => { kills += 1; return false; };
  child.unref = () => { unrefs += 1; };
  const timers = [];
  const result = invokeWindowsHelper({ helper: process.execPath, target: RUNNER, directory: tmpdir(),
    subjectSha256: SHA_A, args: [], input: Buffer.alloc(0), timeoutMs: 30000 }, {
    spawnHelper: () => child,
    schedule: callback => { const timer = { callback, cleared: false }; timers.push(timer); return timer; },
    cancel: timer => { if (timer) timer.cleared = true; },
  });
  assert.equal(timers.length, 1);
  timers[0].callback(); // Watchdog: failed kill and no child close event.
  assert.equal(kills, 1); assert.equal(timers.length, 2);
  timers[1].callback(); // Exhaust the separate unproven-cleanup close guard.
  const execution = await result;
  assert.ok(execution.error); assert.equal(execution.status, null); assert.equal(execution.stdout, "");
  assert.equal(unrefs, 1);
  assert.ok(streams.every(stream => stream.destroyed));
  assert.ok(timers.every(timer => timer.cleared));
  child.emit("close", 0); // Late events cannot convert failure into evidence.
  assert.equal(execution.status, null);
});

function windowsPeFixture(marker) {
  const bytes = Buffer.alloc(64 + 24 + 112 + 40 + 1);
  bytes.write("MZ"); bytes.writeUInt32LE(64, 60); bytes.write("PE\0\0", 64);
  bytes.writeUInt16LE(0x8664, 68); bytes.writeUInt16LE(1, 70);
  bytes.writeUInt16LE(112, 84); bytes.writeUInt16LE(2, 86);
  bytes.writeUInt16LE(0x20b, 88); bytes.writeUInt32LE(4096, 144);
  bytes[bytes.length - 1] = marker;
  return bytes;
}

test("Windows binding recomputes runner/helper/source roles and independently binds native subject", async () => {
  const { createWindowsRunnerArtifact, validateWindowsRunnerBinding } = await import("./conformance.mjs");
  const bytes = { runnerBytes: Buffer.from("runner fixture"), helperBytes: windowsPeFixture(1), subjectBytes: windowsPeFixture(2) };
  const artifact = createWindowsRunnerArtifact(bytes.runnerBytes, bytes.helperBytes, "1".repeat(40));
  const job = { source: { commit: "1".repeat(40) }, runner: { name: "coven-automations-conformance-runner", version: artifact.version,
    artifactSha256: artifact.sha256, vectorSetSha256: sha256(canonicalize([])) }, suites: [],
    subjectArtifact: { platform: { os: "win32", arch: "x64" }, sha256: sha256(bytes.subjectBytes) }, environment: { os: "win32", arch: "x64" } };
  assert.deepEqual(validateWindowsRunnerBinding(job, bytes, { os: "win32", arch: "x64" }), artifact);
  for (const mutate of [
    copy => { copy.source.commit = "2".repeat(40); },
    copy => { copy.runner.version = "0.1.0"; },
    copy => { copy.runner.artifactSha256 = sha256(bytes.runnerBytes); },
    copy => { copy.runner.vectorSetSha256 = SHA_B; },
    copy => { copy.subjectArtifact.sha256 = sha256(bytes.helperBytes); },
    copy => { copy.environment.os = "linux"; },
    copy => { copy.subjectArtifact.platform.arch = "arm64"; },
  ]) {
    const copy = structuredClone(job); mutate(copy);
    assert.throws(() => validateWindowsRunnerBinding(copy, bytes, { os: "win32", arch: "x64" }), { message: "Windows runner artifact binding is invalid" });
  }
  for (const changed of [
    { ...bytes, runnerBytes: Buffer.concat([bytes.runnerBytes, Buffer.from("changed")]) },
    { ...bytes, helperBytes: bytes.subjectBytes, subjectBytes: bytes.helperBytes },
    { ...bytes, helperBytes: Buffer.from("PRIVATE fixture") },
    { ...bytes, subjectBytes: Buffer.from("PRIVATE fixture") },
  ]) assert.throws(() => validateWindowsRunnerBinding(job, changed, { os: "win32", arch: "x64" }), { message: "Windows runner artifact binding is invalid" });
  assert.throws(() => validateWindowsRunnerBinding(job, bytes, { os: "win32", arch: "arm64" }), { message: "Windows runner artifact binding is invalid" });
});
