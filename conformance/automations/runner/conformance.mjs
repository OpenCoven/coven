#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
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
const TARGET_SCRATCH_ENV = "COVEN_AUTOMATIONS_CONFORMANCE_SCRATCH";
// Bounded audit hang guard, including fresh-executable startup on a busy host.
// This is not a daemon/runtime SLO and does not change any product deadline.
const DEFAULT_TARGET_HANG_GUARD_MS = 10_000;
// Two cases may each use a 5-second synchronization guard, and a failed
// shutdown gets one separate 5-second cleanup window.
const STARTUP_WAKE_TARGET_TIMEOUT_MS = 20_000;
// The two race cases can consume three 5-second synchronization guards; keep
// additional time for fixture setup, SQLite settlement, and process cleanup.
const CANCELLATION_ARBITRATION_TARGET_TIMEOUT_MS = 20_000;
// Real-daemon import cases include startup and restart readiness guards.
// This is a fixture hang guard, not a production latency budget.
const LEGACY_IMPORT_TARGET_TIMEOUT_MS = 600_000;
// Four disposable child fixtures each have a bounded handshake; control exit
// and error cleanup need their own room on loaded hosts. Fixture hang guard
// only: this is not a recovery SLO or a production deadline.
const COMMAND_PROCESS_CRASH_TARGET_TIMEOUT_MS = 120_000;
const TARGET_KILL_GRACE_MS = 100;
const TARGET_OUTPUT_LIMIT = 1024 * 1024;
const MAX_JCS_DEPTH = 128;
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
  if (!isPortableJcsValue(job.suites)) {
    throw new JobError("vector set is not portable JCS");
  }
}

export function canonicalize(value) {
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
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
      index += 1;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      return false;
    }
  }
  return true;
}

function isPortableJcsValue(value) {
  const pending = [{ value, depth: 0 }];
  while (pending.length > 0) {
    const current = pending.pop();
    const candidate = current.value;
    if (candidate === null || typeof candidate === "boolean") continue;
    if (typeof candidate === "string") {
      if (!hasOnlyScalarUnicode(candidate)) return false;
      continue;
    }
    if (typeof candidate === "number") {
      if (
        !Number.isFinite(candidate) ||
        (Number.isInteger(candidate) && !Number.isSafeInteger(candidate))
      ) {
        return false;
      }
      continue;
    }
    if (current.depth >= MAX_JCS_DEPTH) return false;
    if (Array.isArray(candidate)) {
      for (const child of candidate) {
        pending.push({ value: child, depth: current.depth + 1 });
      }
      continue;
    }
    if (!isObject(candidate)) return false;
    for (const [key, child] of Object.entries(candidate)) {
      if (!hasOnlyScalarUnicode(key)) return false;
      pending.push({ value: child, depth: current.depth + 1 });
    }
  }
  return true;
}

export function digestCanonical(value) {
  return {
    algorithm: "sha256",
    canonicalization: "jcs-rfc8785",
    value: createHash("sha256").update(canonicalize(value)).digest("hex"),
  };
}

export function digestBytes(value) {
  return createHash("sha256").update(value).digest("hex");
}

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


