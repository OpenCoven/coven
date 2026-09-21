#!/usr/bin/env node
// Verify the owner-adjacent `opencoven-coven-client` crate's package contract
// from outside Cargo, so the crate release workflow can refuse a package that
// would publish with the wrong identity, a forbidden dependency, or a tarball
// missing the fixtures its own tests read.
//
//   node scripts/verify-coven-client-package.mjs                # metadata + tarball listing
//   node scripts/verify-coven-client-package.mjs --no-list      # metadata only (no cargo package)
//   node scripts/verify-coven-client-package.mjs --allow-dirty  # list an uncommitted tree (local only)
//
// Exits 0 on pass and 1 on the first failing group, printing one line per
// assertion. `crates/coven-client/tests/package_contract.rs` checks the same
// contract from inside Cargo.
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const PACKAGE_NAME = 'opencoven-coven-client';
export const LIBRARY_NAME = 'coven_client';
export const FORBIDDEN_DEPENDENCIES = [
  'coven-cli',
  'coven-agents',
  'clap',
  'ratatui',
  'crossterm',
  'dialoguer',
  'inquire'
];
export const REQUIRED_PACKAGE_FILES = [
  'Cargo.toml',
  'README.md',
  'src/lib.rs',
  'tests/package_contract.rs',
  'fixtures/health.json',
  'fixtures/error.json'
];

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Check the package's `cargo metadata` entry. Returns a list of failure
 * strings; an empty list is a pass.
 */
export function checkPackageMetadata(pkg) {
  const failures = [];
  if (!pkg) {
    return [`package ${PACKAGE_NAME} is not in the workspace metadata`];
  }
  if (pkg.name !== PACKAGE_NAME) {
    failures.push(`package name is ${pkg.name}, expected ${PACKAGE_NAME}`);
  }
  if (!/^0\.\d+\.\d+$/.test(pkg.version ?? '')) {
    failures.push(`version ${pkg.version} is not a pre-1.0 x.y.z version`);
  }
  if (pkg.license !== 'MIT') {
    failures.push(`license is ${pkg.license ?? 'unset'}, expected MIT`);
  }
  if (pkg.repository !== 'https://github.com/OpenCoven/coven') {
    failures.push(`repository is ${pkg.repository ?? 'unset'}`);
  }
  if (!pkg.readme) {
    failures.push('readme is unset');
  }
  if (!(pkg.description ?? '').includes('coven.daemon.v1')) {
    failures.push('description does not name the coven.daemon.v1 contract');
  }
  const lib = (pkg.targets ?? []).find((target) => (target.kind ?? []).includes('lib'));
  if (!lib) {
    failures.push('no lib target');
  } else if (lib.name !== LIBRARY_NAME) {
    failures.push(`lib target is ${lib.name}, expected ${LIBRARY_NAME}`);
  }
  const dependencyNames = new Set((pkg.dependencies ?? []).map((dependency) => dependency.name));
  for (const forbidden of FORBIDDEN_DEPENDENCIES) {
    if (dependencyNames.has(forbidden)) {
      failures.push(`forbidden dependency ${forbidden}`);
    }
  }
  return failures;
}

/**
 * Check the `cargo package --list` output. Returns failure strings.
 */
export function checkPackageListing(lines) {
  const listed = new Set(lines.map((line) => line.trim()).filter(Boolean));
  const failures = [];
  for (const required of REQUIRED_PACKAGE_FILES) {
    if (!listed.has(required)) {
      failures.push(`tarball is missing ${required}`);
    }
  }
  return failures;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe']
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${[command, ...args].join(' ')} exited with ${result.status}\n${result.stderr.trim()}`);
  }
  return result.stdout;
}

function report(group, failures) {
  if (failures.length === 0) {
    console.log(`verify-coven-client-package: ok ${group}`);
    return true;
  }
  for (const failure of failures) {
    console.error(`verify-coven-client-package: FAIL ${group}: ${failure}`);
  }
  return false;
}

export function main(argv = process.argv.slice(2)) {
  const listTarball = !argv.includes('--no-list');
  const allowDirty = argv.includes('--allow-dirty');
  const unknown = argv.filter((argument) => argument !== '--no-list' && argument !== '--allow-dirty');
  if (unknown.length > 0) {
    console.error(
      `usage: verify-coven-client-package.mjs [--no-list] [--allow-dirty] (unknown: ${unknown.join(' ')})`
    );
    return 1;
  }

  const metadata = JSON.parse(run('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked']));
  const pkg = metadata.packages.find((candidate) => candidate.name === PACKAGE_NAME);
  const metadataFailures = checkPackageMetadata(pkg);
  if (pkg?.readme && pkg.manifest_path) {
    const readmePath = path.join(path.dirname(pkg.manifest_path), pkg.readme);
    if (!existsSync(readmePath)) {
      metadataFailures.push(`readme ${pkg.readme} does not exist next to the manifest`);
    }
  }
  if (!report('metadata', metadataFailures)) {
    return 1;
  }

  if (!listTarball) {
    return 0;
  }
  // `cargo package` refuses an uncommitted tree by default; the release
  // workflow always runs on a clean tagged checkout, so `--allow-dirty` is a
  // local convenience and never the path a release takes.
  const packageArguments = ['package', '-p', PACKAGE_NAME, '--locked', '--list'];
  if (allowDirty) {
    packageArguments.push('--allow-dirty');
  }
  const listing = run('cargo', packageArguments);
  if (!report('tarball', checkPackageListing(listing.split('\n')))) {
    return 1;
  }
  console.log(`verify-coven-client-package: ok ${PACKAGE_NAME} ${pkg.version}`);
  return 0;
}

const invokedDirectly =
  process.argv[1] !== undefined && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  try {
    process.exitCode = main();
  } catch (error) {
    console.error(`verify-coven-client-package: FAIL ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}
