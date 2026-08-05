import { chmod, mkdtemp, mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  harnessSessionRequest,
  isolatedEnvironment,
  runCommand,
  socketRequest,
  startDaemon,
  stopDaemon,
  waitForOutputEvent
} from './benchmark-cli.mjs';

const SCHEMA_VERSION = 3;
const DEFAULT_CONCURRENCY = [1, 8, 32];
const POLL_ATTEMPTS = 160;
const POLL_DELAY_MS = 25;
// The first scenario absorbs every cold-start cost — daemon spawn, store
// initialization, first PTY — inside a single sample, so launch-to-first-output
// varies by well over an order of magnitude with host load: the same machine has
// produced 642 ms and 20.3 s for `sessions_1`.  `POLL_ATTEMPTS` allows roughly
// four seconds of polling, which a two-core CI runner misses routinely.  This
// budget exists only to bound a hang; it does not affect the reported metric,
// which is measured from the clock, not from the number of polls.
const LAUNCH_POLL_ATTEMPTS = 2400;
const EVENT_WRITER_DIAGNOSTIC_FIELDS = [
  'state',
  'queuedEvents',
  'queuedBytes',
  'capacityBytes',
  'droppedOutputEvents',
  'droppedOutputBytes',
  'connectionOpens',
  'transactions',
  'committedEvents',
  'lastError'
];

function optionValue(args, index, option) {
  const arg = args[index];
  if (arg === option) return [args[index + 1], index + 1];
  return [arg.slice(`${option}=`.length), index];
}

export function parseOptions(args) {
  let binary;
  let output;
  let concurrency = DEFAULT_CONCURRENCY;

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === '--binary' || arg.startsWith('--binary=')) {
      [binary, index] = optionValue(args, index, '--binary');
    } else if (arg === '--output' || arg.startsWith('--output=')) {
      [output, index] = optionValue(args, index, '--output');
    } else if (arg === '--concurrency' || arg.startsWith('--concurrency=')) {
      const [raw, nextIndex] = optionValue(args, index, '--concurrency');
      index = nextIndex;
      concurrency = raw?.split(',').map((value) => Number.parseInt(value, 10)) ?? [];
      if (
        concurrency.length === 0 ||
        concurrency.some((value) => !Number.isSafeInteger(value) || value <= 0)
      ) {
        throw new Error('--concurrency must contain positive integers');
      }
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }

  if (!binary) throw new Error('--binary is required');
  if (!output) throw new Error('--output is required');
  return { binary, output, concurrency };
}

export function summarize(samples) {
  if (samples.length === 0) throw new Error('cannot summarize no samples');
  const sorted = [...samples].sort((left, right) => left - right);
  const percentile = (p) => sorted[Math.ceil(sorted.length * p) - 1];
  return {
    count: sorted.length,
    minMs: sorted[0],
    p50Ms: percentile(0.5),
    p95Ms: percentile(0.95),
    p99Ms: percentile(0.99),
    maxMs: sorted.at(-1)
  };
}

export function redactedEnvironment(environment = process.env) {
  return {
    platform: process.platform,
    arch: process.arch,
    node: process.version,
    ci: environment.GITHUB_ACTIONS === 'true'
  };
}

export function storageMetricStatus() {
  return {
    sqliteConnectionOpens: {
      status: 'measured',
      source: 'eventWriter.connectionOpens'
    },
    sqliteTransactions: {
      status: 'measured',
      source: 'eventWriter.transactions'
    },
    eventQueueDepth: {
      status: 'measured',
      source: 'maxima of periodic eventWriter.queuedEvents/eventWriter.queuedBytes samples'
    },
    rss: {
      status: 'measured',
      source: 'daemon PID sampled through coven pc top --json'
    }
  };
}

