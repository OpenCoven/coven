import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

import {
  buildReport,
  collectBenchmarkScenarios,
  collectCoreScenarios,
  coreScenarioDefinitions,
  daemonSocketPath,
  externalSessionRequest,
  harnessSessionRequest,
  isolatedEnvironment,
  launchHarnessSession,
  prepareEventTail,
  measureEventTails,
  measureCapabilityReads,
  measureSessionLists,
  registerInputEvents,
  registerExternalSessions,
  runCommand,
  runHarnessOutputScenario,
  parseOptions,
  runScenario,
  runSocketScenario,
  sessionInputRequest,
  stopLiveSession,
  socketRequest,
  startDaemon,
  stopDaemon,
  summarizeSamples,
  waitForOutputEvent,
  waitForHealth
} from './benchmark-cli.mjs';

test('summarizeSamples reports deterministic median and nearest-rank p95', () => {
  assert.deepEqual(summarizeSamples([9, 1, 5, 3, 7]), {
    minMs: 1,
    medianMs: 5,
    p95Ms: 9,
    maxMs: 9
  });
});

test('parseOptions requires a binary path', () => {
  assert.throws(() => parseOptions(['--iterations=3']), /--binary is required/);
});

test('parseOptions rejects a non-positive iteration count', () => {
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--iterations=0']),
    /--iterations must be a positive integer/
  );
});

test('parseOptions rejects unknown flags', () => {
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--interations=3']),
    /unknown option: --interations=3/
  );
});

test('parseOptions accepts an output path and explicit session fixture sizes', () => {
  assert.deepEqual(
    parseOptions([
      '--binary=/tmp/coven',
      '--iterations=3',
      '--output=/tmp/report.json',
      '--session-counts=100,1000,10000'
    ]),
    {
      binary: '/tmp/coven',
      iterations: 3,
      output: '/tmp/report.json',
      sessionCounts: [100, 1000, 10000]
    }
  );
});

test('parseOptions rejects a missing output path', () => {
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--output']),
    /--output requires a path/
  );
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--output=']),
    /--output requires a path/
  );
});

test('parseOptions accepts space-separated option values', () => {
  assert.deepEqual(
    parseOptions([
      '--binary',
      '/tmp/coven',
      '--iterations',
      '3',
      '--output',
      '/tmp/report.json',
      '--session-counts',
      '100,1000'
    ]),
    {
      binary: '/tmp/coven',
      iterations: 3,
      output: '/tmp/report.json',
      sessionCounts: [100, 1000]
    }
  );
});

test('parseOptions accepts none as a core-only session fixture mode', () => {
  assert.deepEqual(
    parseOptions(['--binary=/tmp/coven', '--session-counts=none']).sessionCounts,
    []
  );
});

test('parseOptions rejects a missing session fixture size', () => {
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--session-counts']),
    /--session-counts requires a value/
  );
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--session-counts=']),
    /--session-counts requires a value/
  );
});

test('buildReport omits the local binary path and fixture home', () => {
  const report = buildReport({
    binary: '/private/tmp/coven',
    fixtureHome: '/private/tmp/coven-home',
    iterations: 3,
    sessionCounts: [100],
    scenarios: { help: { samplesMs: [1], exitCodes: [0], summary: { minMs: 1 } }
    }
  });

  const serialized = JSON.stringify(report);
  assert.equal(report.schemaVersion, 1);
  assert.equal(report.options.iterations, 3);
  assert.deepEqual(report.options.sessionCounts, [100]);
  assert.doesNotMatch(serialized, /private\/tmp/);
});

test('buildReport includes a CI commit identifier when provided', () => {
  const report = buildReport({
    iterations: 1,
    sessionCounts: [],
    scenarios: {},
    environment: { GITHUB_SHA: 'abc123' }
  });

  assert.equal(report.commit, 'abc123');
});

