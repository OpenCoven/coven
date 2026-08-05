import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildReport,
  chaosCoverage,
  parseOptions,
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

test('records unavailable writer counters without pretending they are measured', () => {
  assert.equal(storageMetricStatus().sqliteConnectionOpens.status, 'unavailable');
  assert.equal(storageMetricStatus().eventQueueDepth.status, 'not_applicable');
  assert.equal(chaosCoverage().diskFull.status, 'blocked_by_injection');
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