export function createWindowsControlReader(nonce) {
  const invalid = () => new JobError("Windows helper control is invalid");
  if (typeof nonce !== "string" || !/^[0-9a-f]{32}$/.test(nonce)) throw invalid();
  let pending = Buffer.alloc(0);
  let ready = false;
  let ackRequested = false;
  let ackComplete = false;
  let terminal;
  let failed = false;
  let finished = false;
  const refuse = () => { failed = true; throw invalid(); };
  const check = () => { if (failed || finished) refuse(); };
  const failures = new Set(["launch_invalid", "containment_unavailable", "controller_lost",
    "deadline_exceeded", "io_refused", "output_limit_exceeded", "cleanup_unproven"]);
  return {
    requestAck() {
      check();
      if (!ready || ackRequested || terminal !== undefined) refuse();
      ackRequested = true;
    },
    completeAck() {
      check();
      if (!ackRequested || ackComplete) refuse();
      ackComplete = true;
    },
    push(chunk, onReady = () => {}) {
      check();
      if (!Buffer.isBuffer(chunk)) refuse();
      let start = 0;
      while (start < chunk.length) {
        if (terminal !== undefined) refuse();
        const end = chunk.indexOf(10, start);
        const stop = end < 0 ? chunk.length : end;
        // Check length before concatenating even an oversized single chunk.
        const length = pending.length + stop - start;
        if (length >= 256) refuse();
        pending = Buffer.concat([pending, chunk.subarray(start, stop)]);
        if (end < 0) return;
        let frame;
        let text;
        try {
          text = new TextDecoder("utf-8", { fatal: true }).decode(pending);
          frame = JSON.parse(text);
        } catch { refuse(); }
        pending = Buffer.alloc(0);
        const extra = frame?.kind === "exited" ? ["exitCode"] : frame?.kind === "refused" ? ["failure"] : [];
        if (!hasExactKeys(frame, ["schemaVersion", "nonce", "kind", ...extra]) ||
          frame.schemaVersion !== "coven.automations.windows-job-supervisor.v1" || frame.nonce !== nonce) refuse();
        // Every permitted value below is a closed primitive. Counting decoded
        // key tokens therefore rejects repeated/escaped aliases without adding
        // a general JSON parser or accepting nested arbitrary control payloads.
        let keys;
        try { keys = [...text.matchAll(/"((?:[^"\\]|\\.)*)"\s*:/g)]
          .map(match => JSON.parse(`"${match[1]}"`));
        } catch { refuse(); }
        if (keys.length !== Object.keys(frame).length || new Set(keys).size !== keys.length) refuse();
        if (frame.kind === "ready") {
          if (ready) refuse();
          ready = true;
          try { onReady(); } catch { refuse(); }
        } else if (frame.kind === "exited") {
          if (!ready || !ackRequested || !Number.isInteger(frame.exitCode) || Object.is(frame.exitCode, -0) ||
            frame.exitCode < 0 || frame.exitCode > 0xffffffff) refuse();
          terminal = { kind: "exited", exitCode: frame.exitCode };
        } else if (frame.kind === "refused") {
          if (!failures.has(frame.failure)) refuse();
          terminal = { kind: "refused", failure: frame.failure };
        } else { refuse(); }
        start = end + 1;
      }
    },
    finish(helperStatus) {
      check();
      if (pending.length !== 0 || terminal === undefined) refuse();
      if (terminal.kind === "exited") {
        if (!ready || !ackComplete || helperStatus !== 0) refuse();
      } else if (helperStatus !== (terminal.failure === "cleanup_unproven" ? 3 : 2)) { refuse(); }
      finished = true;
      return terminal;
    },
  };
}


