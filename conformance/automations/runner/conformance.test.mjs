// Tests for the Coven automations conformance plane (coven#858).
//
// Run with: node --test conformance/automations/runner/conformance.test.mjs
//
// These tests cover the runner's own machinery: schema validation, the
// schedule clock (including DST gap/fold and IANA zones), the reference
// oracle's core contracts, profile separation in reports, redaction, the SLO
// gate, and deterministic randomized invariant testing. The vectors
// themselves are the certification artifacts; the full suite runs through
// scripts/agent-check and CI.

import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  loadVectors,
  parseArgs,
  runConformance,
  validateReport,
  profileResult,
  PLANE_ROOT,
  PLANE_VERSION,
  PROFILES
} from './conformance.mjs';
import { validateAgainstSchema, assertValid } from './lib/schema.mjs';
import {
  latestDueSlot,
  latestDueSlotBrute,
  nextDue,
  parseIso,
  parseRrule,
  resolveWall,
  RruleError
} from './lib/clock.mjs';
import { canonicalJson, ConformanceModel } from './lib/model.mjs';
import { checkInvariants, evaluateVector, fuzzInvariants } from './lib/evaluate.mjs';
import { applyOperation } from './lib/ops.mjs';
import { redactPublishedText, redactText, scrubString } from './lib/redact.mjs';
import { evaluateSloGate } from './conformance.mjs';

test('every vector passes the envelope schema and loads', async () => {
  const { vectors } = await loadVectors();
  assert.ok(vectors.length >= 90, `expected a substantial vector set, got ${vectors.length}`);
  const ids = new Set(vectors.map((entry) => entry.vector.vectorId));
  assert.equal(ids.size, vectors.length, 'vectorIds must be unique');
});

test('every required profile has executable vectors', async () => {
  const { vectors } = await loadVectors();
  for (const profile of PROFILES) {
    const count = vectors.filter(
      (entry) => entry.vector.profile === profile || profile === 'full'
    ).length;
    assert.ok(count > 0, `profile ${profile} has no vectors`);
  }
});

test('the twenty-two golden scenarios exist', async () => {
  const { vectors } = await loadVectors();
  const golden = vectors.filter((entry) => entry.vector.vectorId.startsWith('golden.'));
  assert.equal(golden.length, 22, `expected 22 golden scenarios, got ${golden.length}`);
});

test('schema validator accepts valid documents and refuses invalid ones', () => {
  const schema = {
    type: 'object',
    additionalProperties: false,
    required: ['name'],
    properties: {
      name: { type: 'string', minLength: 1 },
      count: { type: 'integer', minimum: 0, maximum: 10 },
      mode: { enum: ['a', 'b'] },
      nested: { type: 'object', properties: { deep: { type: 'boolean' } } },
      union: { oneOf: [{ enum: ['x'] }, { type: 'string', pattern: '^y' }] },
      list: { type: 'array', items: { type: 'string' }, minItems: 1 }
    }
  };
  assertValid({ name: 'ok', count: 3, mode: 'a', union: 'x', list: ['s'] }, schema, 'valid');
  assert.throws(() => assertValid({ name: '' }, schema, 'empty-name'), /minLength/);
  assert.throws(() => assertValid({ name: 'ok', count: 1.5 }, schema, 'float'), /integer/);
  assert.throws(() => assertValid({ name: 'ok', count: 11 }, schema, 'big'), /maximum/);
  assert.throws(() => assertValid({ name: 'ok', mode: 'z' }, schema, 'enum'), /not one of/);
  assert.throws(() => assertValid({ name: 'ok', surprise: 1 }, schema, 'extra'), /additional property/);
  assert.throws(() => assertValid({ name: 'ok', union: 'z' }, schema, 'oneOf'), /oneOf/);
  assert.throws(() => assertValid({ name: 'ok', list: [] }, schema, 'minItems'), /fewer than/);
  assert.throws(() => assertValid({}, schema, 'required'), /required/);
});

