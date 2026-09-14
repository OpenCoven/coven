import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { loadInventory, sourceScopedTarget } from "./audit.mjs";
import { digestBytes, digestCanonical } from "./conformance.mjs";
import { verifyAutomationsProtocolBundle } from "../../../scripts/package-automations-protocol.mjs";

const RUNNER_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(RUNNER_DIR, "../../..");
const MARKER = "PRIVATE-AUDIT-TEST-MARKER";
const REQUIRED = {
  structural: [
    "attempt-terminal-immutability", "capability-negotiation",
    "command-adoption-idempotency", "definition-lifecycle-transitions",
    "definition-validation", "event-reducer-determinism",
    "occurrence-fence-uniqueness", "receipt-integrity-validation",
    "rrule-vocabulary", "run-terminal-monotonicity",
  ],
  scheduler_reliability: [
    "calendar-schedule-resolution", "cancellation-timeout-arbitration",
    "misfire-latest-planning", "occurrence-lease-recovery", "overlap-forbid-claiming",
    "retry-backoff-timing", "retry-quarantine-recovery",
    "scheduler-leadership-fencing", "startup-reconciliation-wake",
  ],
  runtime_authority: ["runtime-authority-terminal-recovery"],
};
const BUILD_ARGS = [
  "build", "--locked", "-p", "coven-cli", "--bin", "coven", "--message-format=json",
];

function json(file) {
  return JSON.parse(readFileSync(file, "utf8"));
}

function write(file, value) {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, value);
}