export function chaosCoverage() {
  return {
    slowClient: {
      status: 'covered_by_cave',
      reason: 'Cave owns WebSocket consumer buffering and replay (#4317).'
    },
    diskFull: {
      status: 'covered_by_storage_regressions',
      reason: 'A deterministic free-space seam exercises the fail-closed path without filling a host disk.',
      evidence: 'store::tests::scheduled_maintenance_below_watermark_does_not_open_or_write_the_store'
    },
    lockedDatabase: {
      status: 'covered_by_event_writer_regressions',
      reason: 'The writer suite holds real SQLite locks across bounded retry and recovery assertions.',
      evidence: 'event_writer::tests::transient_sqlite_lock_is_retried'
    },
    stalledChild: {
      status: 'covered',
      reason: 'Each fixture child remains alive until the measured cancellation request.'
    },
    crashRestart: {
      status: 'covered_by_daemon_regressions',
      reason: 'Daemon recovery tests own process-crash and orphan-recovery assertions.',
      evidence: 'daemon::tests::recovers_persisted_running_sessions_as_orphaned'
    }
  };
}

function requiredCounter(writer, field) {
  const value = writer?.[field];
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`event writer health has invalid ${field}`);
  }
  return value;
}

export function summarizeRuntimeMetrics(samples) {
  if (samples.length === 0) throw new Error('cannot summarize runtime metrics without samples');
  const first = samples[0].eventWriter;
  const last = samples.at(-1).eventWriter;
  const opensStart = requiredCounter(first, 'connectionOpens');
  const opensEnd = requiredCounter(last, 'connectionOpens');
  const transactionsStart = requiredCounter(first, 'transactions');
  const transactionsEnd = requiredCounter(last, 'transactions');
  const rssSamples = samples
    .map((sample) => sample.residentSetBytes)
    .filter((value) => Number.isSafeInteger(value) && value >= 0);
  if (rssSamples.length === 0) throw new Error('runtime metrics contain no RSS samples');

  return {
    sqliteConnectionOpens: {
      start: opensStart,
      end: opensEnd,
      delta: opensEnd - opensStart
    },
    sqliteTransactions: {
      start: transactionsStart,
      end: transactionsEnd,
      delta: transactionsEnd - transactionsStart
    },
    eventQueueDepth: {
      maxSampledEvents: Math.max(...samples.map((sample) => requiredCounter(sample.eventWriter, 'queuedEvents'))),
      maxSampledBytes: Math.max(...samples.map((sample) => requiredCounter(sample.eventWriter, 'queuedBytes')))
    },
    rss: {
      samplesBytes: rssSamples,
      peakBytes: Math.max(...rssSamples)
    }
  };
}

export function residentSetBytesFromProcessList(output, daemonPid) {
  const processes = JSON.parse(output)?.processes;
  if (!Array.isArray(processes)) throw new Error('pc top output has no process list');
  const daemon = processes.find((process) => process?.pid === daemonPid);
  if (!daemon) throw new Error(`daemon pid ${daemonPid} is absent from pc top output`);
  if (!Number.isSafeInteger(daemon.memory_mb) || daemon.memory_mb < 0) {
    throw new Error(`daemon pid ${daemonPid} has invalid RSS`);
  }
  return daemon.memory_mb * 1024 * 1024;
}

async function daemonResidentSetBytes(binary, covenHome, env) {
  const daemon = JSON.parse(await readFile(join(covenHome, 'daemon.json'), 'utf8'));
  if (!Number.isSafeInteger(daemon.pid) || daemon.pid <= 0) {
    throw new Error('daemon metadata has no valid pid');
  }
  const result = runCommand({
    command: binary,
    args: ['pc', 'top', '--json', '--n', '100000'],
    env
  });
  return residentSetBytesFromProcessList(result.stdout, daemon.pid);
}

async function runtimeHealthSnapshot(socketPath, request = socketRequest) {
  const response = await request(socketPath, { method: 'GET', path: '/api/v1/health' });
  if (response.statusCode !== 200) throw new Error(`health returned ${response.statusCode}`);
  const health = JSON.parse(response.body);
  if (!health.eventWriter || typeof health.eventWriter !== 'object') {
    throw new Error('health has no live event writer metrics');
  }
  return { eventWriter: health.eventWriter };
}

async function observeWriterHealth(socketPath, samples, isRunning) {
  while (isRunning()) {
    samples.push(await runtimeHealthSnapshot(socketPath));
    await new Promise((resolve) => setTimeout(resolve, POLL_DELAY_MS));
  }
}

