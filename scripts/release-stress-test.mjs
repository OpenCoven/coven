import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildStressPlan,
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
      ['test', '-p', 'coven-client', '--test', 'health', '--locked']
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
