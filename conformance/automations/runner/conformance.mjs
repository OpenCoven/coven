#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdtempSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { readFile, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

const JOB_SCHEMA_VERSION = "coven.automations.conformance-job.v1";
const RESULT_SCHEMA_VERSION = "coven.automations.conformance-result.v1";
const TARGET_CAPABILITY_SCHEMA_VERSION =
  "coven.automations.conformance-target-capability.v1";
const SUITE_REQUEST_SCHEMA_VERSION =
  "coven.automations.conformance-suite-request.v1";
const SUITE_RESULT_SCHEMA_VERSION =
  "coven.automations.conformance-suite-result.v1";
const CONTRACT_PROFILE = "coven.automations.v1";
const TARGET_TIMEOUT_MS = 2_000;
const TARGET_KILL_GRACE_MS = 100;
const TARGET_OUTPUT_LIMIT = 1024 * 1024;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:@-]{0,159}$/;
const SUITE_ID = /^[A-Za-z0-9][A-Za-z0-9._:@-]{0,127}$/;
const VERSION = /^[A-Za-z0-9][A-Za-z0-9._+:-]{0,95}$/;
const ENVIRONMENT_FACT = /^[A-Za-z0-9][A-Za-z0-9._+:-]{0,127}$/;
const SOURCE_COMMIT = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const REPOSITORY =
  /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[A-Za-z0-9!#$%&'()*+,./:;=?@_~-]+$/;
const COMPONENT_PROFILES = [
  "structural",
  "scheduler_reliability",
  "runtime_authority",
  "continuity",
  "privacy",
  "interoperability",
];
const STATUS_ORDER = ["passed", "failed", "incomplete", "not_applicable"];
const RUNNER_PATH = fileURLToPath(import.meta.url);

class JobError extends Error {}

function hasExactKeys(value, required, optional = []) {
  if (!isObject(value)) return false;
  const expected = new Set([...required, ...optional]);
  const keys = Object.keys(value);
  return (
    required.every((key) => Object.hasOwn(value, key)) &&
    keys.every((key) => expected.has(key))
  );
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isStringMatching(value, pattern) {
  return typeof value === "string" && pattern.test(value);
}

function isTimestamp(value) {
  if (typeof value !== "string") return false;
  const match =
    /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{3})?Z$/.exec(
      value,
    );
  if (match === null) return false;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText] =
    match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  if (
    month < 1 ||
    month > 12 ||
    day < 1 ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    return false;
  }
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [
    31,
    leapYear ? 29 : 28,
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ];
  return day <= daysInMonth[month - 1];
}

function isSource(value) {
  return (
    hasExactKeys(value, ["repository", "commit"]) &&
    isStringMatching(value.repository, REPOSITORY) &&
    value.repository.length <= 512 &&
    isStringMatching(value.commit, SOURCE_COMMIT)
  );
}

function isProtocolArtifact(value) {
  return (
    hasExactKeys(value, [
      "bundleSchemaVersion",
      "sourceCommit",
      "bundleSha256",
      "contractContentSha256",
      "fileCount",
    ]) &&
    isStringMatching(value.bundleSchemaVersion, VERSION) &&
    isStringMatching(value.sourceCommit, SOURCE_COMMIT) &&
    isStringMatching(value.bundleSha256, SHA256) &&
    isStringMatching(value.contractContentSha256, SHA256) &&
    Number.isSafeInteger(value.fileCount) &&
    value.fileCount > 0
  );
}

function isRunner(value) {
  return (
    hasExactKeys(value, [
      "name",
      "version",
      "artifactSha256",
      "vectorSetSha256",
    ]) &&
    isStringMatching(value.name, IDENTIFIER) &&
    isStringMatching(value.version, VERSION) &&
    isStringMatching(value.artifactSha256, SHA256) &&
    isStringMatching(value.vectorSetSha256, SHA256)
  );
}

function isPlatform(value) {
  return (
    hasExactKeys(value, ["os", "arch"]) &&
    isStringMatching(value.os, ENVIRONMENT_FACT) &&
    isStringMatching(value.arch, ENVIRONMENT_FACT)
  );
}