test('coreScenarioDefinitions includes command startup and doctor scenarios', () => {
  assert.deepEqual(coreScenarioDefinitions('/tmp/coven'), [
    { id: 'help', command: '/tmp/coven', args: ['--help'], allowedExitCodes: [0] },
    { id: 'version', command: '/tmp/coven', args: ['--version'], allowedExitCodes: [0] },
    { id: 'doctor', command: '/tmp/coven', args: ['doctor'], allowedExitCodes: [0, 1] }
  ]);
});

test('collectCoreScenarios runs every core scenario with the requested iterations', () => {
  const calls = [];
  const scenarios = collectCoreScenarios({
    binary: '/tmp/coven',
    iterations: 2,
    env: { COVEN_HOME: '/fixture/home' },
    run: (definition) => {
      calls.push(definition);
      return { samplesMs: [1, 2], exitCodes: [0, 0], summary: { minMs: 1 } };
    }
  });

  assert.deepEqual(Object.keys(scenarios), ['help', 'version', 'doctor']);
  assert.equal(calls.length, 3);
  assert.deepEqual(calls.map((call) => call.iterations), [2, 2, 2]);
  assert.deepEqual(calls.map((call) => call.env), [
    { COVEN_HOME: '/fixture/home' },
    { COVEN_HOME: '/fixture/home' },
    { COVEN_HOME: '/fixture/home' }
  ]);
});

test('isolatedEnvironment replaces Coven and user-home paths', () => {
  const env = isolatedEnvironment('/fixture/coven-home', {
    PATH: '/fixture/bin',
    COVEN_HOME: '/real/coven-home',
    HOME: '/real/home',
    USERPROFILE: '/real/profile'
  });

  assert.deepEqual(env, {
    PATH: '/fixture/bin',
    COVEN_HOME: '/fixture/coven-home',
    HOME: '/fixture/coven-home/user-home',
    USERPROFILE: '/fixture/coven-home/user-home',
    XDG_CONFIG_HOME: '/fixture/coven-home/user-home/.config',
    XDG_CACHE_HOME: '/fixture/coven-home/user-home/.cache',
    XDG_STATE_HOME: '/fixture/coven-home/user-home/.local/state'
  });
});

test('daemonSocketPath reads the daemon socket from isolated metadata', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'coven-benchmark-status-'));
  await writeFile(
    join(directory, 'daemon.json'),
    JSON.stringify({ pid: 42, socket: '/fixture/coven.sock' })
  );
  t.after(() => rm(directory, { recursive: true, force: true }));

  assert.equal(await daemonSocketPath(directory), '/fixture/coven.sock');
});

test('benchmark CLI emits a machine-readable core-scenario report', () => {
  const result = spawnSync(
    process.execPath,
    [
      'scripts/benchmark-cli.mjs',
      `--binary=${process.execPath}`,
      '--iterations=1',
      '--session-counts=none'
    ],
    { encoding: 'utf8' }
  );

  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.schemaVersion, 1);
  assert.deepEqual(Object.keys(report.scenarios), ['help', 'version', 'doctor']);
});

test('benchmark CLI writes its report to the requested output path', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'coven-benchmark-output-'));
  const output = join(directory, 'report.json');
  t.after(() => rm(directory, { recursive: true, force: true }));

  const result = spawnSync(
    process.execPath,
    [
      'scripts/benchmark-cli.mjs',
      `--binary=${process.execPath}`,
      '--iterations=1',
      '--session-counts=none',
      `--output=${output}`
    ],
    { encoding: 'utf8' }
  );

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  const report = JSON.parse(await readFile(output, 'utf8'));
  assert.equal(report.schemaVersion, 1);
});