test('rrule parser mirrors the scoped vocabulary', () => {
  assert.deepEqual(parseRrule('FREQ=DAILY'), { frequency: 'DAILY', byHour: [9], byDay: [] });
  assert.deepEqual(parseRrule('FREQ=DAILY;BYHOUR=17,9'), {
    frequency: 'DAILY',
    byHour: [9, 17],
    byDay: []
  });
  assert.deepEqual(parseRrule('FREQ=WEEKLY;BYDAY=FR,MO;BYHOUR=8'), {
    frequency: 'WEEKLY',
    byHour: [8],
    byDay: ['FR', 'MO']
  });
  assert.deepEqual(parseRrule('FREQ=WEEKLY;BYDAY=MON'), {
    frequency: 'WEEKLY',
    byHour: [9],
    byDay: ['MO']
  });
  assert.throws(() => parseRrule('FREQ=HOURLY'), RruleError);
  assert.throws(() => parseRrule('FREQ=DAILY;COUNT=3'), RruleError);
  assert.throws(() => parseRrule('FREQ=DAILY;BYHOUR=24'), RruleError);
  assert.throws(() => parseRrule('FREQ=DAILY;BYHOUR=9,9'), RruleError);
  assert.throws(() => parseRrule('BYHOUR=9'), RruleError);
  assert.throws(() => parseRrule('FREQ=DAILY;BYDAY=XX'), RruleError);
  // Negative hours are schema-invalid and must be refused, not silently
  // normalized into slots.
  assert.throws(() => parseRrule('FREQ=DAILY;BYHOUR=-9'), RruleError);
  assert.throws(() => parseRrule('FREQ=DAILY;BYHOUR=9,-3'), RruleError);
  assert.throws(() => parseRrule('FREQ=DAILY;BYHOUR=-0'), RruleError);
});

test('direct computation survives an outage longer than the old 4096-slot walk cap', () => {
  // 14 years of daily slots: the removed forward walk stopped after 4096
  // steps (~11 years) and silently reported a stale slot.
  const cursor = parseIso('2014-03-01T00:00:00.000Z');
  const now = parseIso('2028-02-29T10:00:00.000Z');
  assert.ok((now - cursor) / 86400e3 > 4096, 'the gap must exceed the old walk cap');
  assert.equal(
    latestDueSlot('FREQ=DAILY;BYHOUR=9', 'utc', cursor, now),
    parseIso('2028-02-29T09:00:00.000Z')
  );
  assert.equal(
    latestDueSlotBrute('FREQ=DAILY;BYHOUR=9', 'utc', cursor, now),
    parseIso('2028-02-29T09:00:00.000Z')
  );
});

test('direct computation agrees with the independent brute oracle everywhere', () => {
  const rrules = ['FREQ=DAILY', 'FREQ=DAILY;BYHOUR=0,23', 'FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=6,18'];
  const zones = ['UTC', 'America/New_York', 'Asia/Tokyo', 'Australia/Lord_Howe'];
  // Window boundaries straddle both 2026 DST transitions in New York plus
  // ordinary days, with now placed before, inside, and after slot hours.
  const windows = [
    ['2026-03-07T00:00:00.000Z', '2026-03-08T07:30:00.000Z'],
    ['2026-03-07T00:00:00.000Z', '2026-03-08T09:00:00.000Z'],
    ['2026-03-08T06:00:00.000Z', '2026-03-09T12:00:00.000Z'],
    ['2026-10-31T00:00:00.000Z', '2026-11-01T05:30:00.000Z'],
    ['2026-10-31T00:00:00.000Z', '2026-11-01T07:30:00.000Z'],
    ['2026-06-01T04:00:00.000Z', '2026-06-15T09:30:00.000Z']
  ];
  for (const rrule of rrules) {
    for (const zone of zones) {
      for (const [cursor, now] of windows) {
        const direct = latestDueSlot(rrule, zone, parseIso(cursor), parseIso(now));
        const brute = latestDueSlotBrute(rrule, zone, parseIso(cursor), parseIso(now));
        assert.equal(
          direct,
          brute,
          `${rrule} @ ${zone} (${cursor}..${now}): direct ${direct} vs brute ${brute}`
        );
      }
    }
  }
});