function isSubjectArtifact(value) {
  return (
    hasExactKeys(value, [
      "artifactId",
      "artifactVersion",
      "platform",
      "sha256",
    ]) &&
    isStringMatching(value.artifactId, IDENTIFIER) &&
    isStringMatching(value.artifactVersion, VERSION) &&
    isPlatform(value.platform) &&
    isStringMatching(value.sha256, SHA256)
  );
}

function isEnvironment(value) {
  return (
    hasExactKeys(value, ["os", "arch", "runtime"]) &&
    isStringMatching(value.os, ENVIRONMENT_FACT) &&
    isStringMatching(value.arch, ENVIRONMENT_FACT) &&
    isStringMatching(value.runtime, ENVIRONMENT_FACT)
  );
}

function validateJob(job) {
  if (
    !hasExactKeys(job, [
      "schemaVersion",
      "resultId",
      "decisionScope",
      "source",
      "protocolArtifact",
      "runner",
      "subjectArtifact",
      "environment",
      "observedAt",
      "suites",
    ])
  ) {
    throw new JobError("job shape is invalid");
  }
  if (job.schemaVersion !== JOB_SCHEMA_VERSION) {
    throw new JobError("schema version is unsupported");
  }
  if (
    !hasExactKeys(job.decisionScope, ["kind"]) ||
    job.decisionScope.kind !== "audit_only"
  ) {
    throw new JobError("decision scope is not audit-only");
  }
  if (!isStringMatching(job.resultId, IDENTIFIER)) {
    throw new JobError("result id is invalid");
  }
  if (!isSource(job.source)) {
    throw new JobError("source binding is invalid");
  }
  if (!isProtocolArtifact(job.protocolArtifact)) {
    throw new JobError("protocol artifact is invalid");
  }
  if (job.protocolArtifact.sourceCommit !== job.source.commit) {
    throw new JobError("protocol source binding is invalid");
  }
  if (!isRunner(job.runner)) {
    throw new JobError("runner binding is invalid");
  }
  if (!isSubjectArtifact(job.subjectArtifact)) {
    throw new JobError("subject artifact is invalid");
  }
  if (!isEnvironment(job.environment)) {
    throw new JobError("environment is invalid");
  }
  if (!isTimestamp(job.observedAt)) {
    throw new JobError("observed timestamp is invalid");
  }
  if (!Array.isArray(job.suites) || job.suites.length === 0) {
    throw new JobError("suite inventory is invalid");
  }
  if (job.suites.length > 128) {
    throw new JobError("suite inventory is invalid");
  }

  const suiteKeys = new Set();
  for (const suite of job.suites) {
    if (
      !hasExactKeys(suite, ["profile", "suiteId", "vector"]) ||
      !COMPONENT_PROFILES.includes(suite.profile) ||
      !isStringMatching(suite.suiteId, SUITE_ID) ||
      !isObject(suite.vector)
    ) {
      if (suite?.profile === "full") {
        throw new JobError("full profile is unsupported");
      }
      throw new JobError("suite inventory is invalid");
    }
    const key = `${suite.profile}\0${suite.suiteId}`;
    if (suiteKeys.has(key)) {
      throw new JobError("suite inventory is invalid");
    }
    suiteKeys.add(key);
  }
}

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

function hasOnlyScalarUnicode(value) {
  for (let index = 0; index < value.length; index += 1) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) return false;
      index += 1;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      return false;
    }
  }
  return true;
}

function isPortableJcsValue(value) {
  if (value === null || typeof value === "boolean") return true;
  if (typeof value === "string") return hasOnlyScalarUnicode(value);
  if (typeof value === "number") {
    return (
      Number.isFinite(value) &&
      (!Number.isInteger(value) || Number.isSafeInteger(value))
    );
  }
  if (Array.isArray(value)) return value.every(isPortableJcsValue);
  if (!isObject(value)) return false;
  return Object.entries(value).every(
    ([key, child]) =>
      hasOnlyScalarUnicode(key) && isPortableJcsValue(child),
  );
}