test('benchmark CLI isolates the child Coven environment', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'coven-benchmark-isolation-'));
  const binary = join(directory, 'fake-coven.mjs');
  await writeFile(
    binary,
    '#!/usr/bin/env node\nprocess.exit(process.env.COVEN_HOME === "/operator-state" ? 9 : 0);\n'
  );
  await chmod(binary, 0o755);
  t.after(() => rm(directory, { recursive: true, force: true }));

  const result = spawnSync(
    process.execPath,
    [
      'scripts/benchmark-cli.mjs',
      '--binary',
      binary,
      '--iterations',
      '1',
      '--session-counts=none'
    ],
    { encoding: 'utf8', env: { ...process.env, COVEN_HOME: '/operator-state' } }
  );

  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.deepEqual(report.scenarios.help.exitCodes, [0]);
});

test('README documents the non-gating benchmark invocation', async () => {
  const readme = await readFile('README.md', 'utf8');
  assert.match(readme, /benchmark-cli\.mjs/);
  assert.match(readme, /trend data/i);
});

test('runScenario preserves only timing and exit metadata', () => {
  const report = runScenario({
    command: process.execPath,
    args: ['--eval', 'process.exit(0)'],
    iterations: 2
  });

  assert.equal(report.samplesMs.length, 2);
  assert.deepEqual(Object.keys(report).sort(), ['exitCodes', 'samplesMs', 'summary']);
  assert.deepEqual(report.exitCodes, [0, 0]);
});

test('runScenario uses the supplied isolated environment', () => {
  const report = runScenario({
    command: process.execPath,
    args: ['--eval', "process.exit(process.env.COVEN_HOME === '/fixture/home' ? 0 : 7)"],
    iterations: 1,
    env: { ...process.env, COVEN_HOME: '/fixture/home' }
  });

  assert.deepEqual(report.exitCodes, [0]);
});

test('runCommand accepts an expected nonzero exit and preserves environment isolation', () => {
  const result = runCommand({
    command: process.execPath,
    args: ['--eval', "process.exit(process.env.COVEN_HOME === '/fixture/home' ? 1 : 7)"],
    allowedExitCodes: [1],
    env: { ...process.env, COVEN_HOME: '/fixture/home' }
  });

  assert.equal(result.status, 1);
});

test('runCommand includes captured stderr when a lifecycle command fails', () => {
  assert.throws(
    () => runCommand({
      command: process.execPath,
      args: ['--eval', "process.stderr.write('fixture command failed\\n'); process.exit(7)"],
      env: process.env
    }),
    /command exited with 7: fixture command failed/
  );
});

test('runCommand times out hung lifecycle commands', () => {
  assert.throws(
    () => runCommand({
      command: process.execPath,
      args: ['--eval', 'setTimeout(() => {}, 10_000)'],
      env: process.env,
      timeoutMs: 25
    }),
    /ETIMEDOUT/
  );
});

test('startDaemon starts, resolves metadata, then waits for health', async () => {
  const calls = [];
  const socket = await startDaemon({
    binary: '/tmp/coven',
    covenHome: '/fixture/coven-home',
    env: { COVEN_HOME: '/fixture/coven-home' },
    run: (command) => calls.push(['run', command]),
    readSocket: async (home) => {
      calls.push(['readSocket', home]);
      return '/fixture/coven.sock';
    },
    wait: async (path, options) => calls.push(['wait', path, options])
  });

  assert.equal(socket, '/fixture/coven.sock');
  assert.deepEqual(calls, [
    [
      'run',
      {
        command: '/tmp/coven',
        args: ['daemon', 'start'],
        allowedExitCodes: [0],
        env: { COVEN_HOME: '/fixture/coven-home' }
      }
    ],
    ['readSocket', '/fixture/coven-home'],
    ['wait', '/fixture/coven.sock', { attempts: 100, delayMs: 25 }]
  ]);
});

test('stopDaemon shuts down through the same isolated command boundary', () => {
  const calls = [];
  stopDaemon({
    binary: '/tmp/coven',
    env: { COVEN_HOME: '/fixture/coven-home' },
    run: (command) => calls.push(command)
  });

  assert.deepEqual(calls, [
    {
      command: '/tmp/coven',
      args: ['daemon', 'stop'],
      allowedExitCodes: [0],
      env: { COVEN_HOME: '/fixture/coven-home' }
    }
  ]);
});

