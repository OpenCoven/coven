import { spawnSync } from 'node:child_process';
import { chmod, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { request as requestHttp } from 'node:http';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const COMMAND_TIMEOUT_MS = 120_000;

export function summarizeSamples(samples) {
  const sorted = [...samples].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);

  return {
    minMs: sorted[0],
    medianMs:
      sorted.length % 2 === 1
        ? sorted[middle]
        : (sorted[middle - 1] + sorted[middle]) / 2,
    p95Ms: sorted[Math.ceil(sorted.length * 0.95) - 1],
    maxMs: sorted.at(-1)
  };
}

export function parseOptions(args) {
  let binary;
  let iterations = 5;
  let output;
  let sessionCounts;

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    const valueFor = (option) =>
      arg === option ? args[++index] : arg.slice(`${option}=`.length);

    if (arg === '--binary' || arg.startsWith('--binary=')) {
      binary = valueFor('--binary');
    } else if (arg === '--iterations' || arg.startsWith('--iterations=')) {
      const value = Number.parseInt(valueFor('--iterations'), 10);
      if (!Number.isSafeInteger(value) || value <= 0) {
        throw new Error('--iterations must be a positive integer');
      }
      iterations = value;
    } else if (arg === '--output' || arg.startsWith('--output=')) {
      output = valueFor('--output');
      if (!output) {
        throw new Error('--output requires a path');
      }
    } else if (arg === '--session-counts' || arg.startsWith('--session-counts=')) {
      const raw = valueFor('--session-counts');
      if (!raw) {
        throw new Error('--session-counts requires a value');
      }
      if (raw === 'none') {
        sessionCounts = [];
      } else {
        const values = raw.split(',').map((value) => Number.parseInt(value, 10));
        if (values.some((value) => !Number.isSafeInteger(value) || value <= 0)) {
          throw new Error('--session-counts must contain positive integers');
        }
        sessionCounts = values;
      }
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }

  if (!binary) {
    throw new Error('--binary is required');
  }

  return { binary, iterations, output, sessionCounts };
}

export function runScenario({ command, args, iterations, allowedExitCodes = [0], env }) {
  const samplesMs = [];
  const exitCodes = [];

  for (let iteration = 0; iteration < iterations; iteration += 1) {
    const startedAt = process.hrtime.bigint();
    const result = spawnSync(command, args, {
      encoding: 'utf8',
      env,
      timeout: COMMAND_TIMEOUT_MS,
      killSignal: 'SIGKILL'
    });
    const elapsedMs = Number(process.hrtime.bigint() - startedAt) / 1_000_000;

    if (result.error) {
      throw result.error;
    }
    if (!Number.isInteger(result.status) || !allowedExitCodes.includes(result.status)) {
      throw new Error(`scenario exited with ${result.status}`);
    }

    samplesMs.push(Number(elapsedMs.toFixed(3)));
    exitCodes.push(result.status);
  }

  return { samplesMs, exitCodes, summary: summarizeSamples(samplesMs) };
}

export function runCommand({
  command,
  args,
  allowedExitCodes = [0],
  env,
  timeoutMs = COMMAND_TIMEOUT_MS
}) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    env,
    timeout: timeoutMs,
    killSignal: 'SIGKILL'
  });
  if (result.error) {
    throw result.error;
  }
  if (!Number.isInteger(result.status) || !allowedExitCodes.includes(result.status)) {
    const stderr = result.stderr.trim();
    throw new Error(`command exited with ${result.status}${stderr ? `: ${stderr}` : ''}`);
  }
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

export function externalSessionRequest({ id, projectRoot }) {
  return {
    method: 'POST',
    path: '/api/v1/sessions/external',
    body: JSON.stringify({
      id,
      projectRoot,
      harness: 'benchmark-fixture',
      title: 'Benchmark fixture session'
    })
  };
}

export function harnessSessionRequest({ projectRoot }) {
  return {
    method: 'POST',
    path: '/api/v1/sessions',
    body: JSON.stringify({
      projectRoot,
      cwd: projectRoot,
      harness: 'codex',
      launchMode: 'nonInteractive',
      prompt: 'Benchmark fixture prompt',
      title: 'Benchmark harness fixture'
    })
  };
}

export function sessionInputRequest(sessionId, index) {
  return {
    method: 'POST',
    path: `/api/v1/sessions/${sessionId}/input`,
    body: JSON.stringify({ data: `Benchmark event ${String(index).padStart(6, '0')}\n` })
  };
}

