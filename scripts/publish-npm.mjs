#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const distRoot = path.join(repoRoot, 'npm', 'dist');

const targets = {
  'darwin-arm64': {
    packageName: '@opencoven/cli-darwin-arm64',
    os: 'darwin',
    cpu: 'arm64',
    rustTarget: 'aarch64-apple-darwin',
    binaryName: 'coven'
  },
  'darwin-x64': {
    packageName: '@opencoven/cli-darwin-x64',
    os: 'darwin',
    cpu: 'x64',
    rustTarget: 'x86_64-apple-darwin',
    binaryName: 'coven'
  },
  'linux-x64': {
    packageName: '@opencoven/cli-linux-x64',
    os: 'linux',
    cpu: 'x64',
    rustTarget: 'x86_64-unknown-linux-gnu',
    binaryName: 'coven'
  },
  'linux-arm64': {
    packageName: '@opencoven/cli-linux-arm64',
    os: 'linux',
    cpu: 'arm64',
    rustTarget: 'aarch64-unknown-linux-gnu',
    binaryName: 'coven'
  },
  'win32-x64': {
    packageName: '@opencoven/cli-win32-x64',
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

  const targetName = optionValue('--target') ?? process.env.COVEN_NPM_TARGET ?? `${process.platform}-${process.arch}`;
  const dryRun = args.has('--dry-run') || !args.has('--publish');
  const skipBuild = args.has('--skip-build');
  const version = releaseVersion(process.env, wrapperPackageVersion());
  const target = targets[targetName];

  if (!target) {
    fail(`Unsupported npm target ${targetName}. Known targets: ${Object.keys(targets).join(', ')}`);
  }

  validatePublishVersion(version, dryRun);
  if (!dryRun && process.env.NPM_TOKEN === undefined) {
    fail('Refusing real npm publish without NPM_TOKEN. Prefer --dry-run until Val manually approves publishing.');
  }

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
  const wrapperDir = writeWrapperPackage(version);

  run('npm', ['publish', dryRun ? '--dry-run' : '--access', dryRun ? undefined : 'public'].filter(Boolean), {
    cwd: platformDir,
    env: publishEnv(dryRun)
  });
  run('npm', ['publish', dryRun ? '--dry-run' : '--access', dryRun ? undefined : 'public'].filter(Boolean), {
    cwd: wrapperDir,
    env: publishEnv(dryRun)
  });

  console.log(`Prepared npm packages in ${distRoot}`);
  console.log(`${dryRun ? 'Dry-run completed' : 'Publish completed'} for ${target.packageName} and @opencoven/cli at version ${version}.`);
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

export function validatePublishVersion(version, dryRun) {
  if (!dryRun && version === '0.0.0') {
    throw new Error('Refusing real npm publish with placeholder version 0.0.0. Set COVEN_NPM_VERSION or run from a v* tag.');
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

function publishEnv(dryRun) {
  if (dryRun) {
    return process.env;
  }
  return {
    ...process.env,
    NODE_AUTH_TOKEN: process.env.NPM_TOKEN
  };
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