test('registerExternalSessions seeds deterministic rows through the socket API', async () => {
  const calls = [];
  await registerExternalSessions({
    socketPath: '/fixture/coven.sock',
    count: 3,
    projectRoot: '/fixture/project',
    request: async (socketPath, request) => {
      calls.push([socketPath, request]);
      return { statusCode: 201, body: '{}' };
    }
  });

  assert.deepEqual(calls.map(([socketPath, request]) => [socketPath, request.path]), [
    ['/fixture/coven.sock', '/api/v1/sessions/external'],
    ['/fixture/coven.sock', '/api/v1/sessions/external'],
    ['/fixture/coven.sock', '/api/v1/sessions/external']
  ]);
  assert.deepEqual(
    calls.map(([, request]) => JSON.parse(request.body).id),
    ['benchmark-session-000001', 'benchmark-session-000002', 'benchmark-session-000003']
  );
});

test('registerExternalSessions limits concurrent fixture registration', async () => {
  let active = 0;
  let maximumActive = 0;
  let registered = 0;

  await registerExternalSessions({
    socketPath: '/fixture/coven.sock',
    count: 6,
    concurrency: 2,
    projectRoot: '/fixture/project',
    request: async () => {
      active += 1;
      maximumActive = Math.max(maximumActive, active);
      await new Promise((resolve) => setTimeout(resolve, 5));
      active -= 1;
      registered += 1;
      return { statusCode: 201, body: '{}' };
    }
  });

  assert.equal(registered, 6);
  assert.equal(maximumActive, 2);
});

test('runSocketScenario records only response timing and status metadata', async () => {
  const report = await runSocketScenario({
    socketPath: '/fixture/coven.sock',
    request: async () => ({ statusCode: 200, body: '[{"id":"fixture"}]' }),
    path: '/api/v1/sessions',
    iterations: 2
  });

  assert.equal(report.samplesMs.length, 2);
  assert.deepEqual(report.statusCodes, [200, 200]);
  assert.deepEqual(Object.keys(report).sort(), ['samplesMs', 'statusCodes', 'summary']);
});

test('measureSessionLists isolates every requested fixture size', async () => {
  const calls = [];
  const reports = await measureSessionLists({
    binary: '/tmp/coven',
    fixtureRoot: '/fixture/root',
    sessionCounts: [2, 3],
    fixtureConcurrency: 2,
    environment: { PATH: '/fixture/bin' },
    makeDirectory: async (path) => calls.push(['mkdir', path]),
    start: async ({ covenHome, env }) => {
      calls.push(['start', covenHome, env.COVEN_HOME]);
      return `${covenHome}/coven.sock`;
    },
    seed: async ({ count, socketPath, concurrency }) =>
      calls.push(['seed', count, socketPath, concurrency]),
    measure: async ({ socketPath, path }) => {
      calls.push(['measure', socketPath, path]);
      return {
        samplesMs: [1],
        statusCodes: [200],
        summary: { minMs: 1 },
        socketPath
      };
    },
    stop: ({ covenHome }) => calls.push(['stop', covenHome])
  });

  assert.deepEqual(Object.keys(reports), ['sessions_2', 'sessions_3']);
  assert.deepEqual(calls, [
    ['mkdir', '/fixture/root/s-2/user-home'],
    ['start', '/fixture/root/s-2', '/fixture/root/s-2'],
    ['seed', 2, '/fixture/root/s-2/coven.sock', 2],
    ['measure', '/fixture/root/s-2/coven.sock', '/api/v1/sessions?limit=100'],
    ['stop', '/fixture/root/s-2'],
    ['mkdir', '/fixture/root/s-3/user-home'],
    ['start', '/fixture/root/s-3', '/fixture/root/s-3'],
    ['seed', 3, '/fixture/root/s-3/coven.sock', 2],
    ['measure', '/fixture/root/s-3/coven.sock', '/api/v1/sessions?limit=100'],
    ['stop', '/fixture/root/s-3']
  ]);
});