export function socketRequest(socketPath, {
  method,
  path,
  body = '',
  timeoutMs = COMMAND_TIMEOUT_MS
}) {
  return new Promise((resolve, reject) => {
    const request = requestHttp(
      {
        socketPath,
        method,
        path,
        headers: {
          'content-type': 'application/json',
          'content-length': Buffer.byteLength(body)
        }
      },
      (response) => {
        let responseBody = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => {
          responseBody += chunk;
        });
        response.on('end', () => {
          resolve({ statusCode: response.statusCode, body: responseBody });
        });
      }
    );

    request.once('error', reject);
    request.setTimeout(timeoutMs, () => {
      request.destroy(new Error(`socket request timed out after ${timeoutMs}ms`));
    });
    request.end(body);
  });
}

export async function waitForHealth(socketPath, { attempts, delayMs }) {
  let lastError;

  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const response = await socketRequest(socketPath, {
        method: 'GET',
        path: '/api/v1/health'
      });
      if (response.statusCode === 200 && JSON.parse(response.body).ok === true) {
        return;
      }
      lastError = new Error(`health returned ${response.statusCode}`);
    } catch (error) {
      lastError = error;
    }

    if (attempt + 1 < attempts && delayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }

  throw new Error(`daemon health did not become ready: ${lastError?.message ?? 'unknown error'}`);
}

export async function waitForOutputEvent(
  socketPath,
  sessionId,
  { attempts, delayMs, request = socketRequest }
) {
  let lastError;
  let afterSeq;

  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const cursor = afterSeq === undefined ? '' : `afterSeq=${afterSeq}&`;
      const path = `/api/v1/sessions/${sessionId}/events?${cursor}limit=1`;
      const response = await request(socketPath, { method: 'GET', path });
      if (response.statusCode !== 200) {
        lastError = new Error(`events returned ${response.statusCode}`);
      } else {
        const body = JSON.parse(response.body);
        if (body.events?.some((event) => event.kind === 'output')) {
          return;
        }
        if (Number.isInteger(body.nextCursor?.afterSeq)) {
          afterSeq = body.nextCursor.afterSeq;
        }
        lastError = new Error('no output event yet');
      }
    } catch (error) {
      lastError = error;
    }

    if (attempt + 1 < attempts && delayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }

  throw new Error(`harness output event did not arrive: ${lastError?.message ?? 'unknown error'}`);
}

export async function registerInputEvents({
  socketPath,
  sessionId,
  count,
  request = socketRequest
}) {
  for (let index = 1; index <= count; index += 1) {
    const response = await request(socketPath, sessionInputRequest(sessionId, index));
    if (response.statusCode !== 202) {
      throw new Error(`input event fixture returned ${response.statusCode}`);
    }
  }
}

export async function launchHarnessSession({
  socketPath,
  projectRoot,
  request = socketRequest
}) {
  const response = await request(socketPath, harnessSessionRequest({ projectRoot }));
  if (response.statusCode !== 201) {
    throw new Error(`harness fixture returned ${response.statusCode}`);
  }
  const { id } = JSON.parse(response.body);
  if (typeof id !== 'string' || id.length === 0) {
    throw new Error('harness fixture response has no session id');
  }
  return id;
}

export async function stopLiveSession({
  socketPath,
  sessionId,
  request = socketRequest
}) {
  const response = await request(socketPath, {
    method: 'POST',
    path: `/api/v1/sessions/${sessionId}/kill`
  });
  if (response.statusCode !== 202) {
    throw new Error(`live fixture stop returned ${response.statusCode}`);
  }
}

export async function prepareEventTail({
  socketPath,
  sessionId,
  count,
  request = socketRequest
}) {
  const maxEvents = 1000;
  const tailSize = Math.min(count, maxEvents);
  let remainingBeforeTail = count - tailSize;
  let afterSeq;

  while (remainingBeforeTail > 0) {
    const limit = Math.min(maxEvents, remainingBeforeTail);
    const cursor = afterSeq === undefined ? '' : `afterSeq=${afterSeq}&`;
    const path = `/api/v1/sessions/${sessionId}/events?${cursor}limit=${limit}`;
    const response = await request(socketPath, { method: 'GET', path });
    if (response.statusCode !== 200) {
      throw new Error(`event-tail setup returned ${response.statusCode}`);
    }
    const body = JSON.parse(response.body);
    if (body.events?.length !== limit || !Number.isInteger(body.nextCursor?.afterSeq)) {
      throw new Error('event-tail setup did not return the requested cursor page');
    }
    afterSeq = body.nextCursor.afterSeq;
    remainingBeforeTail -= limit;
  }

  const cursor = afterSeq === undefined ? '' : `afterSeq=${afterSeq}&`;
  return `/api/v1/sessions/${sessionId}/events?${cursor}limit=${tailSize}`;
}