export async function invokeWindowsHelper({ helper, target, directory, subjectSha256, args, input, timeoutMs, env = process.env }, { spawnHelper = spawn, schedule = setTimeout, cancel = clearTimeout } = {}) {
  const failedResult = (kind = "unavailable") => ({
    error: new Error("Windows helper execution failed"), failureKind: kind, status: null, stdout: "",
  });
  if (![helper, target, directory].every(value => typeof value === "string" && isAbsolute(value) && !value.includes("\0")) ||
    !isStringMatching(subjectSha256, SHA256) || !Buffer.isBuffer(input) || input.length > 64 * 1024 * 1024 ||
    !Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 600000 || !Array.isArray(args) || args.length > 16 ||
    args.some(value => typeof value !== "string" || value.includes("\0") || value.length > 8192)) return failedResult("invalid");
  const nonce = randomBytes(16).toString("hex");
  const header = Buffer.from(JSON.stringify({ schemaVersion: "coven.automations.windows-job-supervisor.v1",
    kind: "launch", nonce, subjectSha256, args, inputBytes: input.length, timeoutMs }) + "\n");
  if (header.length > 8193) return failedResult("invalid");
  return await new Promise(resolve => {
    const control = createWindowsControlReader(nonce);
    let child;
    try {
      child = spawnHelper(helper, [target, directory], { cwd: directory, env,
        stdio: ["overlapped", "overlapped", "overlapped", "overlapped"], windowsHide: true });
    } catch { resolve(failedResult()); return; }
    const stdout = [];
    const ended = [false, false, false];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let inputFinished = false;
    let failureKind;
    let settled = false;
    let killGuard;
    const settle = result => {
      if (settled) return;
      settled = true;
      cancel(watchdog);
      cancel(killGuard);
      resolve(result);
    };
    const fail = (kind = "unavailable") => {
      if (settled || failureKind !== undefined) return;
      failureKind = kind;
      // Kill only the owned helper process. Its private kill-on-close Job
      // Object is the containment backstop; no taskkill/PID tree fallback.
      child.kill("SIGKILL");
      killGuard = schedule(() => {
        // Closure/containment is unproven. Release only our own event-loop and
        // stream ownership so a stuck helper cannot keep the runner referenced.
        // This remains failure even if a late close or terminal record arrives.
        settle(failedResult(kind));
        child.unref();
        for (const stream of child.stdio) stream?.destroy();
      }, 10000);
    };
    // Separate allowance for cooperative job/worker/lease/terminal cleanup,
    // not a target deadline or a ratified SLO. The helper enforces its original
    // validated target timeout while its independent coordinator keeps polling.
    const watchdog = schedule(() => fail(), timeoutMs + 70000);
    child.on("error", () => fail());
    child.stdin.on("error", () => fail("invalid"));
    for (const [index, stream] of [child.stdout, child.stderr, child.stdio[3]].entries()) {
      stream.on("end", () => { ended[index] = true; });
      stream.on("error", () => fail("invalid"));
    }
    child.stdout.on("data", chunk => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > TARGET_OUTPUT_LIMIT) fail("invalid");
      else if (failureKind === undefined) stdout.push(chunk);
    });
    child.stderr.on("data", chunk => {
      stderrBytes += chunk.length;
      if (stderrBytes > TARGET_OUTPUT_LIMIT) fail("invalid");
    });
    child.stdio[3].on("data", chunk => {
      if (failureKind !== undefined || settled) return;
      try {
        control.push(chunk, () => {
          control.requestAck();
          child.stdio[3].write(`GO ${nonce}\n`, error => {
            if (settled || failureKind !== undefined) return;
            if (error) { fail(); return; }
            try { control.completeAck(); } catch { fail("invalid"); }
          });
        });
      } catch { fail("invalid"); }
    });
    child.on("close", helperStatus => {
      if (settled) return;
      if (failureKind !== undefined || !inputFinished || !ended.every(Boolean)) {
        settle(failedResult(failureKind)); return;
      }
      try {
        const terminal = control.finish(helperStatus);
        if (terminal.kind === "refused") {
          settle(failedResult(terminal.failure === "output_limit_exceeded" || terminal.failure === "launch_invalid" ? "invalid" : "unavailable"));
          return;
        }
        const decoded = new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(stdout));
        settle({ error: undefined, failureKind: undefined, status: terminal.exitCode, stdout: decoded });
      } catch { settle(failedResult("invalid")); }
    });
    // Install control handling before queuing body bytes. The ready/ACK path
    // must remain live even while stdin's bounded body buffer is backpressured.
    child.stdin.end(Buffer.concat([header, input]), () => { inputFinished = true; });
    // Keep the duplex lease open until helper exit. Ending it after ACK would
    // be controller loss, not successful completion.
  });
}


export function isWindowsX64Executable(bytes) {
  if (!Buffer.isBuffer(bytes) || bytes.length < 64 || bytes.length > 256 * 1024 * 1024 ||
    bytes.readUInt16LE(0) !== 0x5a4d) return false;
  const offset = bytes.readUInt32LE(60);
  if (offset < 64 || offset > bytes.length - 24 || bytes.readUInt32LE(offset) !== 0x4550) return false;
  const optional = offset + 24;
  const optionalSize = bytes.readUInt16LE(offset + 20);
  const sections = bytes.readUInt16LE(offset + 6);
  const flags = bytes.readUInt16LE(offset + 22);
  if (bytes.readUInt16LE(offset + 4) !== 0x8664 || sections < 1 || sections > 96 || optionalSize < 112 ||
    optional + optionalSize + sections * 40 > bytes.length || (flags & 2) === 0 || (flags & 0x2000) !== 0) return false;
  const imageBytes = bytes.readUInt32LE(optional + 56);
  return bytes.readUInt16LE(optional) === 0x20b && imageBytes > 0 && imageBytes <= 512 * 1024 * 1024;
}

export function validateWindowsRunnerBinding(job, bytes, host = { os: process.platform, arch: process.arch }) {
  try {
    const { runnerBytes, helperBytes, subjectBytes } = bytes;
    if (host.os !== "win32" || host.arch !== "x64" || job.environment.os !== host.os || job.environment.arch !== host.arch ||
      job.subjectArtifact.platform.os !== host.os || job.subjectArtifact.platform.arch !== host.arch ||
      !isWindowsX64Executable(helperBytes) || !isWindowsX64Executable(subjectBytes)) throw new Error();
    const artifact = createWindowsRunnerArtifact(runnerBytes, helperBytes, job.source.commit);
    if (job.runner.name !== "coven-automations-conformance-runner" || job.runner.version !== artifact.version ||
      job.runner.artifactSha256 !== artifact.sha256 || job.runner.vectorSetSha256 !== digestCanonical(job.suites).value ||
      job.subjectArtifact.sha256 !== digestBytes(subjectBytes)) throw new Error();
    return artifact;
  } catch { throw new JobError("Windows runner artifact binding is invalid"); }
}


