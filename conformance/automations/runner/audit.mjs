#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  packageAutomationsProtocol,
  verifyAutomationsProtocolBundle,
} from "../../../scripts/package-automations-protocol.mjs";
import { canonicalize, digestBytes, digestCanonical } from "./conformance.mjs";

const RUNNER_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(RUNNER_DIR, "../../..");
const BUILD_ARGS = [
  "build", "--locked", "-p", "coven-cli", "--bin", "coven", "--message-format=json",
];
const PROFILES = ["structural", "scheduler_reliability", "runtime_authority"];
const STATUSES = ["passed", "failed", "incomplete", "not_applicable"];
const SHA256 = /^[0-9a-f]{64}$/;

class AuditError extends Error {}

function requireAudit(condition, message) {
  if (!condition) throw new AuditError(message);
}

function exactKeys(value, keys) {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    Object.keys(value).sort().join("\0") === [...keys].sort().join("\0");
}

function same(left, right) {
  return canonicalize(left) === canonicalize(right);
}

function readJson(file) {
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch {
    throw new AuditError("required JSON input is unavailable or invalid");
  }
}

function writeJson(file, value) {
  writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx", mode: 0o600 });
}

function git(args, repoRoot = ROOT, { env = process.env, failure = "Git source inspection failed" } = {}) {
  const result = spawnSync("git", ["-c", "core.fsmonitor=false", ...args], {
    cwd: repoRoot,
    env,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    stdio: ["ignore", "pipe", "ignore"],
  });
  requireAudit(!result.error && result.status === 0, failure);
  return result.stdout;
}

function assertCleanSource(commit, repoRoot = ROOT) {
  requireAudit(
    git(["rev-parse", "HEAD"], repoRoot).trim() === commit,
    "source HEAD changed during audit",
  );
  // Ignored inputs can still affect Cargo (for example .cargo/config.toml).
  requireAudit(
    git(["status", "--porcelain=v1", "--untracked-files=all", "--ignored=matching"], repoRoot).trim() === "",
    "source checkout is dirty (including untracked or ignored files)",
  );
  requireAudit(
    git(["ls-files", "-v", "-z"], repoRoot).split("\0").filter(Boolean)
      .every((entry) => entry.startsWith("H ")),
    "source index hides files (assume-unchanged or skip-worktree)",
  );
  requireAudit(
    git(["ls-tree", "-r", "-z", commit], repoRoot).split("\0").filter(Boolean)
      .every((entry) => /^(100644|100755) blob /.test(entry)),
    "source must contain only tracked regular files (no symlinks or submodules)",
  );
}

function sourceRepository() {
  const origin = git(["config", "--get", "remote.origin.url"], ROOT, {
    failure: "source origin must be configured as a credential-free GitHub remote",
  }).trim();
  const match = /^(?:https:\/\/github\.com\/|git@github\.com:|ssh:\/\/git@github\.com\/)([A-Za-z0-9][A-Za-z0-9-]{0,38})\/([A-Za-z0-9_.-]{1,104})\/?$/i.exec(origin);
  requireAudit(match !== null, "source origin must be a credential-free GitHub HTTPS or SSH remote");
  const repository = match[2].replace(/\.git$/i, "");
  requireAudit(repository.length > 0 && repository.length <= 100 &&
    repository !== "." && repository !== "..", "source origin repository is invalid");
  return `https://github.com/${match[1]}/${repository}`;
}

function snapshotSource(root, commit, directory) {
  const sourceRoot = path.join(directory, "checkout");
  const options = {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_"))),
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_SYSTEM: "/dev/null",
      GIT_TERMINAL_PROMPT: "0",
      GIT_OPTIONAL_LOCKS: "0",
    },
    failure: "pinned local source snapshot materialization failed",
  };
  // Local object copying and a pinned checkout never read live working files,
  // use the network, run user checkout hooks, or register a shared worktree.
  git([
    "clone", "--local", "--no-hardlinks", "--no-checkout", "--quiet",
    "--config", "core.hooksPath=/dev/null",
    "--config", "core.autocrlf=false",
    "--config", "core.fsmonitor=false",
    "--config", `core.worktree=${sourceRoot}`,
    "--", root, sourceRoot,
  ], root, options);
  git(["checkout", "--quiet", "--detach", commit], sourceRoot, options);
  assertCleanSource(commit, sourceRoot);
  return sourceRoot;
}