function git(repo, args) {
  const result = spawnSync("git", args, {
    cwd: repo, encoding: "utf8",
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" },
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

function commit(repo) {
  git(repo, ["add", "."]);
  git(repo, ["-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "test: audit fixture"]);
  return git(repo, ["rev-parse", "HEAD"]);
}

function writeCargoSources(repo, marker) {
  write(path.join(repo, "Cargo.toml"),
    '[workspace]\nmembers = ["crates/coven-cli", "crates/coven-core"]\nresolver = "2"\n');
  write(path.join(repo, "crates/coven-cli/Cargo.toml"), `[package]
name = "coven-cli"
version = "0.0.0"
edition = "2021"
[[bin]]
name = "coven"
path = "src/main.rs"
[dependencies]
coven-core = { path = "../coven-core" }
`);
  write(path.join(repo, "crates/coven-cli/src/main.rs"),
    'fn main() { println!("{}:{}", coven_core::marker(), env!("FIXTURE_COMMIT")); }\n');
  write(path.join(repo, "crates/coven-cli/build.rs"), `fn main() {
    println!("cargo:rerun-if-env-changed=COVEN_BUILD_COMMIT");
    println!("cargo:rustc-env=FIXTURE_COMMIT={}", std::env::var("COVEN_BUILD_COMMIT").expect("audit commit"));
}
`);
  write(path.join(repo, "crates/coven-core/Cargo.toml"),
    '[package]\nname = "coven-core"\nversion = "0.0.0"\nedition = "2021"\n');
  write(path.join(repo, "crates/coven-core/src/lib.rs"),
    `pub fn marker() -> &'static str { "${marker}" }\n`);
}

function fixtureBinary(payload, { os, arch }) {
  const header = Buffer.alloc(64);
  if (os === "darwin") {
    header.writeUInt32LE(0xfeedfacf, 0);
    header.writeUInt32LE({ x64: 0x01000007, arm64: 0x0100000c }[arch], 4);
    header.writeUInt32LE(2, 12);
  } else {
    const elfClass = ["ia32", "arm"].includes(arch) ? 1 : 2;
    header.set([0x7f, 0x45, 0x4c, 0x46, elfClass, 1, 1]);
    header.writeUInt16LE(3, 16);
    header.writeUInt16LE({
      ia32: 3, arm: 40, x64: 62, arm64: 183, ppc64: 21,
      s390x: 22, riscv64: 243, loong64: 258,
    }[arch], 18);
    header.writeUInt32LE(1, 20);
    header.writeUInt16LE(elfClass === 1 ? 52 : 64, elfClass === 1 ? 40 : 52);
  }
  return Buffer.concat([header, Buffer.from(JSON.stringify(payload))]);
}

function fixture(t, {
  runnerSuffix = "",
  runnerTransform = (source) => source,
  sourceTag = "fixture",
  subjectPlatform = { os: process.platform, arch: process.arch },
  subjectTransform = (bytes) => bytes,
  nativeCompilerProbe = false,
} = {}) {
  const directory = realpathSync(mkdtempSync(path.join(tmpdir(), "coven-audit-test-")));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const repo = path.join(directory, "repo");
  const runnerDir = path.join(repo, "conformance/automations/runner");
  mkdirSync(runnerDir, { recursive: true });
  for (const name of ["audit.mjs", "conformance.mjs", "inventory.json",
    ...readdirSync(RUNNER_DIR).filter((entry) => entry.endsWith(".vectors.json"))]) {
    copyFileSync(path.join(RUNNER_DIR, name), path.join(runnerDir, name));
  }
  write(path.join(runnerDir, "conformance.mjs"),
    runnerTransform(readFileSync(path.join(runnerDir, "conformance.mjs"), "utf8")) + runnerSuffix);
  mkdirSync(path.join(repo, "scripts"));
  copyFileSync(
    path.join(ROOT, "scripts/package-automations-protocol.mjs"),
    path.join(repo, "scripts/package-automations-protocol.mjs"),
  );
  write(path.join(repo, "spec/coven-automations/v1/protocol-version.json"),
    '{"contractProfile":"coven.automations.v1","productionReady":false}\n');
  write(path.join(repo, "Cargo.lock"), "# Locked fixture dependencies\nversion = 4\n");
  write(path.join(repo, "Cargo.toml"), '[workspace]\nmembers = ["crates/coven-cli"]\n');
  write(path.join(repo, "crates/coven-cli/Cargo.toml"),
    '[package]\nname = "coven-cli"\nversion = "0.0.0"\nedition = "2021"\n');
  write(path.join(repo, "crates/coven-cli/src/main.rs"), "fn main() {}\n");
  write(path.join(repo, ".gitignore"), "ignored-source/\n");
  const capability = {
    schemaVersion: "coven.automations.conformance-target-capability.v1",
    profiles: Object.entries(REQUIRED).map(([profile, suites]) => ({
      profile, suites,
    })),
  };
  const targetFixture = {
    schemaVersion: "audit-test-target.v1",
    capability,
    evidence: { privateOutput: MARKER, privatePath: "/private/audit-fixture", sourceTag },
  };
  write(path.join(repo, "fixture-target.bin"), subjectTransform(fixtureBinary(targetFixture, subjectPlatform)));
  git(repo, ["init", "--quiet"]);
  git(repo, ["config", "user.name", "Audit Test"]);
  git(repo, ["config", "user.email", "audit-test@users.noreply.github.com"]);
  const head = commit(repo);
  const tools = path.join(directory, "tools");
  const cache = path.join(directory, "cache");
  mkdirSync(tools);
  mkdirSync(cache);
  // Mock only the OS target transport, not conformance.mjs or its artifact
  // checks. Repeated fresh executable launches can exceed its strict deadline
  // before fixture code starts on a busy host. conformance.test.mjs owns real
  // process containment/deadline coverage; these tests own audit orchestration.
  const transport = path.join(tools, "target-transport.mjs");
  write(transport, `
import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { EventEmitter } from "node:events";
import { readFileSync, writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import path from "node:path";
import { PassThrough } from "node:stream";

const realSpawn = childProcess.spawn;
childProcess.spawn = (command, args, options) => {
  if (args?.[0] !== "automations" || args[1] !== "conformance") {
    return realSpawn(command, args, options);
  }
  const buildSource = readFileSync(${JSON.stringify(path.join(directory, "build-source"))}, "utf8");
  assert.equal(process.argv[1], path.join(buildSource, "conformance/automations/runner/conformance.mjs"));
  const bytes = readFileSync(command);
  // Compiler probes check the real retained executable separately; ordinary
  // fixtures interpret the actual runner-staged bytes as their protocol data.
  const fixture = ${nativeCompilerProbe
    ? JSON.stringify(targetFixture)
    : 'JSON.parse(bytes.subarray(64).toString("utf8"))'};
  assert.equal(fixture.schemaVersion, "audit-test-target.v1");
  const child = new EventEmitter();
  child.stdin = new PassThrough();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  let input = "";
  child.stdin.on("data", (chunk) => { input += chunk; });
  child.stdin.on("finish", () => {
    const mode = options.env.AUDIT_TEST_TARGET_MODE || "pass";
    const operation = args.at(-1);
    child.stderr.end("${MARKER}");
    let response;
    let status = 0;
    if (mode === "unavailable") {
      status = 10;
    } else if (operation === "capability") {
      if (mode === "malformed") {
        response = "${MARKER}";
      } else {
        if (mode === "omit") {
          fixture.capability.profiles.find((profile) => profile.profile === "runtime_authority").suites = [];
        }
        response = JSON.stringify(fixture.capability);
      }
    } else {
      assert.equal(operation, "evaluate");
      const request = JSON.parse(input);
      assert.ok(fixture.capability.profiles.some((profile) =>
        profile.profile === request.profile && profile.suites.includes(request.suiteId)));
      if (mode === "change-source") writeFileSync(options.env.AUDIT_TEST_SOURCE_FILE, "${MARKER}");
      if (mode === "change-bundle") writeFileSync(options.env.AUDIT_TEST_BUNDLE, "${MARKER}");
      response = JSON.stringify({
        schemaVersion: "coven.automations.conformance-suite-result.v1",
        suiteId: request.suiteId,
        status: ["failed", "incomplete"].includes(mode) ? mode : "passed",
        evidence: fixture.evidence,
      });
    }
    child.stdout.end(response);
    child.emit("exit", status);
    child.emit("close", status);
  });
  return child;
};
syncBuiltinESMExports();
`);
  const cargo = path.join(tools, "cargo");
  write(cargo, `#!${process.execPath}
const assert = require("node:assert/strict");
const { copyFileSync, chmodSync, existsSync, mkdirSync, writeFileSync } = require("node:fs");
const { spawnSync } = require("node:child_process");
const path = require("node:path");
assert.deepEqual(process.argv.slice(2), ${JSON.stringify(BUILD_ARGS)});
assert.notEqual(process.cwd(), ${JSON.stringify(repo)});
const snapshotHead = spawnSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" });
assert.equal(snapshotHead.status, 0);
assert.equal(snapshotHead.stdout.trim(), ${JSON.stringify(head)});
assert.equal(process.env.COVEN_BUILD_COMMIT, ${JSON.stringify(head)});
assert.equal(process.env.COVEN_BUILD_VERSION, ${JSON.stringify(`audit-${head}`)});
const mode = process.env.AUDIT_TEST_BUILD_MODE || "pass";
process.stderr.write("${MARKER}");
writeFileSync(${JSON.stringify(path.join(directory, "build-invoked"))}, "yes");
writeFileSync(${JSON.stringify(path.join(directory, "build-target"))}, process.env.CARGO_TARGET_DIR);
writeFileSync(${JSON.stringify(path.join(directory, "build-source"))}, process.cwd());
if (mode === "failure") {
  process.stdout.write("${MARKER}");
  process.exit(7);
}
if (mode === "change-source") {
  writeFileSync(${JSON.stringify(path.join(repo, "crates/coven-cli/src/main.rs"))}, "${MARKER}");
}
if (mode === "change-snapshot") {
  writeFileSync("crates/coven-cli/src/main.rs", "${MARKER}");
}
if (mode === "change-head") {
  const changed = spawnSync("git", ["-c", "commit.gpgsign=false", "commit", "--quiet", "--allow-empty", "-m", "test: changed HEAD"], { cwd: ${JSON.stringify(repo)} });
  assert.equal(changed.status, 0);
}
const executable = path.join(process.env.CARGO_TARGET_DIR, "debug/coven");
mkdirSync(path.dirname(executable), { recursive: true });
if (mode !== "reuse-existing" || !existsSync(executable)) {
  copyFileSync("fixture-target.bin", executable);
}
chmodSync(executable, 0o700);
if (mode === "malformed") {
  process.stdout.write("${MARKER}");
  process.exit(0);
}
const artifact = {
  reason: "compiler-artifact",
  package_id: "path+file://" + process.cwd() + "/crates/coven-cli#0.0.0",
  manifest_path: path.join(process.cwd(), "crates/coven-cli/Cargo.toml"),
  target: { name: "coven", kind: ["bin"] },
  profile: { opt_level: "0", debug_assertions: true, test: false },
  executable,
};
if (mode === "mismatched-manifest") artifact.manifest_path = path.join(process.cwd(), "Cargo.toml");
if (mode === "original-manifest") artifact.manifest_path = ${JSON.stringify(path.join(repo, "crates/coven-cli/Cargo.toml"))};
if (mode === "optimized") artifact.profile.opt_level = "3";
if (mode === "outside-target") artifact.executable = path.join(process.cwd(), "fixture-target.bin");
if (mode !== "missing") console.log(JSON.stringify(artifact));
if (mode === "duplicate") console.log(JSON.stringify(artifact));
console.log(JSON.stringify({ reason: "build-finished", success: true }));
`);
  chmodSync(cargo, 0o700);
  return { directory, repo, runnerDir, head, tools, cache, transport };
}

function run(f, { name = "output", output, env = {}, args } = {}) {
  const outputDir = output ?? path.join(f.directory, name);
  const result = spawnSync(process.execPath, [
    path.join(f.runnerDir, "audit.mjs"), ...(args ?? ["--output", outputDir]),
  ], {
    // Deliberately not the source checkout: root discovery must use the module.
    cwd: f.directory,
    env: {
      ...process.env,
      PATH: `${f.tools}${path.delimiter}${process.env.PATH}`,
      CARGO_TARGET_DIR: f.cache,
      CARGO_BUILD_TARGET: "",
      NODE_OPTIONS: [process.env.NODE_OPTIONS, `--import=${pathToFileURL(f.transport).href}`].filter(Boolean).join(" "),
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_CONFIG_GLOBAL: "/dev/null",
      ...env,
    },
    encoding: "utf8",
    timeout: 60_000,
  });
  assert.ifError(result.error);
  assert.doesNotMatch(result.stdout + result.stderr, new RegExp(MARKER));
  assert.ok(!result.stdout.includes(f.directory), "stdout must not include a local source/output path");
  assert.ok(!result.stderr.includes(f.directory), "stderr must not include a local source/output path");
  const report = result.stdout ? JSON.parse(result.stdout) : undefined;
  const nonpassing = report?.statement?.profileResults.flatMap((profile) =>
    profile.suiteResults.filter((suite) => suite.status !== "passed")
      .map(({ suiteId, status }) => ({ suiteId, status })));
  return {
    ...result,
    outputDir,
    diagnostics: `${result.stderr}\n${JSON.stringify(nonpassing ?? [])}`,
  };
}

test("reviewed inventory covers every vector, all 20 suites, and exact metadata", () => {
  const suites = loadInventory();
  assert.equal(suites.length, 20);
  assert.deepEqual(
    Object.fromEntries(Object.keys(REQUIRED).map((profile) =>
      [profile, suites.filter((suite) => suite.profile === profile).map((suite) => suite.suiteId)])),
    REQUIRED,
  );
  assert.deepEqual(
    suites.map((suite) => `${suite.suiteId}.vectors.json`).sort(),
    readdirSync(RUNNER_DIR).filter((name) => name.endsWith(".vectors.json")).sort(),
  );
  for (const suite of suites) {
    assert.equal(suite.vector.schemaVersion, `coven.automations.${suite.suiteId}-vectors.v1`);
    assert.ok(suite.vector.cases.length > 0);
  }
  const manifest = json(path.join(ROOT, "spec/coven-automations/v1/conformance-manifest.json"));
  assert.equal(manifest.productionReady, false);
  assert.equal(manifest.releaseState, "proposed");
});

test("runner digest imports are safe and preserve the canonicalization implementation", () => {
  const url = pathToFileURL(path.join(RUNNER_DIR, "conformance.mjs")).href;
  const result = spawnSync(process.execPath, ["--input-type=module", "-e",
    `const {canonicalize} = await import(${JSON.stringify(url)}); console.log(canonicalize({z:[1,{b:2,a:"test"}],a:0}));`],
  { encoding: "utf8" });
  assert.equal(result.status, 0);
  assert.equal(result.stderr, "");
  assert.equal(result.stdout, '{"a":0,"z":[1,{"a":"test","b":2}]}\n');
});

test("CLI packages, builds, binds and retains reproducible all-suite audit artifacts", (t) => {
  const f = fixture(t);
  const first = run(f);
  assert.equal(first.status, 0, first.diagnostics);
  const borrowedTarget = readFileSync(path.join(f.directory, "build-target"), "utf8");
  const buildSource = readFileSync(path.join(f.directory, "build-source"), "utf8");
  assert.notEqual(buildSource, f.repo);
  assert.ok(!existsSync(buildSource), "owned source snapshot must be removed");
  assert.equal(first.stderr, "");
  const report = JSON.parse(first.stdout);
  assert.equal(report.statement.overallStatus, "passed");
  assert.deepEqual(report.statement.decisionScope, { kind: "audit_only" });
  assert.equal(report.statement.source.commit, f.head);
  assert.equal(report.statement.profileResults.flatMap((profile) => profile.suiteResults).length, 20);
  assert.deepEqual(report, json(path.join(first.outputDir, "result.json")));
  assert.deepEqual(report.statementDigest, digestCanonical(report.statement));
  assert.doesNotMatch(first.stdout, /productionReady|release_eligibility|"full"|privateOutput|privatePath/);

  const job = json(path.join(first.outputDir, "job.json"));
  const provenance = json(path.join(first.outputDir, "provenance.json"));
  const bundle = path.join(first.outputDir, `coven-automations-v1-contract-${f.head}.tar.gz`);
  const verified = verifyAutomationsProtocolBundle({
    bundlePath: bundle, expectedSourceCommit: f.head,
    expectedBundleSha256: job.protocolArtifact.bundleSha256,
  });
  assert.deepEqual(job.protocolArtifact, { bundleSchemaVersion: "coven.automations.bundle.v1", ...verified });
  assert.equal(job.runner.artifactSha256, digestBytes(readFileSync(path.join(f.runnerDir, "conformance.mjs"))));
  assert.equal(job.runner.vectorSetSha256, digestCanonical(job.suites).value);
  assert.equal(job.subjectArtifact.sha256, digestBytes(readFileSync(path.join(first.outputDir, "coven"))));
  assert.equal(job.subjectArtifact.artifactVersion, "0.0.0");
  assert.equal(provenance.cargoLockSha256, digestBytes(readFileSync(path.join(f.repo, "Cargo.lock"))));
  assert.equal(provenance.manifestSha256, digestBytes(readFileSync(path.join(first.outputDir, "manifest.json"))));
  assert.equal(provenance.jobSha256, digestBytes(readFileSync(path.join(first.outputDir, "job.json"))));
  assert.equal(provenance.inventorySha256, digestBytes(readFileSync(path.join(f.runnerDir, "inventory.json"))));
  assert.equal(provenance.auditSha256, digestBytes(readFileSync(path.join(f.runnerDir, "audit.mjs"))));
  assert.equal(provenance.packagerSha256, digestBytes(readFileSync(path.join(f.repo, "scripts/package-automations-protocol.mjs"))));
  assert.deepEqual(provenance.build, {
    command: ["cargo", ...BUILD_ARGS], profile: "debug", locked: true,
    versionStamp: `audit-${f.head}`, commitStamp: f.head,
    sourceInput: "private_pinned_git_checkout",
  });
  assert.deepEqual(provenance.subjectArtifact, job.subjectArtifact);
  assert.equal(provenance.sourceTree, git(f.repo, ["rev-parse", "HEAD^{tree}"]));
  assert.doesNotMatch(JSON.stringify(provenance), /PRIVATE-AUDIT|privateOutput|privatePath|productionReady|release_eligibility/);
  assert.ok(!JSON.stringify(provenance).includes(f.directory));
  assert.ok(!JSON.stringify(provenance).includes(buildSource));
  assert.ok(!first.stdout.includes(buildSource));
  assert.deepEqual(readdirSync(first.outputDir).sort(),
    ["coven", path.basename(bundle), "job.json", "manifest.json", "provenance.json", "result.json"].sort());
  const second = run(f, { name: "second-output", env: { CARGO_TARGET_DIR: "" } });
  assert.equal(second.status, 0, second.diagnostics);
  const nextJob = json(path.join(second.outputDir, "job.json"));
  assert.deepEqual({ ...job, observedAt: null }, { ...nextJob, observedAt: null });
  assert.deepEqual(readFileSync(bundle), readFileSync(path.join(second.outputDir, path.basename(bundle))));
  assert.equal(git(f.repo, ["status", "--porcelain", "--ignored"]), "");
  assert.ok(existsSync(path.join(borrowedTarget, "debug/coven")), "borrowed cache must survive");
  assert.ok(!existsSync(readFileSync(path.join(f.directory, "build-target"), "utf8")),
    "privately created build cache must be removed");
});

test("shared Cargo cache is source-isolated and never consumes unscoped workspace artifacts", (t) => {
  const a = fixture(t, { sourceTag: "SOURCE_A" });
  const b = fixture(t, { sourceTag: "SOURCE_B" });
  b.cache = a.cache;
  const unscoped = path.join(a.cache, "debug/coven");
  write(unscoped, "preserve-unscoped-workspace-cache");

  const warmB = run(b);
  assert.equal(warmB.status, 0, warmB.diagnostics);
  const auditA = run(a, { env: { AUDIT_TEST_BUILD_MODE: "reuse-existing" } });
  assert.equal(auditA.status, 0, auditA.diagnostics);
  assert.equal(
    json(path.join(auditA.outputDir, "job.json")).subjectArtifact.sha256,
    digestBytes(readFileSync(path.join(a.repo, "fixture-target.bin"))),
    "a clean A checkout must not attribute B's cached dependency bytes to A",
  );
  const targetA = readFileSync(path.join(a.directory, "build-target"), "utf8");
  const targetB = readFileSync(path.join(b.directory, "build-target"), "utf8");
  assert.notEqual(targetA, targetB);
  assert.equal(readFileSync(unscoped, "utf8"), "preserve-unscoped-workspace-cache");
  const repeatA = run(a, { name: "repeat", env: { AUDIT_TEST_BUILD_MODE: "reuse-existing" } });
  assert.equal(repeatA.status, 0, repeatA.diagnostics);
  assert.equal(readFileSync(path.join(a.directory, "build-target"), "utf8"), targetA);
});

test("real locked Cargo cannot attribute another workspace's cached dependency to this source", (t) => {
  const directory = realpathSync(mkdtempSync(path.join(tmpdir(), "coven-audit-cargo-test-")));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const sharedTarget = path.join(directory, "target");
  function cargo(repo, args, target, head) {
    const result = spawnSync("cargo", args, {
      cwd: repo,
      env: {
        ...process.env,
        CARGO_TARGET_DIR: target,
        CARGO_BUILD_TARGET: undefined,
        CARGO_NET_OFFLINE: "true",
        COVEN_BUILD_COMMIT: head,
      },
      encoding: "utf8",
      // A hang guard for a dependency-free two-crate build, not a speed claim.
      timeout: 120_000,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 0, "offline Cargo fixture command failed");
  }
  function workspace(name) {
    const repo = path.join(directory, name);
    writeCargoSources(repo, name);
    cargo(repo, ["generate-lockfile", "--offline"], sharedTarget);
    git(repo, ["init", "--quiet"]);
    git(repo, ["config", "user.name", "Audit Test"]);
    git(repo, ["config", "user.email", "audit-test@users.noreply.github.com"]);
    return { repo, head: commit(repo) };
  }
  function marker(executable) {
    const result = spawnSync(executable, [], {
      encoding: "utf8",
      // Only prevent a wedge; source attribution is proven by the output value.
      timeout: 30_000,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 0, "native cache fixture execution failed");
    return result.stdout.trim();
  }

  // Both source trees must predate the warm build to expose mtime-based
  // cross-workspace freshness mistakes in path dependencies.
  const a = workspace("SOURCE_A");
  const b = workspace("SOURCE_B");
  cargo(b.repo, BUILD_ARGS, sharedTarget, b.head);
  const warmBinary = path.join(sharedTarget, "debug/coven");
  assert.equal(marker(warmBinary), `SOURCE_B:${b.head}`);
  cargo(a.repo, BUILD_ARGS, sharedTarget, a.head);
  const unscopedControl = marker(warmBinary);
  assert.match(unscopedControl, /^SOURCE_[AB]:[0-9a-f]{40}$/);
  assert.equal(unscopedControl.split(":")[1], a.head);
  // Cargo versions may fix their freshness heuristic; do not require that
  // upstream bug to persist. The isolated build must always prove SOURCE_A.
  t.diagnostic(`unscoped dependency control: ${unscopedControl.split(":")[0]} with refreshed A commit`);
  const warmDigest = digestBytes(readFileSync(warmBinary));
  const isolatedTarget = sourceScopedTarget(sharedTarget, a.repo, a.head);
  assert.notEqual(isolatedTarget, sharedTarget);
  cargo(a.repo, BUILD_ARGS, isolatedTarget, a.head);
  assert.equal(marker(path.join(isolatedTarget, "debug/coven")), `SOURCE_A:${a.head}`);
  assert.equal(digestBytes(readFileSync(warmBinary)), warmDigest, "borrowed cache must remain untouched");
  assert.equal(git(a.repo, ["status", "--porcelain", "--ignored"]), "");
  assert.equal(git(b.repo, ["status", "--porcelain", "--ignored"]), "");
});

test("transient original-source edits cannot enter the real compiled audited binary", (t) => {
  const lookup = spawnSync("/bin/sh", ["-c", "command -v cargo"], { encoding: "utf8" });
  assert.equal(lookup.status, 0, "Cargo must be available for the offline source snapshot regression");
  const cargoPath = lookup.stdout.trim();
  assert.ok(path.isAbsolute(cargoPath));
  const f = fixture(t, { nativeCompilerProbe: true });
  writeCargoSources(f.repo, "SOURCE_AT_HEAD");
  const lockfile = spawnSync(cargoPath, ["generate-lockfile", "--offline"], {
    cwd: f.repo, encoding: "utf8",
    env: { ...process.env, CARGO_BUILD_TARGET: undefined, CARGO_NET_OFFLINE: "true" },
  });
  assert.equal(lockfile.status, 0, "offline source snapshot fixture lockfile generation failed");
  f.head = commit(f.repo);
  const originalFile = path.join(f.repo, "crates/coven-core/src/lib.rs");
  const originalBytes = readFileSync(originalFile);
  const worktrees = git(f.repo, ["worktree", "list", "--porcelain"]);
  write(path.join(f.tools, "cargo"), `#!${process.execPath}
const assert = require("node:assert/strict");
const { readFileSync, writeFileSync } = require("node:fs");
const { spawnSync } = require("node:child_process");
assert.deepEqual(process.argv.slice(2), ${JSON.stringify(BUILD_ARGS)});
const original = ${JSON.stringify(originalFile)};
const saved = readFileSync(original);
writeFileSync(original, ${JSON.stringify("pub fn marker() -> &'static str { \"SOURCE_TRANSIENT\" }\n")});
writeFileSync(${JSON.stringify(path.join(f.directory, "build-source"))}, process.cwd());
try {
  const result = spawnSync(${JSON.stringify(cargoPath)}, process.argv.slice(2), {
    cwd: process.cwd(), env: process.env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"]
  });
  assert.ifError(result.error);
  process.stdout.write(result.stdout);
  process.exitCode = result.status;
} finally {
  writeFileSync(original, saved);
}
`);
  chmodSync(path.join(f.tools, "cargo"), 0o700);
  const result = run(f, { env: { CARGO_BUILD_TARGET: undefined, CARGO_NET_OFFLINE: "true" } });
  assert.equal(result.status, 0, result.diagnostics);
  assert.deepEqual(readFileSync(originalFile), originalBytes);
  assert.equal(git(f.repo, ["status", "--porcelain", "--ignored"]), "");
  const executable = path.join(result.outputDir, "coven");
  const execution = spawnSync(executable, [], { encoding: "utf8", timeout: 30_000 });
  assert.ifError(execution.error);
  assert.equal(execution.status, 0);
  assert.equal(execution.stdout.trim(), `SOURCE_AT_HEAD:${f.head}`);
  const job = json(path.join(result.outputDir, "job.json"));
  assert.equal(job.source.commit, f.head);
  assert.equal(job.protocolArtifact.sourceCommit, f.head);
  assert.equal(job.subjectArtifact.sha256, digestBytes(readFileSync(executable)));
  const buildSource = readFileSync(path.join(f.directory, "build-source"), "utf8");
  assert.notEqual(buildSource, f.repo);
  assert.ok(!existsSync(buildSource), "owned snapshot must be removed after the audit");
  assert.equal(git(f.repo, ["worktree", "list", "--porcelain"]), worktrees);
});

test("local source snapshots do not execute user checkout hooks", (t) => {
  const f = fixture(t);
  const hooks = path.join(f.directory, "hooks");
  const hook = path.join(hooks, "post-checkout");
  const marker = path.join(f.directory, "checkout-hook-ran");
  write(hook, '#!/bin/sh\nprintf forbidden > "$COVEN_TEST_CHECKOUT_MARKER"\n');
  chmodSync(hook, 0o700);
  const config = path.join(f.directory, "gitconfig");
  write(config, `[core]\n  hooksPath = ${JSON.stringify(hooks)}\n`);
  const result = run(f, { env: {
    GIT_CONFIG_GLOBAL: config,
    COVEN_TEST_CHECKOUT_MARKER: marker,
  } });
  assert.equal(result.status, 0, result.diagnostics);
  assert.ok(!existsSync(marker));
});

test("Git repository overrides cannot redirect snapshot operations", (t) => {
  const f = fixture(t);
  for (const env of [
    { GIT_WORK_TREE: f.repo },
    { GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "core.worktree", GIT_CONFIG_VALUE_0: f.repo },
  ]) {
    const result = run(f, { env });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /Git repository\/configuration overrides/);
    assert.ok(!existsSync(result.outputDir));
  }
  assert.equal(git(f.repo, ["status", "--porcelain", "--ignored"]), "");
});

test("source-qualified Cargo cache cannot be symlinked onto a different cache", (t) => {
  const f = fixture(t);
  const namespace = path.join(f.cache, "coven-automations-audit");
  const foreign = path.join(f.cache, "foreign");
  mkdirSync(namespace);
  mkdirSync(foreign);
  const scoped = path.join(namespace, digestCanonical({ root: f.repo, commit: f.head }).value);
  symlinkSync(foreign, scoped);
  const result = run(f);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /Cargo cache must not alias/);
  assert.ok(!existsSync(path.join(f.directory, "build-invoked")));
  assert.equal(realpathSync(scoped), foreign);
});

test("subject architecture follows the binary header rather than the Node process", (t) => {
  const subjectPlatform = { os: process.platform, arch: process.arch === "x64" ? "arm64" : "x64" };
  const f = fixture(t, { subjectPlatform });
  const result = run(f);
  assert.equal(result.status, 0, result.diagnostics);
  const report = json(path.join(result.outputDir, "result.json"));
  assert.deepEqual(report.statement.subjectArtifact.platform, subjectPlatform);
  assert.equal(report.statement.environment.arch, process.arch);
  assert.notEqual(report.statement.subjectArtifact.platform.arch, report.statement.environment.arch);
});

test("unrecognized, truncated, or foreign native binary headers are rejected", async (t) => {
  const cases = {
    unknown: { subjectTransform: (bytes) => { bytes.fill(0, 0, 4); return bytes; } },
    truncated: { subjectTransform: (bytes) => bytes.subarray(0, 16) },
    foreign: { subjectPlatform: { os: process.platform === "darwin" ? "linux" : "darwin", arch: process.arch } },
    architecture: { subjectTransform: (bytes) => {
      if (process.platform === "darwin") bytes.writeUInt32LE(0, 4);
      else bytes.writeUInt16LE(0, 18);
      return bytes;
    } },
  };
  for (const [name, options] of Object.entries(cases)) {
    await t.test(name, (t) => {
      const f = fixture(t, options);
      const result = run(f);
      assert.equal(result.status, 2, result.diagnostics);
      assert.match(result.stderr, /native binary|architecture|platform/);
      assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
    });
  }
});

test("an unadvertised required suite stays in the job and fails the whole audit", (t) => {
  const f = fixture(t);
  const result = run(f, { env: { AUDIT_TEST_TARGET_MODE: "omit" } });
  assert.equal(result.status, 1);
  const report = json(path.join(result.outputDir, "result.json"));
  assert.equal(report.statement.overallStatus, "incomplete", result.diagnostics);
  assert.equal(report.statement.profileResults.flatMap((profile) => profile.suiteResults).length, 20);
  assert.deepEqual(report.statement.profileResults.at(-1).suiteResults,
    [{ suiteId: "runtime-authority-terminal-recovery", status: "not_applicable" }]);
  assert.equal(json(path.join(result.outputDir, "job.json")).suites.length, 20);
});

test("fixture data cannot claim native execution without the explicit test transport", (t) => {
  const f = fixture(t);
  const result = run(f, { env: { NODE_OPTIONS: "" } });
  // Hosts may refuse the executable before the target capability exchange.
  assert.ok([1, 2].includes(result.status), result.diagnostics);
  if (result.status === 1) {
    const report = json(path.join(result.outputDir, "result.json"));
    assert.notEqual(report.statement.overallStatus, "passed");
    assert.ok(report.statement.profileResults.flatMap((profile) => profile.suiteResults)
      .every((suite) => suite.status !== "passed"));
  } else {
    assert.equal(result.stdout, "");
    assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
  }
});

test("unavailable, failed, incomplete and malformed targets cannot pass or leak output", async (t) => {
  for (const mode of ["unavailable", "failed", "incomplete", "malformed"]) {
    await t.test(mode, (t) => {
      const f = fixture(t);
      const result = run(f, { env: { AUDIT_TEST_TARGET_MODE: mode } });
      assert.equal(result.status, 1, result.stderr);
      const report = json(path.join(result.outputDir, "result.json"));
      assert.notEqual(report.statement.overallStatus, "passed");
      assert.equal(report.statement.profileResults.flatMap((profile) => profile.suiteResults).length, 20);
      assert.doesNotMatch(JSON.stringify(report), new RegExp(MARKER));
    });
  }
});

test("dirty tracked, untracked, ignored and hidden index inputs fail before build", async (t) => {
  for (const mode of ["tracked", "untracked", "ignored", "assume-unchanged", "skip-worktree"]) {
    await t.test(mode, (t) => {
      const f = fixture(t);
      const source = "crates/coven-cli/src/main.rs";
      if (mode === "assume-unchanged" || mode === "skip-worktree") {
        git(f.repo, ["update-index", `--${mode}`, source]);
      }
      write(path.join(f.repo, mode === "untracked" ? "extra.rs"
        : mode === "ignored" ? "ignored-source/main.rs" : source), MARKER);
      const result = run(f);
      assert.equal(result.status, 2);
      assert.match(result.stderr, /dirty|source index hides files/);
      assert.ok(!existsSync(path.join(f.directory, "build-invoked")));
      assert.ok(!existsSync(result.outputDir));
    });
  }
});

test("source or HEAD changes during build, and source changes during execution, fail closed", async (t) => {
  for (const mode of ["change-source", "change-snapshot", "change-head", "execution"]) {
    await t.test(mode, (t) => {
      const f = fixture(t);
      const result = run(f, {
        env: mode === "execution"
          ? { AUDIT_TEST_TARGET_MODE: "change-source", AUDIT_TEST_SOURCE_FILE: path.join(f.repo, "crates/coven-cli/src/main.rs") }
          : { AUDIT_TEST_BUILD_MODE: mode },
      });
      assert.equal(result.status, 2, result.stderr);
      assert.match(result.stderr, /source (checkout is dirty|HEAD changed)/);
      assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
    });
  }
});

test("Cargo invocation failures and mismatched or missing build artifacts fail closed", async (t) => {
  for (const mode of ["failure", "malformed", "missing", "duplicate", "mismatched-manifest", "original-manifest", "optimized", "outside-target"]) {
    await t.test(mode, (t) => {
      const f = fixture(t);
      const result = run(f, { env: { AUDIT_TEST_BUILD_MODE: mode } });
      assert.equal(result.status, 2);
      assert.match(result.stderr, /Cargo/);
      assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
      assert.ok(!existsSync(readFileSync(path.join(f.directory, "build-source"), "utf8")));
    });
  }
});

test("independent runner invocation failure is nonzero even after a passing suite execution", (t) => {
  const f = fixture(t, {
    runnerSuffix: `\nif (process.argv.includes("--job")) { process.stderr.write("${MARKER}"); process.exitCode = 7; }\n`,
  });
  const result = run(f);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /independent conformance runner invocation failed/);
  assert.equal(result.stdout, "");
  assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
});

test("a runner cannot silently drop a required suite from an otherwise passing report", (t) => {
  const f = fixture(t, {
    runnerTransform: (source) => source.replace(
      "    const outcome = await executeJob(job, subject);",
      `    const outcome = await executeJob(job, subject);
    outcome.report.statement.profileResults[0].requiredSuites.pop();
    outcome.report.statement.profileResults[0].suiteResults.pop();
    outcome.report.statementDigest = digestCanonical(outcome.report.statement);`,
    ),
  });
  const result = run(f);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /omitted or changed a required suite/);
  assert.equal(result.stdout, "");
  assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
});

test("unexpected runner result fields are rejected without retaining raw output", (t) => {
  const f = fixture(t, {
    runnerTransform: (source) => source.replace(
      "    const outcome = await executeJob(job, subject);",
      `    const outcome = await executeJob(job, subject);
    outcome.report.statement.privateOutput = "${MARKER}";
    outcome.report.statementDigest = digestCanonical(outcome.report.statement);`,
    ),
  });
  const result = run(f);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /result binding is invalid/);
  assert.equal(result.stdout, "");
  assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
});

test("bundle mutation during target execution invalidates the artifact binding", (t) => {
  const f = fixture(t);
  const result = run(f, { env: {
    AUDIT_TEST_TARGET_MODE: "change-bundle",
    AUDIT_TEST_BUNDLE: path.join(f.directory, "output", `coven-automations-v1-contract-${f.head}.tar.gz`),
  } });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /artifact changed/);
  assert.equal(result.stdout, "");
  assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
});

test("provenance mutation during target execution prevents a passing publication", (t) => {
  const f = fixture(t);
  const result = run(f, { env: {
    AUDIT_TEST_TARGET_MODE: "change-bundle",
    AUDIT_TEST_BUNDLE: path.join(f.directory, "output/provenance.json"),
  } });
  assert.equal(result.status, 2, result.diagnostics);
  assert.match(result.stderr, /artifact changed/);
  assert.equal(result.stdout, "");
  assert.ok(!existsSync(path.join(result.outputDir, "result.json")));
});

test("direct symlink entrypoints execute while imports with non-file argv remain safe", (t) => {
  const directory = mkdtempSync(path.join(tmpdir(), "coven-audit-entrypoint-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const script of ["audit.mjs", "conformance.mjs"]) {
    const original = path.join(RUNNER_DIR, script);
    const alias = path.join(directory, script);
    symlinkSync(original, alias);
    const direct = spawnSync(process.execPath, [alias, "--invalid"], { encoding: "utf8" });
    assert.equal(direct.status, 2, `${script} symlink must execute the argument guard`);
    assert.equal(direct.stdout, "");
    assert.match(direct.stderr, /arguments are invalid|usage:/);
    for (const argv of [path.join(directory, "missing"), "/dev/null/not-a-file"]) {
      const imported = spawnSync(process.execPath, ["--input-type=module", "-e",
        `process.argv[1] = ${JSON.stringify(argv)}; await import(${JSON.stringify(pathToFileURL(original).href)}); console.log("import-safe");`],
      { encoding: "utf8" });
      assert.equal(imported.status, 0, imported.stderr);
      assert.equal(imported.stdout, "import-safe\n");
      assert.equal(imported.stderr, "");
    }
  }
});

test("persisted job cannot run against changed runner, vectors, or subject bytes", (t) => {
  const f = fixture(t);
  const result = run(f);
  assert.equal(result.status, 0, result.diagnostics);
  const job = json(path.join(result.outputDir, "job.json"));
  for (const binding of ["runner", "vector", "subject"]) {
    const altered = structuredClone(job);
    if (binding === "runner") altered.runner.artifactSha256 = "0".repeat(64);
    if (binding === "vector") altered.suites.pop();
    if (binding === "subject") altered.subjectArtifact.sha256 = "0".repeat(64);
    const jobPath = path.join(result.outputDir, `${binding}-job.json`);
    write(jobPath, JSON.stringify(altered));
    const execution = spawnSync(process.execPath, [
      path.join(f.runnerDir, "conformance.mjs"), "--job", jobPath,
      "--target-command", path.join(result.outputDir, "coven"),
    ], { encoding: "utf8", timeout: 10_000 });
    assert.equal(execution.status, 2);
    assert.match(execution.stderr, /binding is invalid/);
    assert.equal(execution.stdout, "");
  }
});

test("inventory deletion, missing vectors, bad metadata, and full-profile requests are rejected", async (t) => {
  for (const mode of ["omitted", "missing", "empty", "wrong-schema", "duplicate", "full", "release"]) {
    await t.test(mode, (t) => {
      const f = fixture(t);
      const inventoryPath = path.join(f.runnerDir, "inventory.json");
      const inventory = json(inventoryPath);
      const vectorPath = path.join(f.runnerDir, inventory.suites.at(-1).vectorFile);
      if (mode === "omitted") inventory.suites.pop();
      if (mode === "missing") rmSync(vectorPath);
      if (mode === "empty") write(vectorPath, JSON.stringify({
        ...json(vectorPath), cases: [],
      }));
      if (mode === "wrong-schema") write(vectorPath, JSON.stringify({
        ...json(vectorPath), schemaVersion: "wrong",
      }));
      if (mode === "duplicate") inventory.suites.push(inventory.suites[0]);
      if (mode === "full") inventory.suites[0].profile = "full";
      if (mode === "release") inventory.decisionScope.kind = "release_eligibility";
      write(inventoryPath, JSON.stringify(inventory));
      commit(f.repo);
      const result = run(f);
      assert.equal(result.status, 2);
      assert.match(result.stderr, /inventory|vector|metadata|JSON/);
      assert.ok(!existsSync(path.join(f.directory, "build-invoked")));
      assert.ok(!existsSync(result.outputDir));
    });
  }
});

test("output collisions and source/cache containment never overwrite or create source artifacts", (t) => {
  const f = fixture(t);
  const occupiedFile = path.join(f.directory, "occupied");
  write(occupiedFile, "preserve");
  const occupiedDir = path.join(f.directory, "occupied-dir");
  mkdirSync(occupiedDir);
  write(path.join(occupiedDir, "keep"), "preserve");
  const sourceAlias = path.join(f.directory, "source-alias");
  symlinkSync(f.repo, sourceAlias);
  const dangling = path.join(f.directory, "dangling");
  symlinkSync(path.join(f.directory, "missing"), dangling);
  for (const output of [
    occupiedFile, occupiedDir, dangling, f.repo,
    path.join(f.repo, "new-output"), path.join(f.repo, ".git/new-output"),
    path.join(sourceAlias, "new-output"),
  ]) {
    const result = run(f, { output });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /outside|already exists/);
  }
  const cacheInside = run(f, { env: { CARGO_TARGET_DIR: path.join(f.repo, "target") } });
  assert.equal(cacheInside.status, 2);
  assert.match(cacheInside.stderr, /outside/);
  const relativeCache = run(f, { env: { CARGO_TARGET_DIR: "target" } });
  assert.equal(relativeCache.status, 2);
  assert.match(relativeCache.stderr, /outside/);
  const scratchInside = run(f, { env: { TMPDIR: f.repo } });
  assert.equal(scratchInside.status, 2);
  assert.match(scratchInside.stderr, /outside/);
  assert.equal(readFileSync(occupiedFile, "utf8"), "preserve");
  assert.equal(readFileSync(path.join(occupiedDir, "keep"), "utf8"), "preserve");
  assert.ok(!existsSync(path.join(f.repo, "new-output")));
  assert.ok(!existsSync(path.join(f.repo, "target")));
  assert.ok(!existsSync(path.join(f.directory, "build-invoked")));
  assert.equal(git(f.repo, ["status", "--porcelain", "--ignored"]), "");
});

test("standalone Windows invocation fails explicitly before creating output", (t) => {
  const directory = mkdtempSync(path.join(tmpdir(), "coven-audit-platform-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const output = path.join(directory, "output");
  const url = pathToFileURL(path.join(RUNNER_DIR, "audit.mjs")).href;
  const execution = spawnSync(process.execPath, ["--input-type=module", "-e", `
    Object.defineProperty(process, "platform", { value: "win32" });
    const { runAudit } = await import(${JSON.stringify(url)});
    try { runAudit({ outputDir: ${JSON.stringify(output)} }); }
    catch (error) { console.error(error.message); process.exitCode = 2; }
  `], { encoding: "utf8" });
  assert.equal(execution.status, 2);
  assert.match(execution.stderr, /Windows is unsupported/);
  assert.ok(!existsSync(output));
});