async function fullRuntimeSnapshot({ binary, covenHome, env, socketPath }) {
  const snapshot = await runtimeHealthSnapshot(socketPath);
  snapshot.residentSetBytes = await daemonResidentSetBytes(binary, covenHome, env);
  return snapshot;
}

async function fixtureEnvironment(root, environment) {
  const bin = join(root, 'bin');
  await mkdir(bin, { recursive: true });
  const covenHome = join(root, 'home');
  await mkdir(join(covenHome, 'user-home'), { recursive: true });
  const harness = join(bin, 'codex');
  const markerPath = join(root, 'fixture-executions.log');
  await writeFile(harness, fixtureHarnessScript(), { mode: 0o700 });
  // `writeFile`'s `mode` applies only when the file is created and is masked by
  // the process umask, so it cannot be relied on to leave the bit set.  A
  // harness without it spawns nothing, no output event is ever recorded, and
  // the wait below fails as an indistinguishable timeout — so assert the bit
  // rather than trusting either call.
  await chmod(harness, 0o700);
  await assertExecutable(harness);
  const env = isolatedEnvironment(covenHome, environment);
  return {
    covenHome,
    markerPath,
    env: {
      ...env,
      COVEN_BENCHMARK_MARKERS: markerPath,
      PATH: `${bin}:${env.PATH ?? ''}`
    }
  };
}

export function fixtureHarnessScript() {
  return '#!/bin/sh\nprintf "started\\n" >> "$COVEN_BENCHMARK_MARKERS"\nprintf "COVEN_BENCHMARK_READY\\n"\ntrap "exit 0" INT TERM\nwhile :; do sleep 60; done\n';
}

async function assertExecutable(path) {
  const mode = (await stat(path)).mode & 0o777;
  if ((mode & 0o100) === 0) {
    throw new Error(`fixture harness ${path} is not owner-executable (mode ${mode.toString(8)})`);
  }
}

function redactedDiagnosticText(value, redactions) {
  let text = String(value).replace(/[\u0000-\u001F\u007F]+/g, ' ').trim();
  for (const path of [...redactions].sort((left, right) => right.length - left.length)) {
    if (path) text = text.replaceAll(path, '<fixture>');
  }
  return text.slice(0, 240);
}

function boundedDiagnosticValue(value, redactions) {
  if (value === null) return 'null';
  if (value === undefined) return 'unavailable';
  const text = redactedDiagnosticText(value, redactions);
  return typeof value === 'string' ? JSON.stringify(text) : text;
}

export function formatEventWriterHealth(health, redactions = []) {
  const writer = health?.eventWriter;
  if (!writer || typeof writer !== 'object') return 'eventWriter=unavailable';
  const fields = EVENT_WRITER_DIAGNOSTIC_FIELDS.map(
    (field) => {
      const value =
        field === 'state' && /^[a-z_]+$/.test(writer[field] ?? '')
          ? writer[field]
          : boundedDiagnosticValue(writer[field], redactions);
      return `${field}=${value}`;
    }
  );
  return `eventWriter={${fields.join(' ')}}`;
}

async function fixtureExecutionSummary(markerPath, expectedExecutions, read) {
  try {
    const markers = await read(markerPath, 'utf8');
    const count = markers.split('\n').filter((line) => line === 'started').length;
    return `fixtureExecutions=${count}/${expectedExecutions}`;
  } catch (error) {
    if (error?.code === 'ENOENT') return `fixtureExecutions=0/${expectedExecutions}`;
    return `fixtureExecutions=unavailable/${expectedExecutions}`;
  }
}