test('clock resolves DST gaps, folds, and IANA zones deterministically', () => {
  // Spring gap: 02:00 America/New_York on 2026-03-08 does not exist.
  const gapWall = parseIso('2026-03-08T02:00:00.000Z'); // pseudo-UTC of the wall time
  assert.equal(resolveWall('America/New_York', gapWall).status, 'gap');
  // Fall fold: 01:00 America/New_York on 2026-11-01 occurs twice.
  const foldWall = parseIso('2026-11-01T01:00:00.000Z');
  const fold = resolveWall('America/New_York', foldWall);
  assert.equal(fold.status, 'fold');
  assert.equal(fold.instant, parseIso('2026-11-01T05:00:00.000Z')); // earliest pass (EDT)
  assert.equal(fold.latestInstant, parseIso('2026-11-01T06:00:00.000Z')); // second pass (EST)
  // Tokyo 09:00 is 00:00Z.
  assert.equal(
    nextDue('FREQ=DAILY;BYHOUR=9', 'Asia/Tokyo', parseIso('2026-03-01T15:00:00.000Z')),
    parseIso('2026-03-02T00:00:00.000Z')
  );
  // 30-minute DST shift (Lord Howe) does not break resolution.
  const lordHowe = resolveWall('Australia/Lord_Howe', parseIso('2026-10-03T02:00:00.000Z'));
  assert.ok(['single', 'fold', 'gap'].includes(lordHowe.status));
});

test('planner walk reaches the latest due slot after a year-long gap', () => {
  const cursor = parseIso('2027-03-01T00:00:00.000Z');
  const now = parseIso('2028-02-29T10:00:00.000Z');
  assert.equal(
    latestDueSlot('FREQ=DAILY;BYHOUR=9', 'utc', cursor, now),
    parseIso('2028-02-29T09:00:00.000Z')
  );
});

test('redaction covers short, multiline, and quoted prompts', () => {
  const prompts = ['hi', 'Draft the reply\nfrom the private\njournal.', 'summarize the log'];
  const published = redactPublishedText(
    {
      a: { prompt: 'hi' },
      b: { prompt: 'Draft the reply\nfrom the private\njournal.' },
      c: 'the operator said: "summarize the log" and left',
      d: "echo 'hi' done"
    },
    prompts
  );
  assert.ok(!published.includes('"hi"'), `short prompt leaked: ${published}`);
  assert.ok(!published.includes('private'), `multiline prompt leaked: ${published}`);
  assert.ok(!published.includes('summarize the log'), `quoted prompt leaked: ${published}`);
  assert.ok(published.includes('[redacted]'), 'placeholders must appear');
});

test('redaction scrubs credentials of every covered form', () => {
  const published = scrubString(
    JSON.stringify({
      aws: 'AKIAIOSFODNN7EXAMPLE in a log line',
      github: 'ghp_16Charactersffffffffffffffffffffffffffff'.slice(0, 43),
      openai: 'sk-proj-abcdefghijklmnopqrstuvwxyz1234567890',
      bearer: 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig',
      password: 'db password=hunter2 persisted',
      pem: '-----BEGIN RSA PRIVATE KEY-----\nMIIB\n-----END RSA PRIVATE KEY-----',
      url: 'postgres://admin:s3cret@db.internal:5432/coven'
    })
  );
  assert.ok(!published.includes('AKIAIOSFODNN7EXAMPLE'), published);
  assert.ok(!published.includes('ghp_16Characters'), published);
  assert.ok(!published.includes('sk-proj-abcdefghij'), published);
  assert.ok(!published.includes('eyJhbGciOiJIUzI1NiJ9'), published);
  assert.ok(!published.includes('hunter2'), published);
  assert.ok(!published.includes('MIIB'), published);
  assert.ok(!published.includes('s3cret'), published);
});