test('measureCapabilityReads uses a short socket-safe fixture home', async () => {
  const calls = [];
  let releaseHotRead;
  let hotReadStarted;
  const hotReadStartedPromise = new Promise((resolve) => {
    hotReadStarted = resolve;
  });
  const reportPromise = measureCapabilityReads({
    binary: '/tmp/coven',
    fixtureRoot: '/fixture/root',
    environment: { PATH: '/fixture/bin' },
    iterations: 2,
    makeDirectory: async (path) => calls.push(['mkdir', path]),
    start: async ({ covenHome, env }) => {
      calls.push(['start', covenHome, env.COVEN_HOME]);
      return `${covenHome}/coven.sock`;
    },
    measure: async ({ socketPath, path, iterations }) => {
      calls.push(['measure', socketPath, path, iterations]);
      if (iterations === 2) {
        hotReadStarted();
        await new Promise((resolve) => {
          releaseHotRead = resolve;
        });
      }
      return { samplesMs: [1], statusCodes: [200], summary: { minMs: 1 } };
    },
    stop: ({ covenHome }) => calls.push(['stop', covenHome])
  });

  await hotReadStartedPromise;
  assert.equal(calls.some(([name]) => name === 'stop'), false);
  releaseHotRead();
  const report = await reportPromise;

  assert.deepEqual(report, { samplesMs: [1], statusCodes: [200], summary: { minMs: 1 } });
  assert.deepEqual(calls, [
    ['mkdir', '/fixture/root/k/user-home'],
    ['start', '/fixture/root/k', '/fixture/root/k'],
    ['measure', '/fixture/root/k/coven.sock', '/api/v1/capabilities/harnesses', 1],
    ['measure', '/fixture/root/k/coven.sock', '/api/v1/capabilities/harnesses', 2],
    ['stop', '/fixture/root/k']
  ]);
});

test('collectBenchmarkScenarios merges core and daemon fixture reports', async () => {
  const calls = [];
  const scenarios = await collectBenchmarkScenarios({
    options: { binary: '/tmp/coven', iterations: 2, sessionCounts: [2] },
    fixtureRoot: '/fixture/root',
    environment: { PATH: '/fixture/bin' },
    makeDirectory: async (path) => calls.push(['mkdir', path]),
    collectCore: ({ env }) => {
      calls.push(['core', env.COVEN_HOME]);
      return { help: { samplesMs: [1], exitCodes: [0], summary: { minMs: 1 } } };
    },
    measureHarness: async (input) => {
      calls.push(['harness', input.fixtureRoot, input.iterations]);
      return { samplesMs: [3], statusCodes: [201], summary: { minMs: 3 } };
    },
    measureEvents: async (input) => {
      calls.push(['events', input.fixtureRoot, input.eventCounts]);
      return { event_tail_2: { samplesMs: [4], statusCodes: [200], summary: { minMs: 4 } } };
    },
    measureLists: async (input) => {
      calls.push(['lists', input.fixtureRoot, input.sessionCounts]);
      return { sessions_2: { samplesMs: [2], statusCodes: [200], summary: { minMs: 2 } } };
    },
    measureCapabilities: async (input) => {
      calls.push(['capabilities', input.fixtureRoot, input.iterations]);
      return { samplesMs: [5], statusCodes: [200], summary: { minMs: 5 } };
    }
  });

  assert.deepEqual(Object.keys(scenarios), [
    'help',
    'harness_first_output',
    'event_tail_2',
    'sessions_2',
    'capabilities_hot'
  ]);
  assert.deepEqual(calls, [
    ['mkdir', '/fixture/root/c/user-home'],
    ['core', '/fixture/root/c'],
    ['harness', '/fixture/root', 2],
    ['events', '/fixture/root', [2]],
    ['lists', '/fixture/root', [2]],
    ['capabilities', '/fixture/root', 2]
  ]);
});