/// Describe each boundary involved in first output, so a timeout distinguishes
/// child execution, PTY/event ingestion, writer pressure, and read-path failure.
export async function describeScenarioFailure({
  socketPath,
  sessionId,
  markerPath,
  expectedExecutions,
  request = socketRequest,
  read = readFile
}) {
  let sessionEvidence;
  try {
    const [session, events] = await Promise.all([
      request(socketPath, { method: 'GET', path: `/api/v1/sessions/${sessionId}` }),
      request(socketPath, { method: 'GET', path: `/api/v1/sessions/${sessionId}/events?limit=20` })
    ]);
    const status = session.statusCode === 200 ? JSON.parse(session.body).status : `HTTP ${session.statusCode}`;
    const kinds =
      events.statusCode === 200
        ? (JSON.parse(events.body).events ?? []).map((event) => event.kind)
        : [`HTTP ${events.statusCode}`];
    sessionEvidence = `status=${status} events=[${kinds.join(', ') || 'none'}]`;
  } catch (error) {
    sessionEvidence = `sessionState=unavailable(${boundedDiagnosticValue(
      error instanceof Error ? error.message : error,
      [dirname(markerPath)]
    )})`;
  }

  const fixtureEvidence = await fixtureExecutionSummary(markerPath, expectedExecutions, read);
  let writerEvidence;
  try {
    const response = await request(socketPath, { method: 'GET', path: '/api/v1/health' });
    writerEvidence =
      response.statusCode === 200
        ? formatEventWriterHealth(JSON.parse(response.body), [dirname(markerPath)])
        : `eventWriter=HTTP_${response.statusCode}`;
  } catch (error) {
    writerEvidence = `eventWriter=unavailable(${boundedDiagnosticValue(
      error instanceof Error ? error.message : error,
      [dirname(markerPath)]
    )})`;
  }

  return `${sessionEvidence} ${fixtureEvidence} ${writerEvidence}`;
}

export function formatScenarioTimeout({
  concurrency,
  error,
  diagnostic,
  fixtureRoot
}) {
  const message = redactedDiagnosticText(
    error instanceof Error ? error.message : error,
    [fixtureRoot]
  );
  return `sessions_${concurrency}: ${message} — ${diagnostic}`;
}

async function waitForSessionExit(socketPath, sessionId, request = socketRequest) {
  let lastError;
  for (let attempt = 0; attempt < POLL_ATTEMPTS; attempt += 1) {
    try {
      const response = await request(socketPath, {
        method: 'GET',
        path: `/api/v1/sessions/${sessionId}`
      });
      if (response.statusCode === 200) {
        const session = JSON.parse(response.body);
        if (session.status !== 'running') return session.status;
      } else {
        lastError = new Error(`session lookup returned ${response.statusCode}`);
      }
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_DELAY_MS));
  }
  throw new Error(`session did not leave running state: ${lastError?.message ?? 'unknown error'}`);
}

async function fileSize(path) {
  try {
    return (await stat(path)).size;
  } catch (error) {
    if (error?.code === 'ENOENT') return 0;
    throw error;
  }
}

export async function storeFootprint(covenHome) {
  const database = join(covenHome, 'coven.sqlite3');
  const [databaseBytes, walBytes, shmBytes] = await Promise.all([
    fileSize(database),
    fileSize(`${database}-wal`),
    fileSize(`${database}-shm`)
  ]);
  return { databaseBytes, walBytes, shmBytes, totalBytes: databaseBytes + walBytes + shmBytes };
}

