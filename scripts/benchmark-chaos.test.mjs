import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import {
  buildReport,
  chaosCoverage,
  parseOptions,
  residentSetBytesFromProcessList,
  summarizeRuntimeMetrics,
  storageMetricStatus,
  summarize,
  validateReport
} from './benchmark-chaos.mjs';

test('parses the required output and full concurrency matrix', () => {
  assert.deepEqual(
    parseOptions(['--binary=/tmp/coven', '--output=/tmp/report.json']),
    { binary: '/tmp/coven', output: '/tmp/report.json', concurrency: [1, 8, 32] }
  );
});

test('rejects an incomplete full baseline matrix', () => {
  assert.throws(
    () => validateReport(buildReport({
      concurrency: [1, 8],
      scenarios: { sessions_1: { status: 'passed' }, sessions_8: { status: 'passed' } }
    })),
    /required concurrency 32/
  );
});

test('rejects an absent or invalid concurrency value', () => {
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--output=/tmp/report.json', '--concurrency']),
    /--concurrency must contain positive integers/
  );
  assert.throws(
    () => parseOptions(['--binary=/tmp/coven', '--output=/tmp/report.json', '--concurrency=1,zero']),
    /--concurrency must contain positive integers/
  );
});

test('summarizes p50, p95, and p99 using nearest rank', () => {
  assert.deepEqual(summarize([9, 1, 5, 3, 7]), {
    count: 5,
    minMs: 1,
    p50Ms: 5,
    p95Ms: 9,
    p99Ms: 9,
    maxMs: 9
  });
});

test('describes live writer metrics and deterministic fault equivalents', () => {
  assert.equal(storageMetricStatus().sqliteConnectionOpens.status, 'measured');
  assert.equal(storageMetricStatus().eventQueueDepth.status, 'measured');
});

test('marks diskFull chaos coverage as unproven until a write-fault seam is tested', () => {
  // scheduled_maintenance_below_watermark_does_not_open_or_write_the_store only
  // proves low-disk gating: it asserts the store is never opened when free disk
  // is below the maintenance watermark.  It never triggers SQLITE_FULL, a write
  // fault, or recovery, so it cannot back a "covered" claim for diskFull chaos.
  const diskFull = chaosCoverage().diskFull;
  assert.equal(diskFull.status, 'blocked');
  assert.equal(
    Object.hasOwn(diskFull, 'evidence'),
    false,
    'a blocked status must not cite a test as proving the fault path'
  );
  assert.doesNotMatch(
    JSON.stringify(diskFull),
    /scheduled_maintenance_below_watermark_does_not_open_or_write_the_store/
  );
});

test('summarizes writer counter deltas, sampled backlog maxima, and sampled RSS', () => {
  assert.deepEqual(
    summarizeRuntimeMetrics([
      {
        eventWriter: {
          connectionOpens: 1,
          transactions: 2,
          queuedEvents: 0,
          queuedBytes: 0
        },
        residentSetBytes: 10 * 1024 * 1024
      },
      {
        eventWriter: {
          connectionOpens: 1,
          transactions: 9,
          queuedEvents: 5,
          queuedBytes: 4096
        },
        residentSetBytes: 14 * 1024 * 1024
      },
      {
        eventWriter: {
          connectionOpens: 1,
          transactions: 12,
          queuedEvents: 0,
          queuedBytes: 0
        },
        residentSetBytes: 12 * 1024 * 1024
      }
    ]),
    {
      sqliteConnectionOpens: { start: 1, end: 1, delta: 0 },
      sqliteTransactions: { start: 2, end: 12, delta: 10 },
      eventQueueDepth: { maxSampledEvents: 5, maxSampledBytes: 4096 },
      rss: {
        samplesBytes: [10 * 1024 * 1024, 14 * 1024 * 1024, 12 * 1024 * 1024],
        peakBytes: 14 * 1024 * 1024
      }
    }
  );
});