test('redaction scrubs Windows paths and /private/ macOS paths', () => {
  const published = scrubString(
    JSON.stringify({
      win: 'wrote C:\\Users\\dev\\secrets\\notes.txt ok',
      winEnv: 'config at %USERPROFILE%\\coven\\config.toml',
      unc: 'mounted \\\\fileserver\\share\\prompt.txt',
      macPrivate: 'spill file /private/var/folders/zz/x/T/conformance.json',
      macTmp: '/private/tmp/leak.txt',
      posixHome: '/Users/alice/notes/daily.md',
      linuxHome: '/home/bob/.coven/history'
    })
  );
  assert.ok(!published.includes('C:\\Users\\dev'), published);
  assert.ok(!published.includes('%USERPROFILE%\\coven'), published);
  assert.ok(!published.includes('\\\\fileserver\\share'), published);
  assert.ok(!published.includes('/private/var/folders'), published);
  assert.ok(!published.includes('/private/tmp'), published);
  assert.ok(!published.includes('/Users/alice'), published);
  assert.ok(!published.includes('/home/bob'), published);
  assert.ok(published.split('[redacted-path]').length >= 7, `placeholders missing: ${published}`);
});

test('redaction replaces sensitive structured values before serialization', () => {
  const published = redactPublishedText(
    {
      prompt: 'never publish this',
      cwd: 'C:\\Users\\dev\\proj',
      outputTarget: '/Users/alice/out.md',
      token: 'ghp_16Charactersffffffffffffffffffffffffffff'.slice(0, 43),
      nested: { statement: 'also secret', keep: 'plain value' }
    },
    ['never publish this', 'also secret']
  );
  assert.ok(!published.includes('never publish this'), published);
  assert.ok(!published.includes('C:\\\\Users'), published);
  assert.ok(!published.includes('/Users/alice'), published);
  assert.ok(!published.includes('ghp_'), published);
  assert.ok(!published.includes('also secret'), published);
  assert.ok(published.includes('plain value'), 'non-sensitive values survive');
});

test('canonical JSON is stable and sorted', () => {
  assert.equal(canonicalJson({ b: 1, a: 2 }), '{"a":2,"b":1}');
  assert.equal(canonicalJson({ z: { b: [1, 2], a: null } }), '{"z":{"a":null,"b":[1,2]}}');
  assert.equal(canonicalJson('x'), '"x"');
});

test('model refuses settlement of planned occurrences and keeps runs monotonic', () => {
  const model = new ConformanceModel({ start: '2026-03-01T00:00:00.000Z' });
  model.occurrences.set('o1', {
    id: 'o1',
    automationId: 'a',
    scheduledFor: '2026-03-01T09:00:00.000Z',
    scheduledForMs: parseIso('2026-03-01T09:00:00.000Z'),
    state: 'planned',
    leaseOwner: null,
    leaseExpiresAtMs: null,
    attempt: 0,
    failureReason: null,
    nextEligibleAtMs: null,
    createdAtMs: model.now,
    updatedAtMs: model.now
  });
  assert.equal(model.settleOccurrence('o1', 'succeeded', null, model.now), false);
  model.occurrences.get('o1').state = 'claimed';
  assert.equal(model.settleOccurrence('o1', 'succeeded', null, model.now), true);
  assert.equal(model.settleOccurrence('o1', 'failed', 'late', model.now), false);
  assert.equal(model.finishRun('missing', { status: 'succeeded' }, model.now), false);
});

test('run terminal status is guarded and logs are bounded', () => {
  const model = new ConformanceModel({ start: '2026-03-01T00:00:00.000Z' });
  model.runs.set('r1', {
    id: 'r1',
    automationId: 'a',
    occurrenceId: null,
    sessionId: null,
    familiarId: null,
    runtime: 'coven-code',
    status: 'running',
    exitCode: null,
    logJson: null,
    outputCommit: null,
    ambiguous: false,
    behaviorAtLaunch: 'available',
    failureReason: null,
    startedAtMs: model.now,
    finishedAtMs: null,
    timeoutAtMs: null
  });
  assert.throws(
    () => model.finishRun('r1', { status: 'exploded' }, model.now),
    /succeeded, failed, or cancelled/
  );
  assert.equal(model.finishRun('r1', { status: 'failed', logJson: 'x'.repeat(70_000) }, model.now), true);
  assert.ok(model.runs.get('r1').logJson.length <= 64 * 1024 + 16);
  assert.equal(model.finishRun('r1', { status: 'succeeded' }, model.now), false);
});