test('collectBenchmarkScenarios records warmed capability reads', async () => {
  const calls = [];
  const scenarios = await collectBenchmarkScenarios({
    options: { binary: '/tmp/coven', iterations: 2, sessionCounts: [2] },
    fixtureRoot: '/fixture/root',
    environment: { PATH: '/fixture/bin' },
    makeDirectory: async () => {},
    collectCore: () => ({ help: { samplesMs: [1], exitCodes: [0], summary: { minMs: 1 } } }),
    measureHarness: async () => ({ samplesMs: [3], statusCodes: [201], summary: { minMs: 3 } }),
    measureEvents: async () => ({ event_tail_2: { samplesMs: [4], statusCodes: [200], summary: { minMs: 4 } } }),
    measureLists: async () => ({ sessions_2: { samplesMs: [2], statusCodes: [200], summary: { minMs: 2 } } }),
    measureCapabilities: async (input) => {
      calls.push([input.fixtureRoot, input.iterations]);
      return { samplesMs: [5], statusCodes: [200], summary: { minMs: 5 } };
    }
  });

  assert.deepEqual(scenarios.capabilities_hot, {
    samplesMs: [5],
    statusCodes: [200],
    summary: { minMs: 5 }
  });
  assert.deepEqual(calls, [['/fixture/root', 2]]);
});

test('external session fixture request uses the versioned sessions endpoint', () => {
  const request = externalSessionRequest({
    id: 'fixture-session-1',
    projectRoot: '/fixture/project'
  });

  assert.equal(request.path, '/api/v1/sessions/external');
  assert.equal(request.method, 'POST');
  assert.deepEqual(JSON.parse(request.body), {
    id: 'fixture-session-1',
    projectRoot: '/fixture/project',
    harness: 'benchmark-fixture',
    title: 'Benchmark fixture session'
  });
});

test('harness session fixture request uses the public launch endpoint', () => {
  const request = harnessSessionRequest({ projectRoot: '/fixture/project' });

  assert.equal(request.path, '/api/v1/sessions');
  assert.equal(request.method, 'POST');
  assert.deepEqual(JSON.parse(request.body), {
    projectRoot: '/fixture/project',
    cwd: '/fixture/project',
    harness: 'codex',
    launchMode: 'nonInteractive',
    prompt: 'Benchmark fixture prompt',
    title: 'Benchmark harness fixture'
  });
});

test('session input fixture request records safe input through the public API', () => {
  const request = sessionInputRequest('session-1', 7);

  assert.equal(request.method, 'POST');
  assert.equal(request.path, '/api/v1/sessions/session-1/input');
  assert.deepEqual(JSON.parse(request.body), { data: 'Benchmark event 000007\n' });
});

test('registerInputEvents seeds deterministic session events through the input API', async () => {
  const calls = [];
  await registerInputEvents({
    socketPath: '/fixture/coven.sock',
    sessionId: 'session-1',
    count: 3,
    request: async (socketPath, request) => {
      calls.push([socketPath, request]);
      return { statusCode: 202, body: '{"accepted":true}' };
    }
  });

  assert.deepEqual(calls.map(([socketPath, request]) => [socketPath, request.path]), [
    ['/fixture/coven.sock', '/api/v1/sessions/session-1/input'],
    ['/fixture/coven.sock', '/api/v1/sessions/session-1/input'],
    ['/fixture/coven.sock', '/api/v1/sessions/session-1/input']
  ]);
  assert.deepEqual(
    calls.map(([, request]) => JSON.parse(request.body).data),
    ['Benchmark event 000001\n', 'Benchmark event 000002\n', 'Benchmark event 000003\n']
  );
});