test('selects daemon RSS by exact PID without retaining process details', () => {
  assert.equal(
    residentSetBytesFromProcessList(
      JSON.stringify({
        processes: [
          { pid: 41, name: 'other', memory_mb: 99, argv: ['private'] },
          { pid: 42, name: 'coven', memory_mb: 17, argv: ['/private/coven'] }
        ]
      }),
      42
    ),
    17 * 1024 * 1024
  );
  assert.throws(
    () => residentSetBytesFromProcessList('{"processes":[]}', 42),
    /daemon pid 42 is absent/
  );
});

test('report contains no fixture paths or prompt content', () => {
  const report = buildReport({
    concurrency: [1, 8, 32],
    scenarios: {
      sessions_1: { status: 'passed' },
      sessions_8: { status: 'passed' },
      sessions_32: { status: 'passed' }
    },
    environment: { GITHUB_ACTIONS: 'true', HOME: '/private/fixture', PROMPT: 'secret prompt' }
  });
  const serialized = JSON.stringify(report);
  assert.equal(report.schemaVersion, 3);
  assert.doesNotMatch(serialized, /private\/fixture|secret prompt/);
  assert.equal(Object.hasOwn(report.environment, 'host'), false);
  assert.equal(Object.hasOwn(report.environment, 'runner'), false);
  assert.equal(validateReport(report), report);
});

test('fixture harness records child execution before emitting output', async () => {
  const module = await import('./benchmark-chaos.mjs');
  assert.equal(typeof module.fixtureHarnessScript, 'function');
  const script = module.fixtureHarnessScript();
  const markerIndex = script.indexOf('COVEN_BENCHMARK_MARKERS');
  const readyIndex = script.indexOf('COVEN_BENCHMARK_READY');
  assert.notEqual(markerIndex, -1);
  assert.notEqual(readyIndex, -1);
  assert.ok(markerIndex < readyIndex);
  assert.match(script, /printf "started\\n"/);
});

test('event writer diagnostics expose bounded operational fields and redact fixture paths', async () => {
  const module = await import('./benchmark-chaos.mjs');
  assert.equal(typeof module.formatEventWriterHealth, 'function');
  assert.equal(
    module.formatEventWriterHealth({
      eventWriter: {
        state: 'failed',
        queuedEvents: 4,
        queuedBytes: 8192,
        capacityBytes: 16384,
        droppedOutputEvents: 2,
        droppedOutputBytes: 512,
        connectionOpens: 1,
        transactions: 7,
        committedEvents: 21,
        lastError: 'write failed at /fixture/root/coven.sqlite3\nretry stopped'
      }
    }, ['/fixture/root']),
    'eventWriter={state=failed queuedEvents=4 queuedBytes=8192 capacityBytes=16384 droppedOutputEvents=2 droppedOutputBytes=512 connectionOpens=1 transactions=7 committedEvents=21 lastError="write failed at <fixture>/coven.sqlite3 retry stopped"}'
  );
});

test('timeout diagnostics combine session, child execution, and writer evidence', async () => {
  const module = await import('./benchmark-chaos.mjs');
  assert.equal(typeof module.describeScenarioFailure, 'function');
  const responses = new Map([
    ['/api/v1/sessions/session-1', { statusCode: 200, body: '{"status":"running"}' }],
    ['/api/v1/sessions/session-1/events?limit=20', {
      statusCode: 200,
      body: '{"events":[{"kind":"started"}]}'
    }],
    ['/api/v1/health', {
      statusCode: 200,
      body: '{"eventWriter":{"state":"healthy","queuedEvents":0,"queuedBytes":0,"capacityBytes":2097152,"droppedOutputEvents":0,"droppedOutputBytes":0,"connectionOpens":1,"transactions":8,"committedEvents":31,"lastError":null}}'
    }]
  ]);

  const diagnostic = await module.describeScenarioFailure({
    socketPath: '/fixture/socket',
    sessionId: 'session-1',
    markerPath: '/fixture/markers',
    expectedExecutions: 32,
    request: async (_socketPath, request) => responses.get(request.path),
    read: async () => 'started\nstarted\n'
  });

  assert.equal(
    diagnostic,
    'status=running events=[started] fixtureExecutions=2/32 eventWriter={state=healthy queuedEvents=0 queuedBytes=0 capacityBytes=2097152 droppedOutputEvents=0 droppedOutputBytes=0 connectionOpens=1 transactions=8 committedEvents=31 lastError=null}'
  );
});