test('receipt digests verify and tampering is detected', () => {
  const model = new ConformanceModel({ start: '2026-03-01T00:00:00.000Z' });
  model.runs.set('run-x', {
    id: 'run-x',
    automationId: 'a',
    occurrenceId: 'o1',
    sessionId: null,
    familiarId: null,
    runtime: 'coven-code',
    status: 'succeeded',
    exitCode: 0,
    logJson: null,
    outputCommit: 'committed',
    ambiguous: false,
    behaviorAtLaunch: 'available',
    failureReason: null,
    startedAtMs: model.now,
    finishedAtMs: model.now,
    timeoutAtMs: null
  });
  const receipt = model.sealReceipt(model.runs.get('run-x'), model.now);
  assert.equal(model.verifyReceipt(receipt), true);
  const tampered = { ...receipt, digest: 'f'.repeat(64) };
  assert.equal(model.verifyReceipt(tampered), false);
});

test('invariants catch a manufactured duplicate dispatch', () => {
  const model = new ConformanceModel({ start: '2026-03-01T00:00:00.000Z' });
  model.dispatchLog.push(
    { occurrenceId: 'o1', automationId: 'a', runId: 'r1', attempt: 1, at: 'x', behaviorAtLaunch: 'available' },
    { occurrenceId: 'o1', automationId: 'a', runId: 'r2', attempt: 1, at: 'x', behaviorAtLaunch: 'available' }
  );
  const violations = checkInvariants(model);
  assert.ok(violations.some((violation) => violation.invariant === 'no-duplicate-dispatch-per-fence'));
});

test('fuzzed operation sequences preserve every invariant', () => {
  for (const seed of [858, 42, 7, 1234]) {
    const result = fuzzInvariants({ operations: 250, seed });
    assert.deepEqual(
      result.violations,
      [],
      `seed ${seed} violated invariants: ${JSON.stringify(result.violations)}`
    );
    assert.equal(result.stopped, false);
  }
});

test('all vectors pass against the reference oracle', async () => {
  const { report } = await runConformance({
    profile: 'all',
    target: 'reference-oracle',
    report: null,
    vector: null,
    slo: null,
    fuzz: 0,
    seed: 858,
    list: false,
    quiet: true
  });
  assert.equal(report.gate.status, 'passed', JSON.stringify(report.failures, null, 1));
  assert.equal(report.failures.length, 0);
  for (const profile of ['structural', 'scheduler-reliability', 'runtime-authority', 'continuity', 'privacy', 'interoperability']) {
    assert.equal(
      report.profiles[profile].failed,
      0,
      `profile ${profile} has failures: ${JSON.stringify(report.failures.filter((f) => f.profile === profile))}`
    );
    assert.equal(
      report.profiles[profile].status,
      'passed',
      `profile ${profile} must be passed, got ${report.profiles[profile].status}`
    );
  }
  // Target-dependent canaries (cross-repo artifacts the reference oracle
  // does not provide) are reported separately, never as skips that pass.
  assert.ok(report.notApplicable.length >= 6);
  assert.ok(
    report.notApplicable.every(
      (entry) => entry.required === false && entry.reason.includes('prerequisites not met')
    ),
    'every not-applicable entry must be an explicit target-dependent gap'
  );
});

