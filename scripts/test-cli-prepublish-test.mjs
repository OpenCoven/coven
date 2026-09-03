import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  buildLocalInstallArgs,
  DEFAULT_COMMAND_TIMEOUT_MS,
  synthesizeDryRunVersion
} from './test-cli-prepublish.mjs';

test('local tarball install is offline and disables registry extras', () => {
  assert.deepEqual(
    buildLocalInstallArgs('/tmp/native.tgz', '/tmp/wrapper.tgz'),
    [
      'install',
      '--offline',
      '--no-package-lock',
      '--omit=optional',
      '--no-audit',
      '--no-fund',
      '/tmp/native.tgz',
      '/tmp/wrapper.tgz'
    ]
  );
});

test('synthesizeDryRunVersion honors COVEN_NPM_DRY_RUN_VERSION without calling npm view', () => {
  let called = false;
  const version = synthesizeDryRunVersion('@opencoven/cli', {
    env: { COVEN_NPM_DRY_RUN_VERSION: 'v999.0.0' },
    spawnSyncImpl() {
      called = true;
      throw new Error('override should bypass npm view');
    }
  });

  assert.equal(version, '999.0.0');
  assert.equal(called, false);
});

test('synthesizeDryRunVersion bounds npm view and reports timeout override guidance', () => {
  let request;
  assert.throws(
    () =>
      synthesizeDryRunVersion('@opencoven/cli', {
        env: {},
        spawnSyncImpl(command, args, options) {
          request = { command, args, options };
          return {
            status: null,
            stdout: '',
            stderr: '',
            error: { code: 'ETIMEDOUT' }
          };
        }
      }),
    /Set COVEN_NPM_DRY_RUN_VERSION/
  );

  assert.deepEqual(request, {
    command: 'npm',
    args: ['view', '@opencoven/cli', 'version', '--silent'],
    options: {
      shell: process.platform === 'win32',
      stdio: ['ignore', 'pipe', 'pipe'],
      encoding: 'utf8',
      timeout: DEFAULT_COMMAND_TIMEOUT_MS
    }
  });
});

test('synthesizeDryRunVersion bumps the published patch version for dry-run packaging', () => {
  const version = synthesizeDryRunVersion('@opencoven/cli', {
    env: {},
    spawnSyncImpl() {
      return {
        status: 0,
        stdout: '1.2.3\n',
        stderr: ''
      };
    }
  });

  assert.equal(version, '1.2.4');
});

test('prepublish failures preserve finally cleanup', () => {
  const script = readFileSync(new URL('./test-cli-prepublish.mjs', import.meta.url), 'utf8');
  assert.match(script, /process\.exitCode = 1/);
  assert.doesNotMatch(script, /process\.exit\(1\)/);
});
