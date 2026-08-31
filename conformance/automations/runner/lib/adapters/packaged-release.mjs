// Packed-artifact execution helper for the packaged-release adapter.
// Spawns the artifact's binary synchronously and parses its stdout as JSON;
// any failure is reported as a target-evaluator failure, never as a skip.

import { spawnSync } from 'node:child_process';

export function runPackaged(bin, args, { input = null } = {}) {
  const result = spawnSync(bin, args, {
    input,
    encoding: 'utf8',
    timeout: 120_000
  });
  if (result.error) {
    return {
      status: 'failed',
      failures: [
        {
          vectorId: '',
          profile: 'full',
          invariant: 'target-evaluator',
          objectIds: [],
          eventCursor: null,
          expected: 'the packed artifact binary executes',
          observed: `failed to spawn ${bin}: ${result.error.message}`,
          reproduction: `${bin} ${args.join(' ')}`
        }
      ]
    };
  }
  if (result.status !== 0) {
    return {
      status: 'failed',
      failures: [
        {
          vectorId: '',
          profile: 'full',
          invariant: 'target-evaluator',
          objectIds: [],
          eventCursor: null,
          expected: `${bin} ${args.join(' ')} exits 0 with a JSON report`,
          observed: `exit code ${result.status}: ${(result.stderr ?? '').slice(0, 400)}`,
          reproduction: `${bin} ${args.join(' ')}`
        }
      ]
    };
  }
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    return {
      status: 'failed',
      failures: [
        {
          vectorId: '',
          profile: 'full',
          invariant: 'target-evaluator',
          objectIds: [],
          eventCursor: null,
          expected: 'a JSON report on stdout',
          observed: `stdout is not JSON: ${error.message}`,
          reproduction: `${bin} ${args.join(' ')}`
        }
      ]
    };
  }
}