function contains(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" ||
    (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative));
}

// Resolve existing ancestors as well as a not-yet-created leaf, including aliases
// such as macOS /tmp -> /private/tmp. Never create output parents speculatively.
function resolveDestination(destination) {
  const absolute = path.resolve(destination);
  let ancestor = absolute;
  const missing = [];
  while (true) {
    try {
      lstatSync(ancestor);
      return path.join(realpathSync(ancestor), ...missing);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      const parent = path.dirname(ancestor);
      requireAudit(parent !== ancestor, "artifact destination is unavailable");
      missing.unshift(path.basename(ancestor));
      ancestor = parent;
    }
  }
}

function assertExternal(destination, sourceRoots) {
  requireAudit(
    sourceRoots.every((root) => !contains(root, destination)),
    "artifact destination must be outside the source checkout and Git metadata",
  );
}

export function sourceScopedTarget(cacheRoot, root, commit) {
  // Cargo freshness does not establish source identity across workspaces.
  // Never consume unscoped workspace artifacts from the caller's cache.
  const sourceIdentity = digestCanonical({ root: realpathSync(root), commit }).value;
  const requested = path.join(resolveDestination(cacheRoot), "coven-automations-audit", sourceIdentity);
  const resolved = resolveDestination(requested);
  requireAudit(resolved === requested, "source-isolated Cargo cache must not alias another directory");
  return resolved;
}

export function loadInventory(directory = RUNNER_DIR) {
  const inventory = readJson(path.join(directory, "inventory.json"));
  requireAudit(
    exactKeys(inventory, ["schemaVersion", "decisionScope", "suites"]) &&
      inventory.schemaVersion === "coven.automations.audit-inventory.v1" &&
      same(inventory.decisionScope, { kind: "audit_only" }) &&
      Array.isArray(inventory.suites) && inventory.suites.length > 0,
    "reviewed audit inventory is invalid",
  );
  const ids = new Set();
  const files = new Set();
  const suites = inventory.suites.map((entry) => {
    requireAudit(
      exactKeys(entry, ["profile", "suiteId", "vectorFile"]) &&
        PROFILES.includes(entry.profile) &&
        typeof entry.suiteId === "string" && /^[a-z][a-z0-9-]{0,127}$/.test(entry.suiteId) &&
        entry.vectorFile === `${entry.suiteId}.vectors.json` &&
        !ids.has(entry.suiteId) && !files.has(entry.vectorFile),
      "reviewed audit suite metadata is invalid",
    );
    ids.add(entry.suiteId);
    files.add(entry.vectorFile);
    const vector = readJson(path.join(directory, entry.vectorFile));
    requireAudit(
      exactKeys(vector, ["schemaVersion", "cases"]) &&
        vector.schemaVersion === `coven.automations.${entry.suiteId}-vectors.v1` &&
        Array.isArray(vector.cases) && vector.cases.length > 0 &&
        vector.cases.every((item) => typeof item?.caseId === "string" && item.caseId.length > 0) &&
        new Set(vector.cases.map((item) => item.caseId)).size === vector.cases.length,
      "required vector set is empty or has invalid metadata",
    );
    return { profile: entry.profile, suiteId: entry.suiteId, vector };
  });
  requireAudit(
    same([...files].sort(), readdirSync(directory).filter((name) => name.endsWith(".vectors.json")).sort()),
    "reviewed inventory does not cover exactly every vector file",
  );
  return suites;
}

