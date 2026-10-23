#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const distRoot = path.join(repoRoot, 'npm', 'dist');
const primaryWrapperPackageName = '@opencoven/cli';
const wrapperPackageNames = [primaryWrapperPackageName];
const RELEASE_SUPPORT_PATH = path.join(repoRoot, '.github', 'release-support.json');
const PUBLISHABLE_STATE = 'supported';
const BLOCKED_STATES = new Set(['support-disabled', 'security-blocked']);

const targets = {
  macos: {
    packageName: '@opencoven/cli-macos',
    os: 'darwin',
    cpu: 'arm64',
    rustTarget: 'aarch64-apple-darwin',
    binaryName: 'coven',
    // The per-mount NFS export process. Declared only for macOS because
    // `afs_mount::backend()` returns None elsewhere, and the `mount` feature
    // does not compile on Windows at all. Shipping it where nothing can use
    // it would be dead weight; omitting it on macOS makes `afsMount` false
    // for every installed user, which is what this fixes.
    helperBinaryName: 'coven-afs-serve'
  },
  'macos-x64': {
    packageName: '@opencoven/cli-macos-x64',
    os: 'darwin',
    cpu: 'x64',
    rustTarget: 'x86_64-apple-darwin',
    binaryName: 'coven',
    helperBinaryName: 'coven-afs-serve'
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

const nativePackageSets = {
  'pre-intel': ['macos', 'linux-x64', 'windows'],
  'post-intel': ['macos', 'macos-x64', 'linux-x64', 'windows']
};

// npm installs these into a consumer's dependency tree. `devDependencies` is
// absent: npm does not install it for a consumer of a published package.
// `bundleDependencies` is absent too — npm defines it as an array of names, not
// a spec map, and bundled code ships inside the tarball under the tarball's own
// integrity hash.
const LOCKSTEP_DEPENDENCY_FIELDS = ['dependencies', 'optionalDependencies'];

// A specifier npm resolves from somewhere other than the registry, so it
// carries no integrity hash and needs git or a network fetch at install time.
// A semver range never contains a slash, which is what separates a bare
// `owner/repo` GitHub shorthand from a legitimate range.
const NON_REGISTRY_SPECIFIER =
  /^(?:git(?:\+[a-z]+)?:|ssh:|https?:|file:|link:|portal:|github:|gitlab:|bitbucket:|gist:)|\//i;

/**
 * Fail closed on any published dependency npm cannot install from the registry
 * against an integrity hash.
 *
 * Every installed dependency of a published wrapper is a lockstep-versioned
 * `@opencoven/cli-*` native, so each spec must equal the release version
 * exactly. That is stronger and simpler than "looks like a version": it also
 * rejects a `0.0.0` placeholder that escaped the rewrite in
 * `writeWrapperPackage`, which is exact-shaped but names a version that was
 * never published. Prerelease and build-metadata versions therefore pass or
 * fail with the release itself rather than against a separate shape rule.
 *
 * Only this package's own manifest is in scope. A transitive git dependency
 * pulled in by an otherwise well-formed dependency is invisible here; the
 * defense against that is not declaring dependencies whose own closure is
 * unaudited.
 *
 * `peerDependencies` are legitimately ranges, so they are held only to the
 * weaker rule that they name something npm can integrity-check.
 */
export function assertInstallableDependencies(packageJson, version) {
  for (const field of LOCKSTEP_DEPENDENCY_FIELDS) {
    for (const [name, spec] of Object.entries(packageJson[field] ?? {})) {
      if (spec !== version) {
        throw new Error(
          `Refusing to publish ${packageJson.name}: ${field}['${name}'] must be ` +
            `pinned to the release version ${version}, got '${spec}'. Every ` +
            `installed dependency of a published package is a lockstep-versioned ` +
            `native package npm resolves from the registry with an integrity hash.`
        );
      }
    }
  }
  for (const [name, spec] of Object.entries(packageJson.peerDependencies ?? {})) {
    if (NON_REGISTRY_SPECIFIER.test(spec)) {
      throw new Error(
        `Refusing to publish ${packageJson.name}: peerDependencies['${name}'] ` +
          `resolves outside the npm registry ('${spec}'), so npm cannot ` +
          `integrity-check it and installing it needs credentials.`
      );
    }
  }
}

if (isMainModule()) {
  main();
}

export function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && moduleUrl === pathToFileURL(argv1).href;
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
  const wrapperOnly = args.has('--wrapper-only');
  const version = releaseVersion(process.env, wrapperPackageVersion());
  const target = targets[targetName];

  if (wrapperOnly && skipWrapper) {
    fail('--wrapper-only and --skip-wrapper cannot be used together');
  }
  if (!wrapperOnly && !target) {
    fail(`Unsupported npm target ${targetName}. Known targets: ${Object.keys(targets).join(', ')}`);
  }

  validatePublishVersion(version, dryRun);
  validatePublishToken(process.env, dryRun);
  try {
    const support = loadReleaseSupport();
    // Dry-runs stage arbitrary versions (CI uses 999.0.0); only a registry
    // write has to be on the supported line.
    if (!dryRun) {
      assertSupportedReleaseLine(support, version);
    }
    if (!wrapperOnly) {
      assertPublishableSurface(support, target.packageName);
    }
    if (!skipWrapper) {
      for (const packageName of wrapperPackageNames) {
        assertPublishableSurface(support, packageName);
      }
      // The wrapper's optionalDependencies name these natives, so it is only
      // publishable when every one of them is.
      for (const targetName of nativeTargetNamesForPackageSet()) {
        assertPublishableSurface(support, targets[targetName].packageName);
      }
    }
  } catch (error) {
    fail(error.message);
  }

  prepareDistRoot();

  if (!wrapperOnly) {
    if (!skipBuild) {
      run('cargo', ['build', '--release', '--package', 'coven-cli', '--target', target.rustTarget]);
      if (target.helperBinaryName) {
        // Built here as well as in CI, so a local `--dry-run` produces the
        // same payload the release does. Without this the helper check below
        // would fail locally for a reason that has nothing to do with the
        // package being wrong.
        run('cargo', [
          'build', '--release', '-p', 'coven-afs',
          '--features', 'mount', '--bins',
          '--target', target.rustTarget
        ]);
      }
    }

    const binaryPath = path.join(repoRoot, 'target', target.rustTarget, 'release', target.binaryName);
    if (!existsSync(binaryPath)) {
      fail(`Built binary not found at ${binaryPath}`);
    }

    const platformDir = writePlatformPackage(targetName, target, binaryPath, version);
    publishPackage(target.packageName, version, dryRun, platformDir);
  }

  if (!skipWrapper) {
    for (const packageName of wrapperPackageNames) {
      const wrapperDir = writeWrapperPackage(version, packageName, nativeTargetNamesForPackageSet());
      publishPackage(packageName, version, dryRun, wrapperDir);
    }
  }

  console.log(`Prepared npm packages in ${distRoot}`);
  const publishedNames = [
    ...(wrapperOnly ? [] : [target.packageName]),
    ...(skipWrapper ? [] : wrapperPackageNames)
  ];
  console.log(`${dryRun ? 'Dry-run completed' : 'Publish completed'} for ${publishedNames.join(', ')} at version ${version}.`);
}

export function prepareDistRoot(root = distRoot) {
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
}

function publishPackage(packageName, version, dryRun, cwd) {
  let published = false;
  if (!dryRun) {
    try {
      published = packageVersionPublished(packageName, version);
    } catch (error) {
      fail(error.message);
    }
  }
  if (published) {
    fail(`${packageName}@${version} is already published to npm. Refusing to publish because this package version already exists on npm and could trust an unverified pre-existing artifact.`);
  }
  run('npm', publishArgs(dryRun), {
    cwd,
    env: publishEnv(dryRun)
  });
}

/// Whether this exact version is already on the registry. Only two answers
/// are trusted: npm printing the version back, and npm's E404. A network,
/// auth, or registry error is neither, and throws rather than letting the
/// publish proceed on a guess (#805).
export function packageVersionPublished(packageName, version, runner = npmViewRunner) {
  const result = runner(packageName, version);
  const stdout = String(result.stdout ?? '').trim();
  const stderr = String(result.stderr ?? '');
  if (result.status === 0 && stdout === version) {
    return true;
  }
  if (result.status !== 0 && /\bE404\b/.test(stderr)) {
    return false;
  }
  throw new Error(
    `Refusing to publish ${packageName}@${version}: npm view could not say whether it is already published ` +
      `(exit ${result.status ?? 'unknown'}, stdout ${JSON.stringify(stdout.slice(0, 80))}, stderr ${JSON.stringify(stderr.trim().split('\n').slice(0, 2).join(' '))}).`
  );
}

function npmViewRunner(packageName, version) {
  return spawnSync('npm', ['view', `${packageName}@${version}`, 'version'], {
    ...spawnOptionsForCommand(),
    cwd: repoRoot,
    env: process.env,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe']
  });
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

  try {
    // The template declares no dependencies today; the gate is what keeps that
    // true for the native packages as well as the wrapper.
    assertInstallableDependencies(JSON.parse(packageJson), version);
  } catch (error) {
    fail(error.message);
  }
  writeFileSync(path.join(outDir, 'package.json'), `${packageJson.trim()}\n`);
  cpSync(path.join(repoRoot, 'npm', 'coven-platform-template', 'README.md'), path.join(outDir, 'README.md'));
  cpSync(binaryPath, path.join(binDir, target.binaryName));
  chmodSync(path.join(binDir, target.binaryName), 0o755);

  if (target.helperBinaryName) {
    const helperPath = path.join(path.dirname(binaryPath), target.helperBinaryName);
    if (!existsSync(helperPath)) {
      // Loud rather than optional. v0.3.0 shipped without this helper and
      // every installed user got `afsMount: false`; nothing failed, because
      // nothing looked.
      fail(`Declared helper binary not found at ${helperPath}`);
    }
    cpSync(helperPath, path.join(binDir, target.helperBinaryName));
    chmodSync(path.join(binDir, target.helperBinaryName), 0o755);
  }
  return outDir;
}

function writeWrapperPackage(version, packageName = primaryWrapperPackageName, nativeTargetNames = nativeTargetNamesForPackageSet()) {
  const outDir = path.join(distRoot, wrapperPackageDirName(packageName));
  cpSync(path.join(repoRoot, 'npm', 'coven'), outDir, { recursive: true });
  const packagePath = path.join(outDir, 'package.json');
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  packageJson.name = packageName;
  packageJson.version = version;
  for (const optionalName of Object.keys(packageJson.optionalDependencies)) {
    if (optionalName.startsWith('@opencoven/cli-')) {
      const targetName = Object.entries(targets).find(([, target]) => target.packageName === optionalName)?.[0];
      if (targetName && !nativeTargetNames.includes(targetName)) {
        delete packageJson.optionalDependencies[optionalName];
      } else {
        packageJson.optionalDependencies[optionalName] = version;
      }
    }
  }
  try {
    assertInstallableDependencies(packageJson, version);
  } catch (error) {
    // Report like every other publish failure: a bare actionable line, not an
    // uncaught-throw stack dump in the release log.
    fail(error.message);
  }
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`);
  rewriteWrapperText(path.join(outDir, 'README.md'), packageName);
  rewriteWrapperText(path.join(outDir, 'bin', 'coven.js'), packageName);
  chmodSync(path.join(outDir, 'bin', 'coven.js'), 0o755);
  return outDir;
}

function rewriteWrapperText(filePath, packageName) {
  const text = readFileSync(filePath, 'utf8');
  writeFileSync(filePath, wrapperTextForPackage(text, packageName));
}

export function wrapperTextForPackage(text, packageName) {
  return text.replace(/@opencoven\/cli(?!-)/g, packageName);
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

/// The extra binary a platform package ships beside `coven`, or undefined.
///
/// Exported so the packaging contract is testable: v0.3.0 shipped without the
/// AFS export helper and nothing failed, because nothing inspected what went
/// into the tarball.
export function targetHelperBinaryName(targetName) {
  return targets[targetName]?.helperBinaryName;
}

export function nativeTargetNamesForPackageSet(packageSet = process.env.COVEN_NPM_NATIVE_PACKAGE_SET ?? 'post-intel') {
  const targetNames = nativePackageSets[packageSet];
  if (!targetNames) {
    throw new Error(`Unsupported native package set ${packageSet}. Known sets: ${Object.keys(nativePackageSets).join(', ')}`);
  }
  return [...targetNames];
}

export function wrapperPackageNameList() {
  return [...wrapperPackageNames];
}

export function wrapperPackageDirName(packageName) {
  return packageName === primaryWrapperPackageName ? 'coven' : 'coven-alias';
}

export function defaultTargetName(platform, arch) {
  if (platform === 'darwin' && arch === 'arm64') {
    return 'macos';
  }
  if (platform === 'darwin' && arch === 'x64') {
    return 'macos-x64';
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

  test('defaultTargetName maps darwin x64 to macos-x64', () => {
    assert.strictEqual(defaultTargetName('darwin', 'x64'), 'macos-x64');
  });

  test('defaultTargetName falls back to platform-arch for non-special cases', () => {
    assert.strictEqual(defaultTargetName('linux', 'x64'), 'linux-x64');
    assert.strictEqual(defaultTargetName('freebsd', 'x64'), 'freebsd-x64');
  });

  test('defaultTargetName maps win32 x64 to windows', () => {
    assert.strictEqual(defaultTargetName('win32', 'x64'), 'windows');
  });
}

/// Reads the reviewed support policy. Packaging metadata in this script lists
/// what *can* be built; .github/release-support.json decides what may be
/// published, so a blocked surface cannot come back through a package-set or
/// template edit alone.
export function loadReleaseSupport(filePath = RELEASE_SUPPORT_PATH) {
  const support = JSON.parse(readFileSync(filePath, 'utf8'));
  if (support?.schemaVersion !== 1 || support?.kind !== 'coven.release-support') {
    throw new Error(`${path.basename(filePath)} must be coven.release-support schemaVersion 1.`);
  }
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(String(support.supportedReleaseLine ?? ''))) {
    throw new Error(`${path.basename(filePath)} supportedReleaseLine must be MAJOR.MINOR, got ${JSON.stringify(support.supportedReleaseLine)}.`);
  }
  for (const [packageName, entry] of Object.entries(support.packages ?? {})) {
    if (entry?.state === PUBLISHABLE_STATE) {
      continue;
    }
    if (!BLOCKED_STATES.has(entry?.state) || !entry?.reason || !/^https:\/\/github\.com\/OpenCoven\/[^/]+\/issues\/\d+$/.test(String(entry?.issue ?? ''))) {
      throw new Error(
        `${path.basename(filePath)} ${packageName} must be "supported", or "support-disabled"/"security-blocked" with a reason and an OpenCoven issue URL.`
      );
    }
  }
  return support;
}

export function assertPublishableSurface(support, packageName) {
  const entry = support.packages?.[packageName];
  if (!entry) {
    throw new Error(`Refusing to publish ${packageName}: it is not listed in .github/release-support.json, so it has no support state.`);
  }
  if (entry.state !== PUBLISHABLE_STATE) {
    throw new Error(`Refusing to publish ${packageName}: .github/release-support.json marks it ${entry.state} (${entry.reason}; ${entry.issue}).`);
  }
}

/// A release must be on the supported line. Coven has no long-term support
/// before 1.0, so a patch to an older minor line is refused, and opening a new
/// line means updating the support policy and SECURITY.md first. The 0.0.0
/// placeholder used by local dry-runs is exempt.
export function assertSupportedReleaseLine(support, version) {
  if (version === '0.0.0') {
    return;
  }
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(version);
  if (!match) {
    throw new Error(`Refusing to publish version ${JSON.stringify(version)}: it is not a stable MAJOR.MINOR.PATCH version.`);
  }
  const line = `${match[1]}.${match[2]}`;
  if (line !== support.supportedReleaseLine) {
    throw new Error(
      `Refusing to publish ${version}: .github/release-support.json supports the ${support.supportedReleaseLine}.x line, not ${line}.x. ` +
        'Open a new line by updating the support policy and SECURITY.md through a reviewed pull request first.'
    );
  }
}

export function validatePublishVersion(version, dryRun) {
  if (!dryRun && version === '0.0.0') {
    throw new Error('Refusing real npm publish with placeholder version 0.0.0. Set COVEN_NPM_VERSION or run from a v* tag.');
  }
}

export function validatePublishToken(env, dryRun) {
  if (dryRun) {
    return;
  }
  if (isOidcContext(env)) {
    // Authenticated via GitHub Actions OIDC trusted publishing — no long-lived
    // npm token needed. `npm publish --provenance` will exchange the OIDC
    // token for a short-lived registry credential.
    return;
  }
  if (!env.NPM_TOKEN && !env.NODE_AUTH_TOKEN) {
    throw new Error('Refusing real npm publish without OIDC trusted publishing or an NPM_TOKEN / NODE_AUTH_TOKEN fallback.');
  }
}

export function isOidcContext(env = process.env) {
  // Both vars are injected by GitHub Actions when `permissions.id-token: write`
  // is granted to the job. Their presence indicates the job can mint OIDC
  // tokens; absence means we must fall back to a static npm token.
  return Boolean(env.ACTIONS_ID_TOKEN_REQUEST_TOKEN && env.ACTIONS_ID_TOKEN_REQUEST_URL);
}

export function publishArgs(dryRun, env = process.env) {
  if (dryRun) {
    return ['publish', '--dry-run'];
  }
  const args = ['publish', '--access', 'public'];
  if (isOidcContext(env)) {
    args.push('--provenance');
  }
  return args;
}

function wrapperPackageVersion() {
  const wrapperPackage = JSON.parse(readFileSync(path.join(repoRoot, 'npm', 'coven', 'package.json'), 'utf8'));
  return wrapperPackage.version;
}

function run(command, args, options = {}) {
  const printable = [command, ...args].join(' ');
  console.log(`$ ${printable}`);
  const result = spawnSync(command, args, {
    ...spawnOptionsForCommand(),
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
  if (isOidcContext(env)) {
    // npm 11+ auto-detects OIDC via ACTIONS_ID_TOKEN_REQUEST_* and does not
    // read NODE_AUTH_TOKEN. Leaving the env untouched avoids accidentally
    // smuggling in a stale token from a misconfigured fallback path.
    return env;
  }

  const authToken = env.NPM_TOKEN || env.NODE_AUTH_TOKEN;
  return {
    ...env,
    NODE_AUTH_TOKEN: authToken
  };
}

function spawnOptionsForCommand(platform = process.platform) {
  return {
    shell: platform === 'win32'
  };
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