test('profile results fail closed on gaps and partial execution', () => {
  const vector = (vectorId, execution = 'required') => ({ vectorId, execution });
  const passed = { vector: vector('v-pass'), status: 'passed', failures: [] };
  const failed = { vector: vector('v-fail'), status: 'failed', failures: [{ invariant: 'x' }] };
  const gap = { vector: vector('v-gap'), status: 'not-applicable', failures: [], reason: 'prereq' };
  const optionalGap = {
    vector: vector('v-opt', 'target-dependent'),
    status: 'not-applicable',
    failures: [],
    reason: 'prereq'
  };

  assert.equal(profileResult([passed]).status, 'passed');
  assert.equal(profileResult([passed, failed]).status, 'failed');
  // A required vector that did not execute forces incomplete — never passed.
  assert.equal(profileResult([passed, gap]).status, 'incomplete');
  // Target-dependent gaps do not upgrade or downgrade an executed profile.
  assert.equal(profileResult([passed, optionalGap]).status, 'passed');
  // Nothing executed: not-applicable either way, never passed.
  assert.equal(profileResult([gap]).status, 'not-applicable');
  assert.equal(profileResult([optionalGap]).status, 'not-applicable');
  assert.equal(profileResult([]).status, 'not-applicable');
  const result = profileResult([passed, optionalGap]);
  assert.equal(result.notApplicable, 1);
  assert.equal(result.passed, 1);
});

test('a scoped run never reports the full profile passed', async () => {
  const { report } = await runConformance({
    profile: 'structural',
    target: 'reference-oracle',
    report: null,
    vector: null,
    slo: null,
    fuzz: 0,
    seed: 858,
    list: false,
    quiet: true
  });
  assert.equal(report.gate.status, 'passed', JSON.stringify(report.failures, null, 1));
  assert.equal(report.profiles.structural.status, 'passed');
  assert.equal(
    report.profiles.full.status,
    'not-applicable',
    'a structural-only run must not certify the full profile'
  );
  assert.equal(report.gate.fullProfileStatus, 'not-applicable');
});

test('a selected vector that cannot execute fails the gate', async () => {
  const { report } = await runConformance({
    profile: 'all',
    target: 'reference-oracle',
    report: null,
    vector: 'canary.packed-artifact-standalone',
    slo: null,
    fuzz: 0,
    seed: 858,
    list: false,
    quiet: true
  });
  assert.equal(report.gate.status, 'failed', 'selecting a target-dependent canary on the oracle must fail the gate');
  assert.equal(report.notApplicable.length, 1);
  assert.equal(report.notApplicable[0].vectorId, 'canary.packed-artifact-standalone');
  assert.ok(
    report.gate.notes.includes('did not execute'),
    `gate notes must name the non-executed vectors: ${report.gate.notes}`
  );
});

test('reports carry revisions, digests, and redaction before writing', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'conformance-report-'));
  const reportPath = join(dir, 'report.json');
  const { report, prompts } = await runConformance({
    profile: 'all',
    target: 'reference-oracle',
    report: reportPath,
    vector: null,
    slo: null,
    fuzz: 0,
    seed: 858,
    list: false,
    quiet: true
  });
  const written = JSON.parse(await readFile(reportPath, 'utf8'));
  assert.equal(written.plane, 'coven.automations.conformance');
  assert.equal(written.reportVersion, 1);
  assert.ok(written.target.revisions.sourceCommit, 'source revision must be pinned');
  assert.ok(Object.keys(written.artifactDigests).length > 10, 'artifact digests must be present');
  for (const digest of Object.values(written.artifactDigests)) {
    assert.match(digest, /^sha256-[0-9a-f]{64}$/);
  }
  // A vector prompt must never survive into the written report.
  for (const prompt of prompts) {
    if (prompt && prompt.length >= 4) {
      assert.ok(!JSON.stringify(written).includes(prompt), `prompt leaked: ${prompt}`);
    }
  }
  await rm(dir, { recursive: true, force: true });
});

test('SLO gate evaluates measures and stays provisional without baselines', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'conformance-slo-'));
  const measuredPath = join(dir, 'measured.json');
  await writeFile(
    measuredPath,
    JSON.stringify({ measures: [{ id: 'planning.latency.p95', value: 120 }] })
  );
  assert.equal((await evaluateSloGate(measuredPath)).status, 'passed');
  await writeFile(
    measuredPath,
    JSON.stringify({ measures: [{ id: 'planning.latency.p95', value: 999 }] })
  );
  assert.equal((await evaluateSloGate(measuredPath)).status, 'failed');
  await writeFile(measuredPath, JSON.stringify({ measures: [] }));
  assert.equal((await evaluateSloGate(measuredPath)).status, 'provisional');
  await rm(dir, { recursive: true, force: true });
});

