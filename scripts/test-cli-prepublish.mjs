#!/usr/bin/env node
// Pre-publish smoke test for the npm-distributed Coven CLI.
//
// What it does:
//   1. Verifies prerequisites (node, npm, cargo).
//   2. Runs the secrets scan, onboarding guardrails, and publish-npm.mjs unit tests.
//   3. Stages the dist tree by running publish-npm.mjs in --dry-run mode
//      (which also runs `cargo build --release --target <rust-target>` unless
//      --skip-build is passed) and lets `npm publish --dry-run` validate the
//      platform + wrapper tarballs.
//   4. `npm pack`s the native and wrapper packages, installs them into a fresh
//      temp project, and invokes the wrapper bin to confirm the native binary
//      is resolved, executable, and can start the daemon with isolated state.
//
// Flags:
//   --target=<name>       Override the npm target (macos, linux-x64, windows).
//                         Defaults to the local platform.
//   --skip-build          Reuse an existing release binary at
//                         target/<rust-target>/release/coven instead of
//                         re-running `cargo build --release --target ...`.
//   --with-cargo-gates    Also run `cargo fmt --check`, `cargo clippy`, and
//                         `cargo test --workspace --locked` (the CI verify
//                         gates). Off by default to keep local runs fast.
//   --skip-secrets-scan   Skip `python3 scripts/check-secrets.py` for local
//                         iteration; CI still runs it.
//   --keep-tempdir        Leave the temp install dir on disk for inspection.
//   COVEN_NPM_DRY_RUN_VERSION=vX.Y.Z
//                         Override the synthesized dry-run version when the
//                         public npm registry cannot be reached.
//
// Exit code is non-zero on the first failing step.

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { defaultTargetName } from './publish-npm.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const distRoot = path.join(repoRoot, 'npm', 'dist');
const DEFAULT_COMMAND_TIMEOUT_MS = 120_000;

const PLATFORM_TARGETS = {
  macos: { packageName: '@opencoven/cli-macos', binaryName: 'coven' },
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
    run('python3', ['scripts/check-secrets.py']);
  });
}

step('onboarding and publish guardrails', () => {
  run('node', ['--test', 'scripts/onboarding-docs-test.mjs', 'scripts/publish-npm-test.mjs']);
});

