import { mkdtemp, mkdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  harnessSessionRequest,
  isolatedEnvironment,
  socketRequest,
  startDaemon,
  stopDaemon,
  waitForOutputEvent
} from './benchmark-cli.mjs';

const SCHEMA_VERSION = 1;
const DEFAULT_CONCURRENCY = [1, 8, 32];
const POLL_ATTEMPTS = 160;
const POLL_DELAY_MS = 25;

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
      status: 'unavailable',
      reason: 'The current daemon does not expose per-process connection counters.'
    },
    sqliteTransactions: {
      status: 'unavailable',
      reason: 'The current daemon does not expose committed transaction counters.'
    },
    eventQueueDepth: {
      status: 'not_applicable',
      reason: 'Events are persisted synchronously; #596 owns the bounded writer queue.'
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
      status: 'blocked_by_injection',
      reason: 'A cross-platform storage fault hook is not present in the daemon.'
    },
    lockedDatabase: {
      status: 'covered_by_store_regressions',
      reason: 'The store suite owns deterministic SQLite lock assertions.'
    },
    stalledChild: {
      status: 'covered',
      reason: 'Each fixture child remains alive until the measured cancellation request.'
    },
    crashRestart: {
      status: 'covered_by_daemon_regressions',
      reason: 'Daemon recovery tests own process-crash and orphan-recovery assertions.'
    }
  };
}

async function fixtureEnvironment(root, environment) {
  const bin = join(root, 'bin');
  await mkdir(bin, { recursive: true });
  const covenHome = join(root, 'home');
  await mkdir(join(covenHome, 'user-home'), { recursive: true });
  const harness = join(bin, 'codex');
  await writeFile(
    harness,
    '#!/bin/sh\nprintf "COVEN_BENCHMARK_READY\\n"\ntrap "exit 0" INT TERM\nwhile :; do sleep 60; done\n',
    { mode: 0o700 }
  );
  const env = isolatedEnvironment(covenHome, environment);
  return { covenHome, env: { ...env, PATH: `${bin}:${env.PATH ?? ''}` } };
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
  const { covenHome, env } = await fixtureEnvironment(root, environment);
  const socketPath = await startDaemon({ binary, covenHome, env });
  const footprintBefore = await storeFootprint(covenHome);
  let ids = [];

  try {
    const startedAt = process.hrtime.bigint();
    const launches = await Promise.all(
      Array.from({ length: concurrency }, async () => {
        const launchedAt = process.hrtime.bigint();
        const response = await socketRequest(socketPath, harnessSessionRequest({ projectRoot: root }));
        if (response.statusCode !== 201) throw new Error(`launch returned ${response.statusCode}`);
        const id = JSON.parse(response.body).id;
        if (typeof id !== 'string' || id.length === 0) throw new Error('launch response has no id');
        await waitForOutputEvent(socketPath, id, { attempts: POLL_ATTEMPTS, delayMs: POLL_DELAY_MS });
        return { id, firstOutputMs: Number(process.hrtime.bigint() - launchedAt) / 1_000_000 };
      })
    );
    ids = launches.map((launch) => launch.id);
    const completedAt = process.hrtime.bigint();
    const elapsedMs = Number(completedAt - startedAt) / 1_000_000;

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

    return {
      status: 'passed',
      concurrency,
      launchToFirstMeaningfulOutput: summarize(launches.map((launch) => launch.firstOutputMs)),
      throughputSessionsPerSecond: Number((concurrency / (elapsedMs / 1_000)).toFixed(3)),
      cancellation: { allSessionsMs: Number(cancellationMs.toFixed(3)), terminalStates: concurrency },
      diskGrowthBytes: footprintAfter.totalBytes - footprintBefore.totalBytes,
      storage: storageMetricStatus()
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
