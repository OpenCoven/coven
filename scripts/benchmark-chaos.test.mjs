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
