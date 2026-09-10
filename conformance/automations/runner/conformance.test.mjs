import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const RUNNER = fileURLToPath(new URL("./conformance.mjs", import.meta.url));
const SHA_A = "a".repeat(64);
const SHA_B = "b".repeat(64);
const SHA_C = "c".repeat(64);
const SHA_D = "d".repeat(64);
const SHA_E = "e".repeat(64);

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
import { chmodSync, writeFileSync } from "node:fs";
const mode = process.env.COVEN_TEST_TARGET_MODE ?? "pass";
if (mode === "unavailable") process.exit(10);
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
  if (mode === "ignore-timeout") {
    process.on("SIGTERM", () => {});
    setTimeout(() => process.exit(10), 5_000);
    await new Promise(() => {});
  }
  if (mode === "descendant-timeout") {
    spawn(process.execPath, ["-e", "setTimeout(() => {}, 5000)"], {
      stdio: ["ignore", process.stdout, process.stderr]
    });
    process.on("SIGTERM", () => {});
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
    profiles: [{ profile: "structural", suites: ["schema-validation"] }]
  }));
  process.exit(0);
}
if (operation !== "evaluate") process.exit(3);
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
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
if (mode === "unsafe-evidence") {
  process.stdout.write('{"schemaVersion":"coven.automations.conformance-suite-result.v1","suiteId":"' + request.suiteId + '","status":"passed","evidence":{"count":9007199254740993}}');
  process.exit(0);
}
process.stdout.write(JSON.stringify({
  schemaVersion: "coven.automations.conformance-suite-result.v1",
  suiteId: request.suiteId,
  status: "passed",
  evidence: { assertion: "native-target-ran", count: 1 }
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
      timeout: 10_000,
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

test("force-kills a target that ignores the graceful timeout", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const startedAt = Date.now();

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "ignore-timeout" },
  });

  const elapsedMs = Date.now() - startedAt;
  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target unavailable\n");
  assert.ok(
    elapsedMs < 3_500,
    `hard timeout took ${elapsedMs}ms`,
  );
});

test("force-kills descendants that retain the target pipes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "coven-conformance-target-"));
  const target = await writeTarget(directory);
  const startedAt = Date.now();

  const result = await runRunner({
    targetCommand: target,
    env: { COVEN_TEST_TARGET_MODE: "descendant-timeout" },
  });

  const elapsedMs = Date.now() - startedAt;
  assert.equal(result.status, 1);
  assert.equal(result.stderr, "conformance target unavailable\n");
  assert.ok(
    elapsedMs < 3_500,
    `process-tree timeout took ${elapsedMs}ms`,
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