test('the plane manifest pins versions, profiles, and hard gates', async () => {
  const manifest = JSON.parse(await readFile(join(PLANE_ROOT, 'manifest.json'), 'utf8'));
  assert.equal(manifest.version, PLANE_VERSION);
  assert.deepEqual(
    Object.keys(manifest.profiles).sort(),
    [...PROFILES].sort()
  );
  assert.ok(manifest.hardGates.length >= 4);
  assert.equal(manifest.targetsCapability, 'coven.automations.conformance.v1');
});

test('parseArgs validates arguments', () => {
  assert.equal(parseArgs(['--profile=structural']).profile, 'structural');
  assert.equal(parseArgs(['--fuzz', '10']).fuzz, 10);
  assert.throws(() => parseArgs(['--bogus']), /unknown argument/);
  assert.throws(() => parseArgs(['--profile=nope']), /unknown profile/);
});

test('the first-event crash boundary publishes exactly one event then dies', () => {
  const model = new ConformanceModel({ start: '2026-03-01T00:00:00.000Z' });
  model.insertDefinition({
    schemaVersion: 1,
    id: 'routine',
    name: 'Routine',
    status: 'ACTIVE',
    rrule: 'FREQ=DAILY;BYHOUR=9',
    timezone: 'utc',
    misfire: 'latest',
    overlap: 'forbid',
    timeoutMinutes: 30,
    runtime: 'coven-code',
    cwd: 'work/proj',
    prompt: 'Run the routine.'
  });
  applyOperation(model, { op: 'crash', during: 'first-event' });
  applyOperation(model, { op: 'tick', at: '2026-03-01T10:00:00.000Z' });
  assert.equal(model.dead, true, 'the pass dies at the first-event boundary');
  assert.equal(model.events.length, 1, 'exactly the first event is durably published');
  assert.ok(
    model.eventCursor > model.events.length,
    `the rest of the pass's events are lost with the crash (emitted ${model.eventCursor}, published ${model.events.length})`
  );
  // A restart reconciles: the occurrence state is intact, the changefeed
  // resumes from the published cursor, and nothing re-dispatches.
  applyOperation(model, { op: 'restart', at: '2026-03-01T10:05:00.000Z' });
  assert.equal(model.dead, false);
  assert.ok(
    [...model.occurrences.values()].every(
      (occurrence) => !['claimed', 'running'].includes(occurrence.state)
    ),
    'no occurrence is left mid-flight after the boundary crash'
  );
});

test('reports validate against conformance.report.v1 before publication', async () => {
  const { report } = await runConformance({
    profile: 'structural',
    target: 'reference-oracle',
    report: null,
    vector: null,
    slo: null,
    fuzz: 0,
    seed: 858,
    list: false,
    quiet: true
  });
  assert.deepEqual(validateReport(report), [], 'a real report must satisfy its schema');
  const broken = structuredClone(report);
  delete broken.gate.status;
  assert.ok(validateReport(broken).length > 0, 'a report missing gate.status must be refused');
  const drifted = structuredClone(report);
  drifted.plane = 'some.other.plane';
  assert.ok(
    validateReport(drifted).some((error) => error.includes('plane')),
    'a drifted plane id must be refused'
  );
});

test('structural vectors refuse invalid definitions with matching reasons', async () => {
  const { vectors, definitionSchema } = await loadVectors();
  const definitionVectors = vectors.filter(
    (entry) => entry.vector.category === 'definitions'
  );
  assert.ok(definitionVectors.length >= 10);
  for (const entry of definitionVectors) {
    const { failures } = evaluateVector(entry.vector, { definitionSchema });
    assert.deepEqual(
      failures,
      [],
      `vector ${entry.vector.vectorId}: ${JSON.stringify(failures)}`
    );
  }
});
