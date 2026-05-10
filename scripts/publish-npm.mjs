#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const distRoot = path.join(repoRoot, 'npm', 'dist');

const targets = {
  macos: {
    packageName: '@opencoven/cli-macos',
    os: 'darwin',
    cpu: 'arm64',
    rustTarget: 'aarch64-apple-darwin',
    binaryName: 'coven'
  },
  'linux-x64': {
    packageName: '@opencoven/cli-linux-x64',
    os: 'linux',
    cpu: 'x64',
    rustTarget: 'x86_64-unknown-linux-gnu',
    binaryName: 'coven'
  },
  windows: {
    packageName: '@opencoven/cli-windows',
    os: 'win32',
    cpu: 'x64',
    rustTarget: 'x86_64-pc-windows-msvc',
    binaryName: 'coven.exe'
  }
};

if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}

function main() {
  const args = new Set(process.argv.slice(2));
  const optionValue = (name) => {
    const prefix = `${name}=`;
    const found = process.argv.slice(2).find((arg) => arg.startsWith(prefix));
    return found ? found.slice(prefix.length) : undefined;
  };

  const targetName = optionValue('--target') ?? process.env.COVEN_NPM_TARGET ?? defaultTargetName(process.platform, process.arch);
  const dryRun = args.has('--dry-run') || !args.has('--publish');
  const skipBuild = args.has('--skip-build');
  const skipWrapper = args.has('--skip-wrapper');
  const version = releaseVersion(process.env, wrapperPackageVersion());
  const target = targets[targetName];

  if (!target) {
    fail(`Unsupported npm target ${targetName}. Known targets: ${Object.keys(targets).join(', ')}`);
  }

  validatePublishVersion(version, dryRun);
  validatePublishToken(process.env, dryRun);

  if (!skipBuild) {
    run('cargo', ['build', '--release', '--target', target.rustTarget]);
  }

  const binaryPath = path.join(repoRoot, 'target', target.rustTarget, 'release', target.binaryName);
  if (!existsSync(binaryPath)) {
    fail(`Built binary not found at ${binaryPath}`);
  }

  rmSync(distRoot, { recursive: true, force: true });
  mkdirSync(distRoot, { recursive: true });

  const platformDir = writePlatformPackage(targetName, target, binaryPath, version);

  run('npm', ['publish', dryRun ? '--dry-run' : '--access', dryRun ? undefined : 'public'].filter(Boolean), {
    cwd: platformDir,
    env: publishEnv(dryRun)
  });

  if (!skipWrapper) {
    const wrapperDir = writeWrapperPackage(version);
    run('npm', ['publish', dryRun ? '--dry-run' : '--access', dryRun ? undefined : 'public'].filter(Boolean), {
      cwd: wrapperDir,
      env: publishEnv(dryRun)
    });
  }

  console.log(`Prepared npm packages in ${distRoot}`);
  console.log(`${dryRun ? 'Dry-run completed' : 'Publish completed'} for ${target.packageName}${skipWrapper ? '' : ' and @opencoven/cli'} at version ${version}.`);
}

function writePlatformPackage(targetName, target, binaryPath, version) {
  const outDir = path.join(distRoot, targetName);
  const binDir = path.join(outDir, 'bin');
  mkdirSync(binDir, { recursive: true });

  const template = readFileSync(path.join(repoRoot, 'npm', 'coven-platform-template', 'package.json.tpl'), 'utf8');
  const packageJson = template
    .replaceAll('__PACKAGE_NAME__', target.packageName)
    .replaceAll('__VERSION__', version)
    .replaceAll('__OS__', target.os)
    .replaceAll('__CPU__', target.cpu);

  writeFileSync(path.join(outDir, 'package.json'), `${packageJson.trim()}\n`);
  cpSync(path.join(repoRoot, 'npm', 'coven-platform-template', 'README.md'), path.join(outDir, 'README.md'));
  cpSync(binaryPath, path.join(binDir, target.binaryName));
  chmodSync(path.join(binDir, target.binaryName), 0o755);
  return outDir;
}

function writeWrapperPackage(version) {
  const outDir = path.join(distRoot, 'coven');
  cpSync(path.join(repoRoot, 'npm', 'coven'), outDir, { recursive: true });
  const packagePath = path.join(outDir, 'package.json');
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  packageJson.version = version;
  for (const packageName of Object.keys(packageJson.optionalDependencies)) {
    packageJson.optionalDependencies[packageName] = version;
  }
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`);
  chmodSync(path.join(outDir, 'bin', 'coven.js'), 0o755);
  return outDir;
}

export function releaseVersion(env = process.env, packageVersion = wrapperPackageVersion()) {
  const fromEnv = env.COVEN_NPM_VERSION;
  if (fromEnv) {
    return fromEnv.replace(/^v/, '');
  }

  const githubRef = env.GITHUB_REF_NAME;
  if (githubRef?.startsWith('v')) {
    return githubRef.slice(1);
  }

  return packageVersion;
}

export function targetPackageName(targetName) {
  return targets[targetName]?.packageName;
}

export function defaultTargetName(platform, arch) {
  if (platform === 'darwin' && arch === 'arm64') {
    return 'macos';
  }
  if (platform === 'win32' && arch === 'x64') {
    return 'windows';
  }
  return `${platform}-${arch}`;
}

if ('NODE_TEST_CONTEXT' in process.env) {
  const assert = await import('node:assert/strict');
  const { test } = await import('node:test');

  test('defaultTargetName maps darwin arm64 to macos', () => {
    assert.strictEqual(defaultTargetName('darwin', 'arm64'), 'macos');
  });

  test('defaultTargetName falls back to platform-arch for non-special cases', () => {
    assert.strictEqual(defaultTargetName('linux', 'x64'), 'linux-x64');
    assert.strictEqual(defaultTargetName('darwin', 'x64'), 'darwin-x64');
  });

  test('defaultTargetName maps win32 x64 to windows', () => {
    assert.strictEqual(defaultTargetName('win32', 'x64'), 'windows');
  });
}

export function validatePublishVersion(version, dryRun) {
  if (!dryRun && version === '0.0.0') {
    throw new Error('Refusing real npm publish with placeholder version 0.0.0. Set COVEN_NPM_VERSION or run from a v* tag.');
  }
}

export function validatePublishToken(env, dryRun) {
  if (!dryRun && !env.NPM_TOKEN && !env.NODE_AUTH_TOKEN) {
    throw new Error('Refusing real npm publish without NPM_TOKEN or NODE_AUTH_TOKEN. Set one of these to authenticate with the npm registry.');
  }
}

function wrapperPackageVersion() {
  const wrapperPackage = JSON.parse(readFileSync(path.join(repoRoot, 'npm', 'coven', 'package.json'), 'utf8'));
  return wrapperPackage.version;
}

function run(command, args, options = {}) {
  const printable = [command, ...args].join(' ');
  console.log(`$ ${printable}`);
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    stdio: 'inherit'
  });
  if (result.error) {
    fail(`${printable} failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(`${printable} exited with ${result.status}`);
  }
}

export function publishEnv(dryRun, env = process.env) {
  if (dryRun) {
    return env;
  }

  const authToken = env.NPM_TOKEN || env.NODE_AUTH_TOKEN;
  return {
    ...env,
    NODE_AUTH_TOKEN: authToken
  };
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