function nativePlatform(bytes) {
  requireAudit(bytes.length >= 64, "native binary header is truncated");
  if (bytes.readUInt32LE(0) === 0xfeedfacf) {
    const arch = { 0x01000007: "x64", 0x0100000c: "arm64" }[bytes.readUInt32LE(4)];
    requireAudit(arch !== undefined && bytes.readUInt32LE(12) === 2,
      "native binary architecture or Mach-O executable header is unsupported");
    return { os: "darwin", arch };
  }
  requireAudit(bytes.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46])),
    "native binary format is unsupported (expected thin Mach-O or ELF)");
  requireAudit([1, 2].includes(bytes[5]) && bytes[6] === 1,
    "native binary ELF encoding is invalid");
  const read16 = (offset) => bytes[5] === 1 ? bytes.readUInt16LE(offset) : bytes.readUInt16BE(offset);
  const read32 = (offset) => bytes[5] === 1 ? bytes.readUInt32LE(offset) : bytes.readUInt32BE(offset);
  const architecture = {
    3: { arch: "ia32", elfClass: 1 },
    21: { arch: "ppc64", elfClass: 2 },
    22: { arch: "s390x", elfClass: 2 },
    40: { arch: "arm", elfClass: 1 },
    62: { arch: "x64", elfClass: 2 },
    183: { arch: "arm64", elfClass: 2 },
    243: { arch: "riscv64", elfClass: 2 },
    258: { arch: "loong64", elfClass: 2 },
  }[read16(18)];
  requireAudit(
    architecture !== undefined && bytes[4] === architecture.elfClass &&
      [2, 3].includes(read16(16)) && read32(20) === 1,
    "native binary architecture or ELF executable header is unsupported",
  );
  return { os: "linux", arch: architecture.arch };
}

function buildSubject(sourceRoot, targetDir, outputDir, commit) {
  const build = spawnSync("cargo", BUILD_ARGS, {
    cwd: sourceRoot,
    env: {
      ...process.env,
      CARGO_TARGET_DIR: targetDir,
      COVEN_BUILD_COMMIT: commit,
      COVEN_BUILD_VERSION: `audit-${commit}`,
    },
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "ignore"],
  });
  requireAudit(!build.error && build.status === 0, "locked debug Cargo build failed");
  let messages;
  try {
    messages = build.stdout.split("\n").filter(Boolean).map((line) => JSON.parse(line));
  } catch {
    throw new AuditError("Cargo build did not emit valid artifact metadata");
  }
  const artifacts = messages.filter((message) =>
    message.reason === "compiler-artifact" && message.target?.name === "coven" &&
    message.target.kind?.includes("bin") && message.executable != null);
  requireAudit(
    messages.some((message) => message.reason === "build-finished" && message.success === true) &&
      artifacts.length === 1,
    "Cargo build did not identify exactly one successful coven binary",
  );
  const artifact = artifacts[0];
  const version = /#(?:coven-cli@)?([0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.+-]+)?)$/.exec(artifact.package_id ?? "")?.[1];
  requireAudit(
    typeof artifact.manifest_path === "string" &&
      realpathSync(artifact.manifest_path) === path.join(sourceRoot, "crates/coven-cli/Cargo.toml") &&
      typeof artifact.executable === "string" && path.isAbsolute(artifact.executable) &&
      artifact.profile?.opt_level === "0" && artifact.profile.debug_assertions === true &&
      artifact.profile.test === false && version !== undefined,
    "Cargo artifact is not the pinned snapshot's conventional debug coven-cli binary",
  );
  requireAudit(
    realpathSync(artifact.executable) === path.join(realpathSync(targetDir), "debug/coven") &&
      lstatSync(artifact.executable).isFile(),
    "Cargo executable is outside the expected debug target",
  );
  const bytes = readFileSync(artifact.executable);
  const platform = nativePlatform(bytes);
  requireAudit(platform.os === process.platform, "native binary platform does not match the audit host");
  const subjectPath = path.join(outputDir, "coven");
  writeFileSync(subjectPath, bytes, { flag: "wx", mode: 0o700 });
  chmodSync(subjectPath, 0o700);
  return { subjectPath, sha256: digestBytes(bytes), version, platform };
}