test('diagnostics collapse ASCII controls and tolerate non-Error throws', async () => {
  const module = await import('./benchmark-chaos.mjs');
  assert.equal(
    module.formatEventWriterHealth({
      eventWriter: {
        state: 'failed',
        lastError: 'locked\u0000then\u001Bretried'
      }
    }),
    'eventWriter={state=failed queuedEvents=unavailable queuedBytes=unavailable capacityBytes=unavailable droppedOutputEvents=unavailable droppedOutputBytes=unavailable connectionOpens=unavailable transactions=unavailable committedEvents=unavailable lastError="locked then retried"}'
  );

  const diagnostic = await module.describeScenarioFailure({
    socketPath: '/fixture/socket',
    sessionId: 'session-1',
    markerPath: '/fixture/markers',
    expectedExecutions: 1,
    request: async () => {
      throw null;
    },
    read: async () => {
      const error = new Error('missing');
      error.code = 'ENOENT';
      throw error;
    }
  });

  assert.equal(
    diagnostic,
    'sessionState=unavailable(null) fixtureExecutions=0/1 eventWriter=unavailable(null)'
  );
});

test('scenario timeout messages redact fixture paths from the original error', async () => {
  const module = await import('./benchmark-chaos.mjs');
  assert.equal(typeof module.formatScenarioTimeout, 'function');
  assert.equal(
    module.formatScenarioTimeout({
      concurrency: 32,
      error: new Error('connect ENOENT /fixture/private/daemon.sock'),
      diagnostic: 'status=running events=[none]',
      fixtureRoot: '/fixture/private'
    }),
    'sessions_32: connect ENOENT <fixture>/daemon.sock — status=running events=[none]'
  );
});

test('closes the throughput timing window the instant launches resolve', async () => {
  // The throughput denominator must be captured the moment every launch
  // resolves, before any observer teardown or diagnostic sampling adds latency.
  const source = await readFile(new URL('./benchmark-chaos.mjs', import.meta.url), 'utf8');
  assert.match(
    source,
    /completedAt = process\.hrtime\.bigint\(\);\n\s+elapsedMs = Number\(completedAt - startedAt\) \/ 1_000_000;/
  );
});

test('samples writer health across the full measured interval through cancellation', async () => {
  // The periodic 25ms observer must keep running until cancellation completes,
  // so sampled queue/RSS maxima cover launch AND teardown, not just launch.
  // Stopping it before the cancellation loop (the previous behaviour) left the
  // cancellation interval sampled only by the two manual bracket snapshots.
  const source = await readFile(new URL('./benchmark-chaos.mjs', import.meta.url), 'utf8');
  const observerTeardown = source.indexOf('observing = false;');
  const cancellationStart = source.indexOf('const cancellationStartedAt = process.hrtime.bigint();');
  const cancellationMeasured = source.indexOf(
    'cancellationMs = Number(process.hrtime.bigint() - cancellationStartedAt) / 1_000_000;'
  );
  assert.ok(observerTeardown !== -1, 'observer teardown must exist');
  assert.ok(cancellationStart !== -1, 'cancellation timer must exist');
  assert.ok(cancellationMeasured !== -1, 'cancellation interval must be measured');
  // The observer must still be running when cancellation begins and completes:
  // its teardown has to come after the cancellation interval is measured.
  assert.ok(
    observerTeardown > cancellationStart,
    'observer must not stop before the cancellation loop starts'
  );
  assert.ok(
    observerTeardown > cancellationMeasured,
    'observer must stop only after the cancellation interval is measured'
  );
});