function digestCanonical(value) {
  return {
    algorithm: "sha256",
    canonicalization: "jcs-rfc8785",
    value: createHash("sha256").update(canonicalize(value)).digest("hex"),
  };
}

function digestBytes(value) {
  return createHash("sha256").update(value).digest("hex");
}

async function validateArtifactBindings(job, subjectPath) {
  let runnerBytes;
  let subjectBytes;
  try {
    [runnerBytes, subjectBytes] = await Promise.all([
      readFile(RUNNER_PATH),
      readFile(subjectPath),
    ]);
  } catch {
    throw new JobError("artifact binding input is unavailable");
  }
  if (job.runner.artifactSha256 !== digestBytes(runnerBytes)) {
    throw new JobError("runner artifact binding is invalid");
  }
  if (job.runner.vectorSetSha256 !== digestCanonical(job.suites).value) {
    throw new JobError("vector set binding is invalid");
  }
  if (job.subjectArtifact.sha256 !== digestBytes(subjectBytes)) {
    throw new JobError("subject artifact binding is invalid");
  }
  return {
    bytes: subjectBytes,
    mode: statSync(subjectPath).mode & 0o777,
    name: basename(subjectPath),
  };
}

function targetExecutionSucceeded(execution) {
  return !(
    execution.error ||
    execution.status !== 0 ||
    typeof execution.stdout !== "string" ||
    Buffer.byteLength(execution.stdout) > TARGET_OUTPUT_LIMIT
  );
}

function parseTargetJson(execution) {
  if (!targetExecutionSucceeded(execution)) return undefined;
  try {
    return JSON.parse(execution.stdout);
  } catch {
    return undefined;
  }
}

