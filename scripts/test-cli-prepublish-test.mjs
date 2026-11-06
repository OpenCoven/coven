import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  buildLocalInstallArgs,
  DEFAULT_COMMAND_TIMEOUT_MS,
  synthesizeDryRunVersion,
  TEMP_PROJECT_CLEANUP_OPTIONS,
  verifyNativeBinary
} from './test-cli-prepublish.mjs';

test('the installed native executable must carry the full commit and match the build bytes', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'coven-package-identity-'));
  try {
    const commit = '0123456789abcdef0123456789abcdef01234567';
    const binary = path.join(root, 'coven');
    const built = Buffer.from(`build\0${commit}`);
    writeFileSync(binary, built);
    const sha256 = verifyNativeBinary(binary, commit);
    assert.equal(verifyNativeBinary(binary, commit, sha256), sha256);

    writeFileSync(binary, Buffer.concat([built, Buffer.from('substitution')]));
    assert.throws(() => verifyNativeBinary(binary, commit, sha256), /differs from the build/);
    for (const invalid of [commit.slice(0, 8), 'f'.repeat(40), commit.match(/.{8}/g).join('!')]) {
      writeFileSync(binary, invalid);
      assert.throws(() => verifyNativeBinary(binary, commit, sha256), /does not embed/);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

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

test('temp project cleanup retries transient Windows locks', () => {
  assert.deepEqual(TEMP_PROJECT_CLEANUP_OPTIONS, {
    recursive: true,
    force: true,
    maxRetries: 10,
    retryDelay: 100
  });
  const script = readFileSync(new URL('./test-cli-prepublish.mjs', import.meta.url), 'utf8');
  assert.match(script, /rmSync\(tempDir, TEMP_PROJECT_CLEANUP_OPTIONS\)/);

  const root = mkdtempSync(path.join(os.tmpdir(), 'coven-prepublish-cleanup-'));
  try {
    const project = path.join(root, 'project');
    mkdirSync(path.join(project, 'node_modules', '.bin'), { recursive: true });
    writeFileSync(path.join(project, 'node_modules', '.bin', 'coven'), '');
    rmSync(project, TEMP_PROJECT_CLEANUP_OPTIONS);
    assert.equal(existsSync(project), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('prepublish failures preserve finally cleanup', () => {
  const script = readFileSync(new URL('./test-cli-prepublish.mjs', import.meta.url), 'utf8');
  assert.match(script, /process\.exitCode = 1/);
  assert.doesNotMatch(script, /process\.exit\(1\)/);
});