test('prepareEventTail advances a large fixture to its final bounded page', async () => {
  const paths = [];
  const path = await prepareEventTail({
    socketPath: '/fixture/coven.sock',
    sessionId: 'session-1',
    count: 2_500,
    request: async (_socketPath, request) => {
      paths.push(request.path);
      if (paths.length === 1) {
        return {
          statusCode: 200,
          body: JSON.stringify({ events: Array(1000).fill({ kind: 'input' }), nextCursor: { afterSeq: 1000 } })
        };
      }
      return {
        statusCode: 200,
        body: JSON.stringify({ events: Array(500).fill({ kind: 'input' }), nextCursor: { afterSeq: 1500 } })
      };
    }
  });

  assert.deepEqual(paths, [
    '/api/v1/sessions/session-1/events?limit=1000',
    '/api/v1/sessions/session-1/events?afterSeq=1000&limit=500'
  ]);
  assert.equal(path, '/api/v1/sessions/session-1/events?afterSeq=1500&limit=1000');
});

test('launchHarnessSession returns the daemon-assigned session id', async () => {
  const id = await launchHarnessSession({
    socketPath: '/fixture/coven.sock',
    projectRoot: '/fixture/project',
    request: async (_socketPath, request) => {
      assert.equal(request.path, '/api/v1/sessions');
      return { statusCode: 201, body: JSON.stringify({ id: 'event-session-1' }) };
    }
  });

  assert.equal(id, 'event-session-1');
});

test('measureEventTails isolates each event scale and cleans up its live session', async () => {
  const calls = [];
  const reports = await measureEventTails({
    binary: '/tmp/coven',
    fixtureRoot: '/fixture/root',
    eventCounts: [2, 3],
    environment: { PATH: '/fixture/bin' },
    makeDirectory: async (path) => calls.push(['mkdir', path]),
    createFixture: async (_root, environment) => ({ ...environment, PATH: '/fixture/event-bin' }),
    start: async ({ covenHome, env }) => {
      calls.push(['start', covenHome, env.PATH]);
      return `${covenHome}/coven.sock`;
    },
    launch: async ({ socketPath }) => {
      calls.push(['launch', socketPath]);
      return `session-${calls.filter(([kind]) => kind === 'launch').length}`;
    },
    seed: async ({ sessionId, count }) => calls.push(['seed', sessionId, count]),
    prepare: async ({ sessionId, count }) => {
      calls.push(['prepare', sessionId, count]);
      return `/api/v1/sessions/${sessionId}/events?limit=${count}`;
    },
    measure: async ({ path }) => {
      calls.push(['measure', path]);
      return { samplesMs: [1], statusCodes: [200], summary: { minMs: 1 } };
    },
    finish: async ({ socketPath, sessionId }) => calls.push(['finish', socketPath, sessionId]),
    stop: ({ covenHome }) => calls.push(['stop', covenHome])
  });

  assert.deepEqual(Object.keys(reports), ['event_tail_2', 'event_tail_3']);
  assert.deepEqual(calls, [
    ['mkdir', '/fixture/root/e-2/user-home'],
    ['start', '/fixture/root/e-2', '/fixture/event-bin'],
    ['launch', '/fixture/root/e-2/coven.sock'],
    ['seed', 'session-1', 2],
    ['prepare', 'session-1', 2],
    ['measure', '/api/v1/sessions/session-1/events?limit=2'],
    ['finish', '/fixture/root/e-2/coven.sock', 'session-1'],
    ['stop', '/fixture/root/e-2'],
    ['mkdir', '/fixture/root/e-3/user-home'],
    ['start', '/fixture/root/e-3', '/fixture/event-bin'],
    ['launch', '/fixture/root/e-3/coven.sock'],
    ['seed', 'session-2', 3],
    ['prepare', 'session-2', 3],
    ['measure', '/api/v1/sessions/session-2/events?limit=3'],
    ['finish', '/fixture/root/e-3/coven.sock', 'session-2'],
    ['stop', '/fixture/root/e-3']
  ]);
});