export async function runConcurrencyScenario({ binary, root, concurrency, environment = process.env }) {
  const { covenHome, env, markerPath } = await fixtureEnvironment(root, environment);
  const socketPath = await startDaemon({ binary, covenHome, env });
  let ids = [];

  try {
    const footprintBefore = await storeFootprint(covenHome);
    const runtimeSamples = [await fullRuntimeSnapshot({ binary, covenHome, env, socketPath })];
    const startedAt = process.hrtime.bigint();
    let observing = true;
    const observation = observeWriterHealth(socketPath, runtimeSamples, () => observing);
    let launches;
    let completedAt;
    try {
      launches = await Promise.all(
        Array.from({ length: concurrency }, async () => {
          const launchedAt = process.hrtime.bigint();
          const response = await socketRequest(socketPath, harnessSessionRequest({ projectRoot: root }));
          if (response.statusCode !== 201) throw new Error(`launch returned ${response.statusCode}`);
          const id = JSON.parse(response.body).id;
          if (typeof id !== 'string' || id.length === 0) throw new Error('launch response has no id');
          ids.push(id);
          try {
            await waitForOutputEvent(socketPath, id, {
              attempts: LAUNCH_POLL_ATTEMPTS,
              delayMs: POLL_DELAY_MS
            });
          } catch (error) {
            const diagnostic = await describeScenarioFailure({
              socketPath,
              sessionId: id,
              markerPath,
              expectedExecutions: concurrency
            });
            throw new Error(formatScenarioTimeout({
              concurrency,
              error,
              diagnostic,
              fixtureRoot: root
            }));
          }
          return { id, firstOutputMs: Number(process.hrtime.bigint() - launchedAt) / 1_000_000 };
        })
      );
      completedAt = process.hrtime.bigint();
    } finally {
      observing = false;
      await observation;
    }
    const elapsedMs = Number(completedAt - startedAt) / 1_000_000;
    runtimeSamples.push(await fullRuntimeSnapshot({ binary, covenHome, env, socketPath }));

    const cancellationStartedAt = process.hrtime.bigint();
    await Promise.all(
      ids.map(async (id) => {
        const response = await socketRequest(socketPath, { method: 'POST', path: `/api/v1/sessions/${id}/kill` });
        if (response.statusCode !== 202) throw new Error(`cancel returned ${response.statusCode}`);
        await waitForSessionExit(socketPath, id);
      })
    );
    const cancellationMs = Number(process.hrtime.bigint() - cancellationStartedAt) / 1_000_000;
    ids = [];
    const footprintAfter = await storeFootprint(covenHome);
    runtimeSamples.push(await fullRuntimeSnapshot({ binary, covenHome, env, socketPath }));

    return {
      status: 'passed',
      concurrency,
      launchToFirstMeaningfulOutput: summarize(launches.map((launch) => launch.firstOutputMs)),
      throughputSessionsPerSecond: Number((concurrency / (elapsedMs / 1_000)).toFixed(3)),
      cancellation: { allSessionsMs: Number(cancellationMs.toFixed(3)), terminalStates: concurrency },
      diskGrowthBytes: footprintAfter.totalBytes - footprintBefore.totalBytes,
      storage: {
        availability: storageMetricStatus(),
        measurements: summarizeRuntimeMetrics(runtimeSamples)
      }
    };
  } finally {
    await Promise.all(
      ids.map((id) => socketRequest(socketPath, { method: 'POST', path: `/api/v1/sessions/${id}/kill` }).catch(() => {}))
    );
    stopDaemon({ binary, env });
  }
}

export function buildReport({ concurrency, scenarios, environment = process.env }) {
  return {
    schemaVersion: SCHEMA_VERSION,
    environment: redactedEnvironment(environment),
    matrix: { concurrency, scenarios },
    chaos: chaosCoverage()
  };
}

export function validateReport(report) {
  if (report?.schemaVersion !== SCHEMA_VERSION) throw new Error('unsupported report schema');
  for (const expected of DEFAULT_CONCURRENCY) {
    if (!report.matrix.concurrency.includes(expected)) {
      throw new Error(`required concurrency ${expected} is absent`);
    }
    if (report.matrix.scenarios[`sessions_${expected}`]?.status !== 'passed') {
      throw new Error(`sessions_${expected} did not pass`);
    }
  }
  return report;
}

export async function main(args = process.argv.slice(2)) {
  const options = parseOptions(args);
  for (const required of DEFAULT_CONCURRENCY) {
    if (!options.concurrency.includes(required)) {
      throw new Error(`full baseline requires concurrency ${required}`);
    }
  }
  const fixtureRoot = await mkdtemp(join(tmpdir(), 'coven-chaos-baseline-'));
  try {
    const scenarios = {};
    for (const concurrency of options.concurrency) {
      scenarios[`sessions_${concurrency}`] = await runConcurrencyScenario({
        binary: options.binary,
        root: join(fixtureRoot, `s-${concurrency}`),
        concurrency
      });
    }
    const report = validateReport(buildReport({ concurrency: options.concurrency, scenarios }));
    const serialized = `${JSON.stringify(report)}\n`;
    const temporary = `${options.output}.${process.pid}.tmp`;
    await writeFile(temporary, serialized, { encoding: 'utf8', mode: 0o600 });
    await rename(temporary, options.output);
    return report;
  } finally {
    await rm(fixtureRoot, { recursive: true, force: true });
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`benchmark-chaos: ${error.message}\n`);
    process.exitCode = 1;
  });
}