export async function runHarnessOutputScenario({
  socketPath,
  projectRoot,
  iterations,
  request = socketRequest,
  wait = waitForOutputEvent
}) {
  const samplesMs = [];
  const statusCodes = [];

  for (let iteration = 0; iteration < iterations; iteration += 1) {
    const startedAt = process.hrtime.bigint();
    const response = await request(socketPath, harnessSessionRequest({ projectRoot }));
    if (response.statusCode !== 201) {
      throw new Error(`harness fixture returned ${response.statusCode}`);
    }
    const { id } = JSON.parse(response.body);
    if (typeof id !== 'string' || id.length === 0) {
      throw new Error('harness fixture response has no session id');
    }
    await wait(socketPath, id, { attempts: 100, delayMs: 25, request });
    const elapsedMs = Number(process.hrtime.bigint() - startedAt) / 1_000_000;
    samplesMs.push(Number(elapsedMs.toFixed(3)));
    statusCodes.push(response.statusCode);
  }

  return { samplesMs, statusCodes, summary: summarizeSamples(samplesMs) };
}

export async function createHarnessFixture(fixtureRoot, environment = process.env) {
  const binDir = join(fixtureRoot, 'bin');
  await mkdir(binDir, { recursive: true });
  const executable = join(binDir, 'codex');
  await writeFile(executable, '#!/bin/sh\nprintf "benchmark output\\n"\n', { mode: 0o700 });
  await chmod(executable, 0o700);
  return { ...environment, PATH: `${binDir}:${environment.PATH ?? ''}` };
}

export async function createInputHarnessFixture(fixtureRoot, environment = process.env) {
  const binDir = join(fixtureRoot, 'event-bin');
  await mkdir(binDir, { recursive: true });
  const executable = join(binDir, 'codex');
  await writeFile(executable, '#!/bin/sh\nwhile IFS= read -r line; do :; done\n', { mode: 0o700 });
  await chmod(executable, 0o700);
  return { ...environment, PATH: `${binDir}:${environment.PATH ?? ''}` };
}

export function buildReport({ iterations, sessionCounts, scenarios, environment = process.env }) {
  const report = {
    schemaVersion: 1,
    platform: {
      os: process.platform,
      arch: process.arch,
      node: process.version
    },
    options: { iterations, sessionCounts },
    scenarios
  };
  if (environment.GITHUB_SHA) {
    report.commit = environment.GITHUB_SHA;
  }
  return report;
}

export function coreScenarioDefinitions(binary) {
  return [
    { id: 'help', command: binary, args: ['--help'], allowedExitCodes: [0] },
    { id: 'version', command: binary, args: ['--version'], allowedExitCodes: [0] },
    { id: 'doctor', command: binary, args: ['doctor'], allowedExitCodes: [0, 1] }
  ];
}

export function collectCoreScenarios({ binary, iterations, env, run = runScenario }) {
  return Object.fromEntries(
    coreScenarioDefinitions(binary).map(({ id, ...definition }) => [
      id,
      run({ ...definition, iterations, env })
    ])
  );
}

export function isolatedEnvironment(covenHome, environment = process.env) {
  const userHome = join(covenHome, 'user-home');
  const xdgConfigHome = join(userHome, '.config');
  const xdgCacheHome = join(userHome, '.cache');
  const xdgStateHome = join(userHome, '.local', 'state');
  return {
    ...environment,
    COVEN_HOME: covenHome,
    HOME: userHome,
    USERPROFILE: userHome,
    XDG_CONFIG_HOME: xdgConfigHome,
    XDG_CACHE_HOME: xdgCacheHome,
    XDG_STATE_HOME: xdgStateHome
  };
}

export async function daemonSocketPath(covenHome) {
  const status = JSON.parse(await readFile(join(covenHome, 'daemon.json'), 'utf8'));
  if (typeof status.socket !== 'string' || status.socket.trim() === '') {
    throw new Error('daemon metadata has no socket path');
  }
  return status.socket;
}