function isDigest(value) {
  return exactKeys(value, ["algorithm", "canonicalization", "value"]) &&
    value.algorithm === "sha256" && value.canonicalization === "jcs-rfc8785" &&
    typeof value.value === "string" && SHA256.test(value.value);
}

function validateReport(report, job) {
  requireAudit(
    exactKeys(report, ["schemaVersion", "statement", "statementDigest"]) &&
      report.schemaVersion === "coven.automations.conformance-result.v1" &&
      isDigest(report.statementDigest) &&
      same(report.statementDigest, digestCanonical(report.statement)),
    "independent runner result envelope is invalid",
  );
  const statement = report.statement;
  const bindings = [
    "resultId", "decisionScope", "source", "protocolArtifact", "runner",
    "subjectArtifact", "environment", "observedAt",
  ];
  requireAudit(
    exactKeys(statement, [...bindings, "contractProfile", "profileResults", "overallStatus"]) &&
      bindings.every((key) => same(statement[key], job[key])) &&
      statement.contractProfile === "coven.automations.v1" &&
      STATUSES.includes(statement.overallStatus) && Array.isArray(statement.profileResults),
    "independent runner result binding is invalid",
  );
  const profiles = PROFILES.filter((profile) => job.suites.some((suite) => suite.profile === profile));
  requireAudit(
    same(statement.profileResults.map((entry) => entry?.profile), profiles),
    "independent runner omitted or changed a required profile",
  );
  for (const entry of statement.profileResults) {
    const ids = job.suites.filter((suite) => suite.profile === entry.profile)
      .map((suite) => suite.suiteId).sort((a, b) => a.localeCompare(b));
    requireAudit(
      exactKeys(entry, ["profile", "status", "requiredSuites", "suiteResults"]) &&
        STATUSES.includes(entry.status) && same(entry.requiredSuites, ids) &&
        Array.isArray(entry.suiteResults) &&
        same(entry.suiteResults.map((suite) => suite?.suiteId), ids),
      "independent runner omitted or changed a required suite",
    );
    for (const suite of entry.suiteResults) {
      requireAudit(
        exactKeys(suite, suite.evidenceDigest === undefined
          ? ["suiteId", "status"] : ["suiteId", "status", "evidenceDigest"]) &&
          STATUSES.includes(suite.status) &&
          (suite.evidenceDigest === undefined
            ? suite.status !== "passed" : isDigest(suite.evidenceDigest)),
        "independent runner suite evidence is invalid",
      );
    }
  }
  return statement.overallStatus === "passed" &&
    statement.profileResults.every((entry) =>
      entry.status === "passed" && entry.suiteResults.every((suite) => suite.status === "passed"));
}

