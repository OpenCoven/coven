#!/usr/bin/env node
// Pre-publish smoke test for the npm-distributed Coven CLI.
//
// What it does:
//   1. Verifies prerequisites (node, npm, cargo).
//   2. Runs the secrets scan, onboarding, PR readiness, and publish guardrails.
//   3. Stages the dist tree by running publish-npm.mjs in --dry-run mode
//      (which also runs `cargo build --release --package coven-cli --target <rust-target>` unless
//      --skip-build is passed) and lets `npm publish --dry-run` validate the
//      platform + wrapper tarballs.
//   4. `npm pack`s the native and wrapper packages, installs them into a fresh
//      temp project, then runs the full hermetic installed-wrapper user
//      journey through scripts/user-journey-e2e.mjs.
//
// Flags:
//   --target=<name>       Override the npm target (macos, macos-x64, linux-x64, windows).
//                         Defaults to the local platform.
//   --dashboard-tarball=<path>
//                         Install a locally packed dashboard companion and
//                         verify `coven memory open --help` through the wrapper.
//   --skip-build          Reuse an existing release binary at
//                         target/<rust-target>/release/coven instead of
//                         re-running `cargo build --release --package coven-cli --target ...`.
//   --with-cargo-gates    Also run `cargo fmt --check`, `cargo clippy`, and
//                         `cargo test --workspace --locked` (the CI verify
//                         gates). Off by default to keep local runs fast.
//   --skip-secrets-scan   Skip the secret-guard unit tests and full scan for
//                         local iteration; CI still runs both.
//   --keep-tempdir        Leave the temp install dir on disk for inspection.
//   COVEN_NPM_DRY_RUN_VERSION=vX.Y.Z
//                         Override the synthesized dry-run version to skip
//                         npm view during hermetic verification, or when the
//                         public npm registry cannot be reached.
//
// Exit code is non-zero on the first failing step.