export async function startDaemon({
  binary,
  covenHome,
  env,
  run = runCommand,
  readSocket = daemonSocketPath,
  wait = waitForHealth
}) {
  run({ command: binary, args: ['daemon', 'start'], allowedExitCodes: [0], env });
  const socketPath = await readSocket(covenHome);
  await wait(socketPath, { attempts: 100, delayMs: 25 });
  return socketPath;
}

export function stopDaemon({ binary, env, run = runCommand }) {
  run({ command: binary, args: ['daemon', 'stop'], allowedExitCodes: [0], env });
}

export async function registerExternalSessions({
  socketPath,
  count,
  concurrency = 1,
  projectRoot,
  request = socketRequest
}) {
  let nextIndex = 1;
  const workerCount = Math.min(count, concurrency);

  const worker = async () => {
    while (nextIndex <= count) {
      const index = nextIndex;
      nextIndex += 1;
      const id = `benchmark-session-${String(index).padStart(6, '0')}`;
      const response = await request(socketPath, externalSessionRequest({ id, projectRoot }));
      if (response.statusCode !== 200 && response.statusCode !== 201) {
        throw new Error(`external session fixture returned ${response.statusCode}`);
      }
    }
  };

  await Promise.all(Array.from({ length: workerCount }, worker));
}

export async function runSocketScenario({
  socketPath,
  path,
  iterations,
  allowedStatusCodes = [200],
  request = socketRequest
}) {
  const samplesMs = [];
  const statusCodes = [];

  for (let iteration = 0; iteration < iterations; iteration += 1) {
    const startedAt = process.hrtime.bigint();
    const response = await request(socketPath, { method: 'GET', path });
    const elapsedMs = Number(process.hrtime.bigint() - startedAt) / 1_000_000;
    if (!allowedStatusCodes.includes(response.statusCode)) {
      throw new Error(`socket scenario returned ${response.statusCode}`);
    }
    samplesMs.push(Number(elapsedMs.toFixed(3)));
    statusCodes.push(response.statusCode);
  }

  return { samplesMs, statusCodes, summary: summarizeSamples(samplesMs) };
}

export async function measureSessionLists({
  binary,
  fixtureRoot,
  sessionCounts,
  environment,
  fixtureConcurrency = 8,
  iterations = 1,
  makeDirectory = (path) => mkdir(path, { recursive: true }),
  start = startDaemon,
  seed = registerExternalSessions,
  measure = runSocketScenario,
  stop = stopDaemon
}) {
  const reports = {};

  for (const count of sessionCounts) {
    const covenHome = join(fixtureRoot, `s-${count}`);
    const env = isolatedEnvironment(covenHome, environment);
    await makeDirectory(join(covenHome, 'user-home'));
    const socketPath = await start({ binary, covenHome, env });

    try {
      await seed({
        socketPath,
        count,
        concurrency: fixtureConcurrency,
        projectRoot: fixtureRoot
      });
      reports[`sessions_${count}`] = await measure({
        socketPath,
        path: '/api/v1/sessions?limit=100',
        iterations
      });
    } finally {
      stop({ binary, covenHome, env });
    }
  }

  return reports;
}

export async function measureCapabilityReads({
  binary,
  fixtureRoot,
  environment,
  iterations,
  makeDirectory = (path) => mkdir(path, { recursive: true }),
  start = startDaemon,
  measure = runSocketScenario,
  stop = stopDaemon
}) {
  // Unix-domain socket paths have a small platform limit (104 bytes on macOS).
  // Keep this fixture name shorter than the session fixtures so a default
  // macOS temporary directory still leaves enough room for `coven.sock`.
  const covenHome = join(fixtureRoot, 'k');
  const env = isolatedEnvironment(covenHome, environment);
  await makeDirectory(join(covenHome, 'user-home'));
  const socketPath = await start({ binary, covenHome, env });

  try {
    await measure({
      socketPath,
      path: '/api/v1/capabilities/harnesses',
      iterations: 1
    });
    return await measure({
      socketPath,
      path: '/api/v1/capabilities/harnesses',
      iterations
    });
  } finally {
    stop({ binary, covenHome, env });
  }
}

