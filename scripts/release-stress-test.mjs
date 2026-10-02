import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildStressPlan,
  buildWarmPlan,
  runStressPlan,
  sanitizeOutput
} from './release-stress.mjs';

test('unix stress plan covers every release reliability surface ten times', () => {
  const plan = buildStressPlan({ suite: 'unix', iterations: 10 });

  assert.equal(plan.length, 50);
  assert.deepEqual(
    [...new Set(plan.map((entry) => entry.label))],
    [
      'claim acquisition',
      'memory migration',
      'process cleanup',
      'PTY timeout',
      'short socket homes'
    ]
  );
  assert.ok(plan.every((entry) => entry.timeoutMs === 180_000));
  assert.deepEqual(
    plan.filter((entry) => entry.iteration === 1).map((entry) => entry.args),
    [
      ['test', '-p', 'coven-cli', '--test', 'parallel_protocol', '--locked'],
      [
        'test',
        '-p',
        'coven-cli',
        '--bin',
        'coven',
        'cockpit_sources::tests::opened_memory_record_rechecks_logical_restore_state',
        '--locked',
        '--',
        '--exact'
      ],
      [
        'test',
        '-p',
        'coven-cli',
        '--test',
        'smoke',
        'daemon_stop_terminates_live_piped_session_descendants',
        '--locked',
        '--',
        '--exact'
      ],
      [
        'test',
        '-p',
        'coven-cli',
        '--bin',
        'coven',
        'pty_runner::tests::codex_json_runner_times_out_while_a_large_prompt_is_still_writing',
        '--locked',
        '--',
        '--exact'
      ],
      ['test', '-p', 'opencoven-coven-client', '--test', 'health', '--locked']
    ]
  );
});

test('windows stress plan repeats descendant-killing PTY timeout ten times', () => {
  const plan = buildStressPlan({ suite: 'windows', iterations: 10 });

  assert.equal(plan.length, 10);
  assert.ok(plan.every((entry) => entry.label === 'Windows PTY timeout and process cleanup'));
  assert.deepEqual(plan[0].args, [
    'test',
    '-p',
    'coven-cli',
    '--bin',
    'coven',
    'pty_runner::tests::windows_detached_pty_timeout_fails_and_kills_descendant',
    '--locked',
    '--',
    '--exact'
  ]);
});

test('warm plan compiles each distinct selection once without running it', () => {
  const windows = buildWarmPlan(buildStressPlan({ suite: 'windows', iterations: 10 }));
  assert.deepEqual(windows, [
    {
      label: 'Windows PTY timeout and process cleanup',
      args: [
        'test',
        '-p',
        'coven-cli',
        '--bin',
        'coven',
        'pty_runner::tests::windows_detached_pty_timeout_fails_and_kills_descendant',
        '--locked',
        '--no-run'
      ],
      iteration: 0,
      timeoutMs: 1_800_000
    }
  ]);

  const unixPlan = buildStressPlan({ suite: 'unix', iterations: 10 });
  const unix = buildWarmPlan(unixPlan, { buildTimeoutMs: 600_000 });
  assert.deepEqual(
    unix.map((entry) => entry.label),
    [...new Set(unixPlan.map((entry) => entry.label))]
  );
  for (const entry of unix) {
    assert.equal(entry.args.at(-1), '--no-run');
    assert.ok(!entry.args.includes('--') && !entry.args.includes('--exact'));
    assert.equal(entry.iteration, 0);
    assert.equal(entry.timeoutMs, 600_000);
  }
});

test('a warm-up that times out is reported as a build, not an iteration', () => {
  const writes = [];
  const plan = buildWarmPlan(buildStressPlan({ suite: 'windows', iterations: 1 }));

  assert.throws(
    () =>
      runStressPlan({
        plan,
        repoRoot: '/private/work/coven',
        runCommand() {
          return { error: { code: 'ETIMEDOUT' }, stdout: '', stderr: '' };
        },
        writeLog(text) {
          writes.push(text);
        }
      }),
    /Windows PTY timeout and process cleanup warm-up timed out after 1800000ms/
  );
  assert.match(writes.join(''), /^warm surface=Windows PTY timeout and process cleanup\n/);
});

test('stress runner stops at the first failed command and records its iteration', () => {
  const calls = [];
  const writes = [];
  const plan = buildStressPlan({ suite: 'windows', iterations: 3 });

  assert.throws(
    () =>
      runStressPlan({
        plan,
        repoRoot: '/private/work/coven',
        runCommand(entry) {
          calls.push(entry.iteration);
          return entry.iteration === 2
            ? { status: 17, stdout: '', stderr: '/private/work/coven failed' }
            : { status: 0, stdout: 'ok', stderr: '' };
        },
        writeLog(text) {
          writes.push(text);
        }
      }),
    /iteration 2.*exit 17/
  );
  assert.deepEqual(calls, [1, 2]);
  assert.match(writes.join(''), /<repo> failed/);
  assert.doesNotMatch(writes.join(''), /\/private\/work\/coven/);
});

test('stress output redacts repository paths', () => {
  assert.equal(
    sanitizeOutput(
      'failed in /private/work/coven and C:\\work\\coven',
      ['/private/work/coven', 'C:\\work\\coven']
    ),
    'failed in <repo> and <repo>'
  );
});