import { spawnSync } from 'node:child_process';
import { existsSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { defaultTargetName } from './publish-npm.mjs';
import {
  createScratchDir,
  isMainModule,
  runPackagedUserJourney
} from './user-journey-e2e.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const distRoot = path.join(repoRoot, 'npm', 'dist');
export const DEFAULT_COMMAND_TIMEOUT_MS = 120_000;
const CARGO_GATE_TIMEOUT_MS = 20 * 60_000;

const PLATFORM_TARGETS = {
  macos: { packageName: '@opencoven/cli-macos', binaryName: 'coven' },
  'macos-x64': { packageName: '@opencoven/cli-macos-x64', binaryName: 'coven' },
  'linux-x64': { packageName: '@opencoven/cli-linux-x64', binaryName: 'coven' },
  windows: { packageName: '@opencoven/cli-windows', binaryName: 'coven.exe' }
};

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const opt = (name) => {
  const prefix = `${name}=`;
  const hit = args.find((arg) => arg.startsWith(prefix));
  return hit ? hit.slice(prefix.length) : undefined;
};

const targetName = opt('--target') ?? defaultTargetName(process.platform, process.arch);
const skipBuild = flag('--skip-build');
const withCargoGates = flag('--with-cargo-gates');
const skipSecretsScan = flag('--skip-secrets-scan');
const keepTempdir = flag('--keep-tempdir');
const dashboardTarballOption = opt('--dashboard-tarball');
const dashboardTarball =
  dashboardTarballOption === undefined
    ? undefined
    : path.resolve(dashboardTarballOption);

if (
  dashboardTarballOption !== undefined &&
  (!dashboardTarballOption ||
    !existsSync(dashboardTarball) ||
    !statSync(dashboardTarball).isFile())
) {
  fail(`dashboard tarball not found or is not a file: ${dashboardTarballOption || '(empty)'}`);
}

const target = PLATFORM_TARGETS[targetName];
if (!target) {
  fail(`Unsupported npm target ${targetName}. Known targets: ${Object.keys(PLATFORM_TARGETS).join(', ')}`);
}

const steps = [];
const stepNames = [];
function step(name, fn) {
  stepNames.push(name);
  steps.push(async () => {
    const start = Date.now();
    console.log(`\n=== ${name} ===`);
    await fn();
    const seconds = ((Date.now() - start) / 1000).toFixed(1);
    console.log(`--- ${name} ok (${seconds}s)`);
  });
}

step('prerequisites', () => {
  ensureCommand('node', ['--version']);
  ensureCommand('npm', ['--version']);
  ensureCommand('cargo', ['--version']);
});

if (!skipSecretsScan) {
  step('secrets scan', () => {
    run('python3', ['scripts/check-secrets-test.py']);
    run('python3', ['scripts/check-secrets.py']);
  });
}

step('onboarding, PR readiness, and publish guardrails', () => {
  run('node', [
    '--test',
    'scripts/onboarding-docs-test.mjs',
    'scripts/cli-docs-test.mjs',
    'scripts/export-cli-help-contract-test.mjs',
    'scripts/pr-readiness-test.mjs',
    'scripts/publish-npm-test.mjs',
    'scripts/test-cli-prepublish-test.mjs',
    'scripts/user-journey-e2e-test.mjs'
  ]);
});

if (withCargoGates) {
  step('cargo fmt --check', () => run('cargo', ['fmt', '--check']));
  step('cargo clippy', () =>
    run('cargo', ['clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'], {
      timeoutMs: CARGO_GATE_TIMEOUT_MS
    })
  );
  step('cargo test --workspace --locked', () =>
    run('cargo', ['test', '--workspace', '--locked'], {
      timeoutMs: CARGO_GATE_TIMEOUT_MS
    })
  );
}

let dryRunVersion;
step(`stage dist via publish-npm.mjs --dry-run --target=${targetName}`, () => {
  // `npm publish --dry-run` refuses to publish under the "latest" tag with a
  // version lower than what's already on the registry, so we synthesize a
  // high prerelease version derived from the current latest. This is only
  // used for the dry-run; real releases pull their version from the git tag.
  dryRunVersion = synthesizeDryRunVersion('@opencoven/cli');
  console.log(`using dry-run version ${dryRunVersion}`);

  const publishArgs = ['scripts/publish-npm.mjs', `--target=${targetName}`, '--dry-run'];
  if (skipBuild) {
    publishArgs.push('--skip-build');
  }
  run('node', publishArgs, {
    env: { ...process.env, COVEN_NPM_VERSION: dryRunVersion }
  });
  const platformDir = path.join(distRoot, targetName);
  const wrapperDir = path.join(distRoot, 'coven');
  if (!existsSync(platformDir)) {
    fail(`expected platform dist at ${platformDir} after dry-run`);
  }
  if (!existsSync(wrapperDir)) {
    fail(`expected wrapper dist at ${wrapperDir} after dry-run`);
  }
});

let tempDir;
step(`install wrapper + native package in a temp project (${targetName})`, () => {
  if (targetName !== defaultTargetName(process.platform, process.arch)) {
    console.log(
      `skipping install test: target ${targetName} differs from local platform ${process.platform}-${process.arch}; ` +
        'the wrapper would refuse to launch a cross-platform binary.'
    );
    return;
  }

  const platformDir = path.join(distRoot, targetName);
  const wrapperDir = path.join(distRoot, 'coven');

  const platformTgz = npmPack(platformDir);
  const wrapperTgz = npmPack(wrapperDir);

  tempDir = createScratchDir(path.join(repoRoot, 'target', 'script-scratch'), 'coven-prepublish');
  writeFileSync(
    path.join(tempDir, 'package.json'),
    `${JSON.stringify({ name: 'coven-prepublish-test', private: true, version: '0.0.0' }, null, 2)}\n`
  );

  // --omit=optional avoids npm trying to fetch the optional native package by
  // version from the public registry; we install the local tarball directly.
  const installArgs = ['install', '--no-package-lock', '--omit=optional', platformTgz, wrapperTgz];
  if (dashboardTarball) {
    installArgs.push(dashboardTarball);
  }
  run('npm', installArgs, {
    cwd: tempDir
  });

  const wrapperBin = path.join(
    tempDir,
    'node_modules',
    '.bin',
    process.platform === 'win32' ? 'coven.cmd' : 'coven'
  );
  if (!existsSync(wrapperBin)) {
    fail(`wrapper bin not present at ${wrapperBin} after install`);
  }

  if (dashboardTarball) {
    const dashboardEntry = path.join(
      tempDir,
      'node_modules',
      '@opencoven',
      'coven-memory-dashboard',
      'bin',
      'coven-memory-dashboard.mjs'
    );
    if (!existsSync(dashboardEntry)) {
      fail(`dashboard entry not present after install: ${dashboardEntry}`);
    }
  }

  const result = runPackagedUserJourney({
    dashboardExpected: Boolean(dashboardTarball),
    keepScratchDir: keepTempdir,
    wrapperBin
  });
  console.log(`packaged user journey ok (session ${result.sessionId})`);
  if (keepTempdir) {
    console.log(`packaged user journey scratch preserved at ${result.scratchRoot}`);
  }
});

export async function main() {
  try {
    for (const run of steps) {
      await run();
    }
    console.log(`\nAll ${stepNames.length} pre-publish checks passed for target=${targetName}.`);
    console.log('Next: bump version + tag (vX.Y.Z) to trigger .github/workflows/release-npm.yml.');
  } catch (error) {
    console.error(`\n${error.message}`);
    process.exitCode = 1;
  } finally {
    if (tempDir && !keepTempdir) {
      rmSync(tempDir, { recursive: true, force: true });
    } else if (tempDir) {
      console.log(`\nTemp project left at ${tempDir} (--keep-tempdir).`);
    }
  }
}

export function synthesizeDryRunVersion(
  packageName,
  {
    env = process.env,
    spawnSyncImpl = spawnSync,
    timeoutMs = DEFAULT_COMMAND_TIMEOUT_MS
  } = {}
) {
  const override = env.COVEN_NPM_DRY_RUN_VERSION?.trim();
  if (override) {
    const normalized = override.replace(/^v/, '');
    if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(normalized)) {
      fail(`COVEN_NPM_DRY_RUN_VERSION must be a semver version, got ${override}`);
    }
    return normalized;
  }

  const view = spawnSyncImpl('npm', ['view', packageName, 'version', '--silent'], {
    ...spawnOptionsForCommand(),
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8',
    timeout: timeoutMs
  });
  if (view.error?.code === 'ETIMEDOUT') {
    fail(
      `npm view timed out after ${timeoutMs}ms while reading current ${packageName} version. ` +
        'Set COVEN_NPM_DRY_RUN_VERSION to an unpublished higher semver and rerun.'
    );
  }
  if (view.error) {
    fail(
      `npm view failed while reading current ${packageName} version: ${view.error.message}. ` +
        'Set COVEN_NPM_DRY_RUN_VERSION to an unpublished higher semver and rerun.'
    );
  }
  if (view.status !== 0) {
    const stderr = view.stderr.trim();
    fail(
      `Could not read current ${packageName} version from npm. ` +
        'Set COVEN_NPM_DRY_RUN_VERSION to an unpublished higher semver and rerun.' +
        (stderr ? `\nnpm stderr:\n${stderr}` : '')
    );
  }
  const reported = view.stdout.trim();
  const match = reported.match(/^(\d+)\.(\d+)\.(\d+)/);
  if (!match) {
    fail(
      `Could not read current ${packageName} version from npm output: ${reported || '(empty)'}`
    );
  }
  // Bump patch (no prerelease suffix) so `npm publish --dry-run` accepts it
  // under the implicit "latest" tag. The version is never published, but it
  // must compare higher than what's already on the registry.
  const baseMajor = Number(match[1]);
  const baseMinor = Number(match[2]);
  const basePatch = Number(match[3]);
  return `${baseMajor}.${baseMinor}.${basePatch + 1}`;
}