test('stopLiveSession sends the public kill request', async () => {
  await stopLiveSession({
    socketPath: '/fixture/coven.sock',
    sessionId: 'session-1',
    request: async (_socketPath, request) => {
      assert.deepEqual(request, { method: 'POST', path: '/api/v1/sessions/session-1/kill' });
      return { statusCode: 202, body: '{"accepted":true}' };
    }
  });
});

test('waitForOutputEvent resolves only after an output event is recorded', async () => {
  const paths = [];
  let reads = 0;
  await waitForOutputEvent('/fixture/coven.sock', 'session-1', {
    attempts: 3,
    delayMs: 0,
    request: async (_socketPath, request) => {
      paths.push(request.path);
      reads += 1;
      return {
        statusCode: 200,
        body: JSON.stringify({
          events: reads === 2 ? [{ kind: 'output' }] : [{ kind: 'exit' }],
          nextCursor: { afterSeq: reads * 10 }
        })
      };
    }
  });

  assert.deepEqual(paths, [
    '/api/v1/sessions/session-1/events?limit=1',
    '/api/v1/sessions/session-1/events?afterSeq=10&limit=1'
  ]);
});

test('runHarnessOutputScenario measures launch through first output event', async () => {
  const calls = [];
  const report = await runHarnessOutputScenario({
    socketPath: '/fixture/coven.sock',
    projectRoot: '/fixture/project',
    iterations: 2,
    request: async (_socketPath, request) => {
      calls.push(request);
      return { statusCode: 201, body: JSON.stringify({ id: `session-${calls.length}` }) };
    },
    wait: async (socketPath, sessionId, options) => {
      calls.push({ socketPath, sessionId, options });
    }
  });

  assert.deepEqual(report.statusCodes, [201, 201]);
  assert.equal(report.samplesMs.length, 2);
  assert.equal(calls[0].path, '/api/v1/sessions');
  assert.equal(calls[1].socketPath, '/fixture/coven.sock');
  assert.equal(calls[1].sessionId, 'session-1');
  assert.equal(calls[1].options.attempts, 100);
  assert.equal(calls[1].options.delayMs, 25);
});

test('socketRequest sends an external-session fixture over a local socket', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'coven-benchmark-test-'));
  const socketPath = join(directory, 'daemon.sock');
  const server = createServer((request, response) => {
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk) => {
      body += chunk;
    });
    request.on('end', () => {
      assert.equal(request.method, 'POST');
      assert.equal(request.url, '/api/v1/sessions/external');
      assert.equal(JSON.parse(body).id, 'fixture-session-1');
      response.writeHead(201, { 'content-type': 'application/json' });
      response.end('{"ok":true}');
    });
  });

  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    await rm(directory, { recursive: true, force: true });
  });

  const response = await socketRequest(socketPath, externalSessionRequest({
    id: 'fixture-session-1',
    projectRoot: '/fixture/project'
  }));

  assert.deepEqual(response, { statusCode: 201, body: '{"ok":true}' });
});

test('socketRequest rejects when a local daemon request stalls', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'coven-benchmark-timeout-'));
  const socketPath = join(directory, 'daemon.sock');
  const server = createServer(() => {});

  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    await rm(directory, { recursive: true, force: true });
  });

  await assert.rejects(
    socketRequest(socketPath, { method: 'GET', path: '/api/v1/health', timeoutMs: 25 }),
    /socket request timed out after 25ms/
  );
});

test('waitForHealth resolves after the local daemon health response', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'coven-benchmark-health-'));
  const socketPath = join(directory, 'daemon.sock');
  const server = createServer((request, response) => {
    assert.equal(request.method, 'GET');
    assert.equal(request.url, '/api/v1/health');
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end('{"ok":true}');
  });

  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    await rm(directory, { recursive: true, force: true });
  });

  await waitForHealth(socketPath, { attempts: 1, delayMs: 0 });
});