export function runAudit({ outputDir }) {
  requireAudit(
    ["linux", "darwin"].includes(process.platform),
    "standalone audit supports Linux and macOS only; Windows is unsupported",
  );
  requireAudit(Number(process.versions.node.split(".")[0]) >= 24, "audit requires Node.js 24 or newer");
  requireAudit(typeof outputDir === "string" && outputDir.length > 0, "--output requires a new directory");
  requireAudit(
    !["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
      "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES",
      "GIT_CONFIG_PARAMETERS"].some((key) => process.env[key] !== undefined) &&
      !Object.entries(process.env).some(([key, value]) =>
        /^GIT_CONFIG_KEY_\d+$/.test(key) && value.toLowerCase() === "core.worktree"),
    "Git repository/configuration overrides are unsupported for source snapshots",
  );
  const root = realpathSync(ROOT);
  requireAudit(realpathSync(git(["rev-parse", "--show-toplevel"]).trim()) === root, "audit must run from its source repository");
  const commonDir = realpathSync(path.resolve(ROOT, git(["rev-parse", "--git-common-dir"]).trim()));
  const sourceRoots = [root, commonDir];
  if (path.basename(commonDir) === ".git") sourceRoots.push(path.dirname(commonDir));
  const output = resolveDestination(outputDir);
  assertExternal(output, sourceRoots);
  const commit = git(["rev-parse", "HEAD"]).trim();
  requireAudit(/^[0-9a-f]{40}$/.test(commit), "source HEAD is not a supported Git commit");
  assertCleanSource(commit);
  const repository = sourceRepository();
  // Fail input preflight before reserving output; the job uses snapshot vectors.
  loadInventory();
  const borrowedTarget = process.env.CARGO_TARGET_DIR
    ? path.resolve(ROOT, process.env.CARGO_TARGET_DIR) : undefined;
  if (borrowedTarget) assertExternal(resolveDestination(borrowedTarget), sourceRoots);
  assertExternal(realpathSync(tmpdir()), sourceRoots);
  requireAudit(
    !process.env.CARGO_BUILD_TARGET,
    "cross-compilation is unsupported by this native debug audit",
  );
  // mkdir is exclusive even for an existing empty directory; never reuse output.
  try {
    mkdirSync(output, { mode: 0o700 });
  } catch (error) {
    if (error.code === "EEXIST") throw new AuditError("audit output already exists");
    throw new AuditError("cannot create new audit output directory (parent must exist)");
  }
  let privateTarget;
  let privateSource;
  try {
    privateSource = mkdtempSync(path.join(realpathSync(tmpdir()), "coven-automations-source-"));
    const sourceRoot = snapshotSource(root, commit, privateSource);
    assertCleanSource(commit);
    const sourceRunnerDir = path.join(sourceRoot, "conformance/automations/runner");
    const suites = loadInventory(sourceRunnerDir);
    let packaged;
    let verified;
    try {
      packaged = packageAutomationsProtocol({ repoRoot: sourceRoot, outputDir: output, sourceCommit: commit });
      verified = verifyAutomationsProtocolBundle({
        bundlePath: packaged.bundlePath,
        expectedSourceCommit: commit,
        expectedBundleSha256: packaged.bundleSha256,
      });
    } catch {
      // The packaging helper includes source paths in its errors; keep them local.
      throw new AuditError("source-bound protocol packaging or verification failed");
    }
    const manifestSha256 = digestBytes(readFileSync(packaged.manifestPath));
    assertCleanSource(commit);
    assertCleanSource(commit, sourceRoot);
    const targetDir = borrowedTarget
      ? sourceScopedTarget(borrowedTarget, root, commit)
      : (privateTarget = mkdtempSync(path.join(realpathSync(tmpdir()), "coven-automations-build-")));
    assertExternal(targetDir, sourceRoots);
    const subject = buildSubject(sourceRoot, targetDir, output, commit);
    assertCleanSource(commit);
    assertCleanSource(commit, sourceRoot);
    const runnerPath = path.join(sourceRunnerDir, "conformance.mjs");
    const job = {
      schemaVersion: "coven.automations.conformance-job.v1",
      resultId: `native-audit-${commit}`,
      decisionScope: { kind: "audit_only" },
      source: { repository, commit },
      protocolArtifact: { bundleSchemaVersion: "coven.automations.bundle.v1", ...verified },
      runner: {
        name: "coven-automations-conformance-runner",
        version: "0.1.0",
        artifactSha256: digestBytes(readFileSync(runnerPath)),
        vectorSetSha256: digestCanonical(suites).value,
      },
      subjectArtifact: {
        artifactId: "coven-cli",
        artifactVersion: subject.version,
        platform: subject.platform,
        sha256: subject.sha256,
      },
      environment: { os: process.platform, arch: process.arch, runtime: `node-${process.versions.node}` },
      observedAt: new Date().toISOString(),
      suites,
    };
    const jobPath = path.join(output, "job.json");
    writeJson(jobPath, job);
    const provenancePath = path.join(output, "provenance.json");
    writeJson(provenancePath, {
      schemaVersion: "coven.automations.audit-provenance.v1",
      decisionScope: job.decisionScope,
      source: job.source,
      sourceRepositoryInput: "origin_configuration",
      remoteCommitVerified: false,
      sourceTree: git(["rev-parse", "HEAD^{tree}"], sourceRoot).trim(),
      cargoLockSha256: digestBytes(readFileSync(path.join(sourceRoot, "Cargo.lock"))),
      auditSha256: digestBytes(readFileSync(path.join(sourceRunnerDir, "audit.mjs"))),
      inventorySha256: digestBytes(readFileSync(path.join(sourceRunnerDir, "inventory.json"))),
      packagerSha256: digestBytes(readFileSync(path.join(sourceRoot, "scripts/package-automations-protocol.mjs"))),
      manifestSha256,
      jobSha256: digestBytes(readFileSync(jobPath)),
      build: {
        command: ["cargo", ...BUILD_ARGS], profile: "debug", locked: true,
        versionStamp: `audit-${commit}`, commitStamp: commit,
        sourceInput: "private_pinned_git_checkout",
        trust: "caller_environment",
        hermetic: false,
        toolchainAttested: false,
        reproducibility: "not_assessed",
      },
      protocolArtifact: job.protocolArtifact,
      runner: job.runner,
      subjectArtifact: job.subjectArtifact,
    });
    const provenanceSha256 = digestBytes(readFileSync(provenancePath));
    const execution = spawnSync(process.execPath, [
      runnerPath, "--job", jobPath, "--target-command", subject.subjectPath,
    ], {
      cwd: output,
      encoding: "utf8",
      maxBuffer: 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
    });
    assertCleanSource(commit);
    assertCleanSource(commit, sourceRoot);
    requireAudit(
      !execution.error && [0, 1].includes(execution.status),
      "independent conformance runner invocation failed",
    );
    let report;
    try {
      report = JSON.parse(execution.stdout);
    } catch {
      throw new AuditError("independent conformance runner emitted invalid JSON");
    }
    const passed = validateReport(report, job);
    requireAudit(passed === (execution.status === 0), "independent runner exit status disagrees with suite results");
    requireAudit(
      digestBytes(readFileSync(subject.subjectPath)) === subject.sha256 &&
        digestBytes(readFileSync(runnerPath)) === job.runner.artifactSha256 &&
        digestBytes(readFileSync(packaged.manifestPath)) === manifestSha256 &&
        digestBytes(readFileSync(packaged.bundlePath)) === verified.bundleSha256 &&
        digestBytes(readFileSync(provenancePath)) === provenanceSha256 &&
        same(readJson(jobPath), job),
      "audit artifact changed during execution",
    );
    writeJson(path.join(output, "result.json"), report);
    return { report, exitCode: passed ? 0 : 1 };
  } finally {
    // Only scratch storage created by this invocation is owned by the audit.
    if (privateTarget) rmSync(privateTarget, { recursive: true, force: true });
    if (privateSource) rmSync(privateSource, { recursive: true, force: true });
  }
}

function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    process.stdout.write("Usage: node conformance/automations/runner/audit.mjs --output <new directory>\nBuild and audit every reviewed native suite from clean HEAD (audit_only; Linux/macOS, Node.js 24+).\n");
    return;
  }
  requireAudit(
    args.length === 2 && args[0] === "--output" && args[1] && !args[1].startsWith("--"),
    "usage: audit.mjs --output <new directory>",
  );
  const { report, exitCode } = runAudit({ outputDir: args[1] });
  process.stdout.write(`${JSON.stringify(report)}\n`);
  if (exitCode !== 0) process.stderr.write("audit failed: required suites are unavailable, failed, or incomplete\n");
  process.exitCode = exitCode;
}

const entryPath = process.argv[1] === undefined ? undefined :
  await realpath(process.argv[1]).catch((error) => {
    if (error.code !== "ENOENT" && error.code !== "ENOTDIR") throw error;
    return undefined;
  });
if (entryPath === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`audit failed: ${error instanceof AuditError ? error.message : "audit filesystem or process operation failed"}\n`);
    process.exitCode = 2;
  }
}