if (isMainModule(import.meta.url)) {
  void main();
}

function ensureCommand(command, args) {
  const result = spawnSync(command, args, {
    ...spawnOptionsForCommand(),
    stdio: 'pipe',
    timeout: DEFAULT_COMMAND_TIMEOUT_MS
  });
  if (result.error?.code === 'ETIMEDOUT') {
    fail(`required command \`${command}\` timed out after ${DEFAULT_COMMAND_TIMEOUT_MS}ms`);
  }
  if (result.status !== 0) {
    fail(`required command \`${command}\` not available: ${result.error?.message ?? `exit ${result.status}`}`);
  }
  console.log(`${command}: ${result.stdout.toString().trim().split('\n')[0]}`);
}

function npmPack(packageDir) {
  const result = spawnSync('npm', ['pack', '--silent', '--pack-destination', packageDir], {
    ...spawnOptionsForCommand(),
    cwd: packageDir,
    stdio: ['ignore', 'pipe', 'inherit'],
    timeout: DEFAULT_COMMAND_TIMEOUT_MS
  });
  if (result.error?.code === 'ETIMEDOUT') {
    fail(`npm pack timed out after ${DEFAULT_COMMAND_TIMEOUT_MS}ms in ${packageDir}`);
  }
  if (result.status !== 0) {
    fail(`npm pack failed in ${packageDir} (exit ${result.status})`);
  }
  const tgzName = result.stdout.toString().trim().split('\n').pop();
  if (!tgzName || !tgzName.endsWith('.tgz')) {
    fail(`npm pack did not report a tarball name in ${packageDir} (got: ${tgzName})`);
  }
  const tgzPath = path.join(packageDir, tgzName);
  if (!existsSync(tgzPath)) {
    fail(`packed tarball missing at ${tgzPath}`);
  }
  console.log(`packed ${path.relative(repoRoot, tgzPath)}`);
  return tgzPath;
}