export async function measureHarnessOutput({
  binary,
  fixtureRoot,
  environment,
  iterations,
  makeDirectory = (path) => mkdir(path, { recursive: true }),
  createFixture = createHarnessFixture,
  start = startDaemon,
  measure = runHarnessOutputScenario,
  stop = stopDaemon
}) {
  const covenHome = join(fixtureRoot, 'h');
  const baseEnvironment = isolatedEnvironment(covenHome, environment);
  await makeDirectory(join(covenHome, 'user-home'));
  const env = await createFixture(fixtureRoot, baseEnvironment);
  const socketPath = await start({ binary, covenHome, env });

  try {
    return await measure({ socketPath, projectRoot: fixtureRoot, iterations });
  } finally {
    stop({ binary, covenHome, env });
  }
}

export async function measureEventTails({
  binary,
  fixtureRoot,
  eventCounts,
  environment,
  iterations = 1,
  makeDirectory = (path) => mkdir(path, { recursive: true }),
  createFixture = createInputHarnessFixture,
  start = startDaemon,
  launch = launchHarnessSession,
  seed = registerInputEvents,
  prepare = prepareEventTail,
  measure = runSocketScenario,
  finish = stopLiveSession,
  stop = stopDaemon
}) {
  const reports = {};

  for (const count of eventCounts) {
    const covenHome = join(fixtureRoot, `e-${count}`);
    const baseEnvironment = isolatedEnvironment(covenHome, environment);
    await makeDirectory(join(covenHome, 'user-home'));
    const env = await createFixture(fixtureRoot, baseEnvironment);
    const socketPath = await start({ binary, covenHome, env });
    let sessionId;

    try {
      sessionId = await launch({ socketPath, projectRoot: fixtureRoot });
      await seed({ socketPath, sessionId, count });
      const path = await prepare({ socketPath, sessionId, count });
      reports[`event_tail_${count}`] = await measure({ socketPath, path, iterations });
    } finally {
      if (sessionId) {
        await finish({ socketPath, sessionId });
      }
      stop({ binary, covenHome, env });
    }
  }

  return reports;
}

export async function collectBenchmarkScenarios({
  options,
  fixtureRoot,
  environment,
  makeDirectory = (path) => mkdir(path, { recursive: true }),
  collectCore = collectCoreScenarios,
  measureHarness = measureHarnessOutput,
  measureEvents = measureEventTails,
  measureLists = measureSessionLists,
  measureCapabilities = measureCapabilityReads
}) {
  const coreHome = join(fixtureRoot, 'c');
  const coreEnv = isolatedEnvironment(coreHome, environment);
  await makeDirectory(join(coreHome, 'user-home'));
  const scenarios = collectCore({
    binary: options.binary,
    iterations: options.iterations,
    env: coreEnv
  });

  if (options.sessionCounts.length > 0) {
    scenarios.harness_first_output = await measureHarness({
      binary: options.binary,
      fixtureRoot,
      environment,
      iterations: options.iterations
    });
    Object.assign(
      scenarios,
      await measureEvents({
        binary: options.binary,
        fixtureRoot,
        eventCounts: options.sessionCounts,
        environment,
        iterations: options.iterations
      })
    );
    Object.assign(
      scenarios,
      await measureLists({
        binary: options.binary,
        fixtureRoot,
        sessionCounts: options.sessionCounts,
        environment,
        iterations: options.iterations
      })
    );
    scenarios.capabilities_hot = await measureCapabilities({
      binary: options.binary,
      fixtureRoot,
      environment,
      iterations: options.iterations
    });
  }

  return scenarios;
}

export async function main(args = process.argv.slice(2)) {
  const options = parseOptions(args);
  const sessionCounts = options.sessionCounts ?? [100, 1000, 10000];
  const fixtureRoot = await mkdtemp(join(tmpdir(), 'coven-benchmark-'));

  try {
    const report = buildReport({
      iterations: options.iterations,
      sessionCounts,
      environment: process.env,
      scenarios: await collectBenchmarkScenarios({
        options: { ...options, sessionCounts },
        fixtureRoot,
        environment: process.env
      })
    });

    const serialized = `${JSON.stringify(report)}\n`;
    if (options.output) {
      const temporaryOutput = `${options.output}.${process.pid}.tmp`;
      await writeFile(temporaryOutput, serialized, { encoding: 'utf8', mode: 0o600 });
      await rename(temporaryOutput, options.output);
    } else {
      process.stdout.write(serialized);
    }
    return report;
  } finally {
    await rm(fixtureRoot, { recursive: true, force: true });
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`benchmark-cli: ${error.message}\n`);
    process.exitCode = 1;
  });
}