async function invokeTarget(subject, operation, input) {
  const directory = mkdtempSync(
    join(tmpdir(), "coven-conformance-subject-"),
  );
  const target = join(directory, subject.name);
  writeFileSync(target, subject.bytes, { mode: subject.mode });
  chmodSync(target, subject.mode);

  try {
    return await new Promise((resolve) => {
      const child = spawn(
        target,
        ["automations", "conformance", operation],
        {
          detached: process.platform !== "win32",
          stdio: ["pipe", "pipe", "pipe"],
          windowsHide: true,
        },
      );
      const stdout = [];
      let stdoutBytes = 0;
      let outputExceeded = false;
      let executionError;
      let failureKind;
      let killTimer;
      let hardStopTimer;
      let settled = false;
      const killTree = (signal) => {
        if (child.pid === undefined) return;
        if (process.platform === "win32") {
          if (signal === "SIGKILL") {
            spawnSync(
              "taskkill",
              ["/PID", String(child.pid), "/T", "/F"],
              { stdio: "ignore", windowsHide: true },
            );
          } else {
            child.kill(signal);
          }
          return;
        }
        try {
          process.kill(-child.pid, signal);
        } catch {
          child.kill(signal);
        }
      };
      const finish = (status) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (killTimer !== undefined) clearTimeout(killTimer);
        if (hardStopTimer !== undefined) clearTimeout(hardStopTimer);
        let decoded = "";
        try {
          decoded = new TextDecoder("utf-8", { fatal: true }).decode(
            Buffer.concat(stdout),
          );
        } catch (error) {
          executionError = error;
          failureKind = "invalid";
        }
        resolve({
          error: executionError,
          failureKind,
          status,
          stdout: decoded,
        });
      };
      let terminating = false;
      const terminate = (error, kind) => {
        if (terminating) return;
        terminating = true;
        executionError = error;
        failureKind = kind;
        killTree("SIGTERM");
        killTimer = setTimeout(() => {
          killTree("SIGKILL");
          hardStopTimer = setTimeout(
            () => finish(null),
            TARGET_KILL_GRACE_MS,
          );
        }, TARGET_KILL_GRACE_MS);
      };
      const timeout = setTimeout(
        () => terminate(new Error("target timed out"), "unavailable"),
        TARGET_TIMEOUT_MS,
      );

      child.stdout.on("data", (chunk) => {
        stdoutBytes += chunk.length;
        if (stdoutBytes <= TARGET_OUTPUT_LIMIT) {
          stdout.push(chunk);
        } else if (!outputExceeded) {
          outputExceeded = true;
          terminate(new Error("target output exceeded limit"), "invalid");
        }
      });
      child.stderr.resume();
      child.on("error", (error) => {
        executionError = error;
        failureKind = "unavailable";
      });
      child.stdin.on("error", (error) => {
        executionError = error;
        terminate(error, "invalid");
      });
      child.on("exit", () => {
        // A target must not leave helpers behind or keep inherited pipes open.
        killTree("SIGKILL");
      });
      child.on("close", (status) => {
        finish(status);
      });
      if (input === undefined) {
        child.stdin.end();
      } else {
        child.stdin.end(`${JSON.stringify(input)}\n`);
      }
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function parseCapability(execution) {
  const value = parseTargetJson(execution);
  if (
    !hasExactKeys(value, ["schemaVersion", "profiles"]) ||
    value.schemaVersion !== TARGET_CAPABILITY_SCHEMA_VERSION ||
    !Array.isArray(value.profiles)
  ) {
    return undefined;
  }

  const supported = new Set();
  for (const profile of value.profiles) {
    if (
      !hasExactKeys(profile, ["profile", "suites"]) ||
      !COMPONENT_PROFILES.includes(profile.profile) ||
      !Array.isArray(profile.suites)
    ) {
      return undefined;
    }
    for (const suiteId of profile.suites) {
      if (!isStringMatching(suiteId, SUITE_ID)) return undefined;
      supported.add(`${profile.profile}\0${suiteId}`);
    }
  }
  return supported;
}

function parseSuiteResult(execution, suiteId) {
  const value = parseTargetJson(execution);
  if (
    !hasExactKeys(value, ["schemaVersion", "suiteId", "status"], [
      "evidence",
    ]) ||
    value.schemaVersion !== SUITE_RESULT_SCHEMA_VERSION ||
    value.suiteId !== suiteId ||
    !STATUS_ORDER.includes(value.status)
  ) {
    return undefined;
  }
  if (value.status === "passed" && !Object.hasOwn(value, "evidence")) {
    return undefined;
  }
  if (
    Object.hasOwn(value, "evidence") &&
    !isPortableJcsValue(value.evidence)
  ) {
    return undefined;
  }

  const result = { suiteId, status: value.status };
  if (Object.hasOwn(value, "evidence")) {
    result.evidenceDigest = digestCanonical(value.evidence);
  }
  return result;
}

function deriveStatus(statuses) {
  if (statuses.includes("failed")) return "failed";
  if (statuses.every((status) => status === "passed")) return "passed";
  if (statuses.every((status) => status === "not_applicable")) {
    return "not_applicable";
  }
  return "incomplete";
}

function buildReport(job, suiteResults) {
  const byProfile = new Map();
  for (const suite of job.suites) {
    const entry = byProfile.get(suite.profile) ?? [];
    entry.push(suiteResults.get(`${suite.profile}\0${suite.suiteId}`));
    byProfile.set(suite.profile, entry);
  }

  const profileResults = COMPONENT_PROFILES.filter((profile) =>
    byProfile.has(profile),
  ).map((profile) => {
    const results = byProfile
      .get(profile)
      .toSorted((left, right) => left.suiteId.localeCompare(right.suiteId));
    return {
      profile,
      status: deriveStatus(results.map((result) => result.status)),
      requiredSuites: results.map((result) => result.suiteId),
      suiteResults: results,
    };
  });
  const statement = {
    contractProfile: CONTRACT_PROFILE,
    resultId: job.resultId,
    decisionScope: job.decisionScope,
    source: job.source,
    protocolArtifact: job.protocolArtifact,
    runner: job.runner,
    subjectArtifact: job.subjectArtifact,
    environment: job.environment,
    observedAt: job.observedAt,
    profileResults,
    overallStatus: deriveStatus(
      profileResults.map((profile) => profile.status),
    ),
  };
  return {
    schemaVersion: RESULT_SCHEMA_VERSION,
    statement,
    statementDigest: digestCanonical(statement),
  };
}

async function executeJob(job, target) {
  const capabilityExecution = await invokeTarget(target, "capability");
  const capability = parseCapability(capabilityExecution);
  const suiteResults = new Map();

  if (capability === undefined) {
    const targetUnavailable =
      capabilityExecution.failureKind === "unavailable" ||
      (capabilityExecution.failureKind === undefined &&
        capabilityExecution.status !== 0);
    for (const suite of job.suites) {
      suiteResults.set(`${suite.profile}\0${suite.suiteId}`, {
        suiteId: suite.suiteId,
        status: targetUnavailable ? "not_applicable" : "failed",
      });
    }
    return {
      report: buildReport(job, suiteResults),
      message: targetUnavailable
        ? "conformance target unavailable"
        : "conformance target response invalid",
    };
  }

  let invalidResponse = false;
  for (const suite of job.suites) {
    const key = `${suite.profile}\0${suite.suiteId}`;
    if (!capability.has(key)) {
      suiteResults.set(key, {
        suiteId: suite.suiteId,
        status: "not_applicable",
      });
      continue;
    }

    const request = {
      schemaVersion: SUITE_REQUEST_SCHEMA_VERSION,
      profile: suite.profile,
      suiteId: suite.suiteId,
      protocolArtifact: job.protocolArtifact,
      subjectArtifact: job.subjectArtifact,
      vector: suite.vector,
    };
    const result = parseSuiteResult(
      await invokeTarget(target, "evaluate", request),
      suite.suiteId,
    );
    if (result === undefined) {
      invalidResponse = true;
      suiteResults.set(key, {
        suiteId: suite.suiteId,
        status: "failed",
      });
    } else {
      suiteResults.set(key, result);
    }
  }

  const report = buildReport(job, suiteResults);
  return {
    report,
    message: invalidResponse
      ? "conformance target response invalid"
      : report.statement.overallStatus === "passed"
        ? undefined
        : "conformance suites did not pass",
  };
}

function parseArguments(argv) {
  let jobPath;
  let targetCommand;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const value = argv[index + 1];
    if (argument === "--job" && value !== undefined) {
      jobPath = value;
      index += 1;
    } else if (argument === "--target-command" && value !== undefined) {
      targetCommand = value;
      index += 1;
    } else {
      throw new JobError("arguments are invalid");
    }
  }
  if (jobPath === undefined || targetCommand === undefined) {
    throw new JobError("arguments are invalid");
  }
  if (!isAbsolute(targetCommand)) {
    throw new JobError("target command must be absolute");
  }
  return { jobPath, target: targetCommand };
}

async function main() {
  try {
    const { jobPath, target } = parseArguments(process.argv.slice(2));
    if (process.platform === "win32") {
      throw new JobError("runner platform is unsupported");
    }
    let job;
    try {
      job = JSON.parse(await readFile(jobPath, "utf8"));
    } catch {
      throw new JobError("job document is invalid");
    }
    validateJob(job);
    let resolvedTarget;
    try {
      resolvedTarget = await realpath(target);
    } catch {
      throw new JobError("artifact binding input is unavailable");
    }
    const subject = await validateArtifactBindings(job, resolvedTarget);
    const outcome = await executeJob(job, subject);
    process.stdout.write(`${JSON.stringify(outcome.report)}\n`);
    if (outcome.message !== undefined) {
      process.stderr.write(`${outcome.message}\n`);
      process.exitCode = 1;
    }
  } catch (error) {
    const message =
      error instanceof JobError ? error.message : "runner execution failed";
    process.stderr.write(`invalid conformance job: ${message}\n`);
    process.exitCode = 2;
  }
}

await main();