if (withCargoGates) {
  step('cargo fmt --check', () => run('cargo', ['fmt', '--check']));
  step('cargo clippy', () =>
    run('cargo', ['clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'])
  );
  step('cargo test --workspace --locked', () =>
    run('cargo', ['test', '--workspace', '--locked'])
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

  tempDir = mkdtempSync(path.join(tmpdir(), 'coven-prepublish-'));
  writeFileSync(
    path.join(tempDir, 'package.json'),
    `${JSON.stringify({ name: 'coven-prepublish-test', private: true, version: '0.0.0' }, null, 2)}\n`
  );

  // --omit=optional avoids npm trying to fetch the optional native package by
  // version from the public registry; we install the local tarball directly.
  run('npm', ['install', '--no-package-lock', '--omit=optional', platformTgz, wrapperTgz], {
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

  const smokeEnv = {
    ...process.env,
    COVEN_HOME: path.join(tempDir, 'coven-home'),
    PATH: firstRunSmokePath(wrapperBin)
  };
  mkdirSync(smokeEnv.COVEN_HOME, { recursive: true });

  const versionOutput = runCapture(wrapperBin, ['--version'], { env: smokeEnv });
  if (!versionOutput.stdout.trim()) {
    fail('`coven --version` produced no output');
  }
  console.log(`coven --version => ${versionOutput.stdout.trim()}`);

  const doctorOutput = runCapture(wrapperBin, ['doctor'], { env: smokeEnv });
  if (!doctorOutput.stdout.includes('Coven doctor')) {
    fail(
      `\`coven doctor\` did not print the expected banner.\nstdout:\n${doctorOutput.stdout}\nstderr:\n${doctorOutput.stderr}`
    );
  }
  for (const expected of [
    'Install and authenticate at least one harness in this same shell',
    'npm install -g @openai/codex && codex login',
    'npm install -g @anthropic-ai/claude-code && claude doctor'
  ]) {
    if (!doctorOutput.stdout.includes(expected)) {
      fail(`\`coven doctor\` missing first-run setup guidance "${expected}".\nstdout:\n${doctorOutput.stdout}\nstderr:\n${doctorOutput.stderr}`);
    }
  }
  console.log('coven doctor first-run setup guidance present');

  const helpOutput = runCapture(wrapperBin, ['--help'], { env: smokeEnv });
  if (helpOutput.status !== 0) {
    fail(`\`coven --help\` exited with ${helpOutput.status}\nstderr:\n${helpOutput.stderr}`);
  }
  if (!helpOutput.stdout.toLowerCase().includes('usage')) {
    fail(`\`coven --help\` missing usage section.\nstdout:\n${helpOutput.stdout}`);
  }

  let daemonStarted = false;
  try {
    const startOutput = runDaemonStart(wrapperBin, smokeEnv);
    daemonStarted = true;
    if (startOutput && !startOutput.stdout.includes('status=running')) {
      fail(`\`coven daemon start\` did not report status=running.\nstdout:\n${startOutput.stdout}\nstderr:\n${startOutput.stderr}`);
    }

    const statusOutput = runCapture(wrapperBin, ['daemon', 'status'], { env: smokeEnv });
    if (!statusOutput.stdout.includes('status=running') || !statusOutput.stdout.includes('ok=true')) {
      fail(`\`coven daemon status\` did not report a healthy running daemon.\nstdout:\n${statusOutput.stdout}\nstderr:\n${statusOutput.stderr}`);
    }

    const sessionsOutput = runCapture(wrapperBin, ['sessions', '--plain'], { env: smokeEnv });
    if (!sessionsOutput.stdout.includes('No active Coven sessions yet.')) {
      fail(`\`coven sessions --plain\` did not report an empty fresh store.\nstdout:\n${sessionsOutput.stdout}\nstderr:\n${sessionsOutput.stderr}`);
    }
    console.log('coven daemon lifecycle verified');
  } finally {
    if (daemonStarted) {
      runCapture(wrapperBin, ['daemon', 'stop'], { env: smokeEnv });
    }
  }
});

(async () => {
  try {
    for (const run of steps) {
      await run();
    }
    console.log(`\nAll ${stepNames.length} pre-publish checks passed for target=${targetName}.`);
    console.log('Next: bump version + tag (vX.Y.Z) to trigger .github/workflows/release-npm.yml.');
  } catch (error) {
    console.error(`\n${error.message}`);
    process.exit(1);
  } finally {
    if (tempDir && !keepTempdir) {
      rmSync(tempDir, { recursive: true, force: true });
    } else if (tempDir) {
      console.log(`\nTemp project left at ${tempDir} (--keep-tempdir).`);
    }
  }
})();

function synthesizeDryRunVersion(packageName) {
  const override = process.env.COVEN_NPM_DRY_RUN_VERSION?.trim();
  if (override) {
    const normalized = override.replace(/^v/, '');
    if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(normalized)) {
      fail(`COVEN_NPM_DRY_RUN_VERSION must be a semver version, got ${override}`);
    }
    return normalized;
  }

  const view = spawnSync('npm', ['view', packageName, 'version', '--silent'], {
    ...spawnOptionsForCommand(),
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8'
  });
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
  if (result.status !== 0) {
    fail(`${printable} exited with ${result.status}\nstderr:\n${result.stderr}`);
  }
  return result;
}

function runDaemonStart(wrapperBin, smokeEnv) {
  if (process.platform === 'win32') {
    // Capturing stdout from npm's Windows .cmd shim can keep the pipe open
    // while the background daemon is alive. Run start attached to this process,
    // then verify health via a separate captured status command below.
    run(wrapperBin, ['daemon', 'start'], { env: smokeEnv });
    return undefined;
  }
  return runCapture(wrapperBin, ['daemon', 'start'], { env: smokeEnv });
}

function spawnOptionsForCommand(options = {}, platform = process.platform) {
  return {
    shell: platform === 'win32',
    ...options.spawnOptions
  };
}

function firstRunSmokePath(wrapperBin) {
  return [
    path.dirname(wrapperBin),
    path.dirname(process.execPath)
  ].join(path.delimiter);
}

function fail(message) {
  throw new Error(message);
}