function run(command, commandArgs, options = {}) {
  const printable = [command, ...commandArgs].join(' ');
  console.log(`$ ${printable}`);
  const result = spawnSync(command, commandArgs, {
    ...spawnOptionsForCommand(options),
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    stdio: 'inherit',
    timeout: options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS
  });
  if (result.error?.code === 'ETIMEDOUT') {
    fail(`${printable} timed out after ${options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS}ms`);
  }
  if (result.error) {
    fail(`${printable} failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(`${printable} exited with ${result.status}`);
  }
}

function runCapture(command, commandArgs, options = {}) {
  const printable = [command, ...commandArgs].join(' ');
  console.log(`$ ${printable}`);
  const result = spawnSync(command, commandArgs, {
    ...spawnOptionsForCommand(options),
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8',
    timeout: options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS
  });
  if (result.error?.code === 'ETIMEDOUT') {
    fail(
      `${printable} timed out after ${options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS}ms\nstdout:\n${result.stdout ?? ''}\nstderr:\n${result.stderr ?? ''}`
    );
  }
  if (result.error) {
    fail(`${printable} failed: ${result.error.message}`);
  }
  if (result.status !== 0 && !(options.allowedExitCodes ?? []).includes(result.status)) {
    fail(`${printable} exited with ${result.status}\nstderr:\n${result.stderr}`);
  }
  return result;
}

function spawnOptionsForCommand(options = {}, platform = process.platform) {
  return {
    shell: platform === 'win32',
    ...options.spawnOptions
  };
}

function fail(message) {
  throw new Error(message);
}