async function validateArtifactBindings(job, subjectPath, helperPath) {
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
  let helper;
  if (process.platform === "win32") {
    let helperBytes;
    try { helperBytes = await readFile(helperPath); }
    catch { throw new JobError("artifact binding input is unavailable"); }
    validateWindowsRunnerBinding(job, { runnerBytes, helperBytes, subjectBytes });
    helper = { bytes: helperBytes, name: "coven-conformance-supervisor.exe" };
  } else if (job.runner.artifactSha256 !== digestBytes(runnerBytes)) {
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
    sha256: job.subjectArtifact.sha256,
    ...(helper ? { helper } : {}),
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
    // macOS default TMPDIR paths leave too little room for a Unix socket.
    join(process.platform === "darwin" ? "/tmp" : tmpdir(), "coven-conformance-subject-"),
  );
  const target = join(directory, process.platform === "win32" ? "coven.exe" : subject.name);
  writeFileSync(target, subject.bytes, { mode: subject.mode });
  chmodSync(target, subject.mode);

  try {
    const targetTimeoutMs =
      operation === "evaluate" && input?.suiteId === "command-process-crash-recovery"
        ? COMMAND_PROCESS_CRASH_TARGET_TIMEOUT_MS
        : operation === "evaluate" &&
      input?.suiteId === "startup-reconciliation-wake"
        ? STARTUP_WAKE_TARGET_TIMEOUT_MS
        : operation === "evaluate" &&
            input?.suiteId === "cancellation-timeout-arbitration"
          ? CANCELLATION_ARBITRATION_TARGET_TIMEOUT_MS
        : operation === "evaluate" && input?.suiteId === "legacy-import-source-preservation"
          ? LEGACY_IMPORT_TARGET_TIMEOUT_MS
        : DEFAULT_TARGET_HANG_GUARD_MS;
    if (process.platform === "win32") {
      if (!subject.helper) return { error: new Error("Windows helper binding unavailable"), failureKind: "unavailable", status: null, stdout: "" };
      const helper = join(directory, subject.helper.name);
      writeFileSync(helper, subject.helper.bytes, { flag: "wx", mode: 0o700 });
      return await invokeWindowsHelper({ helper, target, directory, subjectSha256: subject.sha256,
        args: ["automations", "conformance", operation],
        input: input === undefined ? Buffer.alloc(0) : Buffer.from(`${JSON.stringify(input)}\n`),
        timeoutMs: targetTimeoutMs, env: { ...process.env, [TARGET_SCRATCH_ENV]: directory } });
    }
    return await new Promise((resolve) => {
      const child = spawn(
        target,
        ["automations", "conformance", operation],
        {
          detached: process.platform !== "win32",
          env: { ...process.env, [TARGET_SCRATCH_ENV]: directory },
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
        targetTimeoutMs,
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
  let helperPath;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const value = argv[index + 1];
    if (argument === "--job" && value !== undefined) {
      jobPath = value;
      index += 1;
    } else if (argument === "--target-command" && value !== undefined) {
      targetCommand = value;
      index += 1;
    } else if (argument === "--containment-helper" && value !== undefined && helperPath === undefined) {
      helperPath = value;
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
  if (helperPath !== undefined && (!isAbsolute(helperPath) || process.platform !== "win32")) {
    throw new JobError("containment helper argument is invalid");
  }
  return { jobPath, target: targetCommand, helperPath };
}

async function main() {
  try {
    const { jobPath, target, helperPath } = parseArguments(process.argv.slice(2));
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
    const subject = await validateArtifactBindings(job, resolvedTarget, helperPath);
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

const entryPath = process.argv[1] === undefined ? undefined :
  await realpath(process.argv[1]).catch((error) => {
    if (error.code !== "ENOENT" && error.code !== "ENOTDIR") throw error;
    return undefined;
  });
if (entryPath === RUNNER_PATH) {
  await main();
}
