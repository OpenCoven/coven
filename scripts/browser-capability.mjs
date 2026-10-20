/** Optional browser-test declaration detection. No package installation or config execution. */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';

export class BrowserCapabilityError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'BrowserCapabilityError';
    this.code = code;
  }
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function readManifest(packageRoot) {
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
  } catch {
    throw new BrowserCapabilityError('invalid-manifest', 'Package manifest is missing, unreadable, or invalid JSON.');
  }
  if (!isObject(manifest)) {
    throw new BrowserCapabilityError('invalid-manifest', 'Package manifest must be an object.');
  }
  for (const key of ['dependencies', 'devDependencies', 'optionalDependencies', 'scripts']) {
    if (manifest[key] === undefined) continue;
    if (!isObject(manifest[key]) || Object.values(manifest[key]).some(value => typeof value !== 'string' || !value.trim())) {
      throw new BrowserCapabilityError('invalid-manifest', 'Dependency and script maps must contain nonempty strings.');
    }
  }
  return manifest;
}

export function detectDeclaration(packageRoot) {
  const manifest = readManifest(packageRoot);
  const supported = ['@playwright/test', '@vitest/browser-playwright', 'playwright', 'playwright-core'];
  const dependencies = new Map();
  for (const key of ['dependencies', 'devDependencies', 'optionalDependencies']) {
    for (const [name, specifier] of Object.entries(manifest[key] ?? {})) {
      if (!supported.includes(name)) continue;
      if (dependencies.has(name) && dependencies.get(name) !== specifier) {
        throw new BrowserCapabilityError('ambiguous-dependency', 'Browser dependency has conflicting declarations.');
      }
      dependencies.set(name, specifier);
    }
  }
  let hasConfig;
  try {
    hasConfig = readdirSync(packageRoot).some(name =>
      /^playwright\.config\.[cm]?[jt]s$/.test(name) && statSync(path.join(packageRoot, name)).isFile());
  } catch {
    throw new BrowserCapabilityError('package-unreadable', 'Cannot inspect browser configuration files.');
  }
  const hasTestScript = Object.keys(manifest.scripts ?? {}).some(name => name === 'test:browser' || name.startsWith('test:browser:'));
  const hasTestRunner = dependencies.has('@playwright/test') || dependencies.has('@vitest/browser-playwright');
  if (!hasConfig && !hasTestScript && !hasTestRunner) {
    return { status: 'not-applicable', reason: 'no-browser-test-declaration' };
  }
  const dependency = supported.find(name => dependencies.has(name));
  if (!dependency) {
    throw new BrowserCapabilityError('browser-dependency-missing', 'Browser tests are declared without a direct supported Playwright dependency.');
  }
  return { status: 'applicable', dependency, specifier: dependencies.get(dependency) };
}

// Lock resolution never executes package configuration or downloads dependencies.
import { existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';

function fail(code, message) { throw new BrowserCapabilityError(code, message); }
const exactVersion = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
function declaration(map, name) {
  return map?.dependencies?.[name] ?? map?.devDependencies?.[name] ?? map?.optionalDependencies?.[name];
}
function nextDependency(name, entry) {
  const next = name === '@playwright/test' ? 'playwright' : name === 'playwright' ? 'playwright-core' :
    (entry?.dependencies?.playwright ?? entry?.peerDependencies?.playwright) ? 'playwright' : 'playwright-core';
  const reference = entry?.dependencies?.[next] ?? entry?.peerDependencies?.[next];
  if (typeof reference !== 'string') fail('invalid-lock-graph', 'Browser dependency graph is incomplete.');
  return [next, reference];
}

export function resolveLockedBrowser(packageRoot) {
  const detected = detectDeclaration(packageRoot);
  if (detected.status === 'not-applicable') return detected;
  const locks = ['package-lock.json', 'pnpm-lock.yaml'].filter(name => existsSync(path.join(packageRoot, name)));
  if (!locks.length) fail('missing-lock', 'Declared browser tests require a package lock.');
  if (locks.length !== 1) fail('ambiguous-lock', 'Choose one package manager lock for browser tests.');
  let raw;
  try { raw = readFileSync(path.join(packageRoot, locks[0]), 'utf8'); }
  catch { fail('invalid-lock', 'Cannot read browser package lock.'); }
  const manager = locks[0] === 'package-lock.json' ? 'npm' : 'pnpm';
  let semver;
  try { semver = createRequire(new URL('./browser/package.json', import.meta.url))('semver'); }
  catch { fail('parser-missing', 'Run npm ci --prefix scripts/browser --ignore-scripts to install locked parsing tools.'); }
  let lock;
  if (manager === 'npm') {
    try { lock = JSON.parse(raw); } catch { fail('invalid-lock', 'Invalid npm package lock.'); }
    if (![2, 3].includes(lock?.lockfileVersion) || !isObject(lock.packages)) {
      fail('unsupported-lock', 'Browser verification requires npm lockfile version 2 or 3.');
    }
    if (declaration(lock.packages[''], detected.dependency) !== detected.specifier) {
      fail('stale-lock', 'Browser declaration differs from the lock importer.');
    }
  } else {
    const manifest = readManifest(packageRoot);
    if (!/^pnpm@\d+\.\d+\.\d+(?:\+sha\d+\.[a-f0-9]+)?$/.test(manifest.packageManager ?? '')) {
      fail('manager-pin-missing', 'PNPM browser packages require an exact packageManager pin.');
    }
    let yaml;
    try { yaml = createRequire(new URL('./browser/package.json', import.meta.url))('yaml'); }
    catch { fail('parser-missing', 'Run npm ci --prefix scripts/browser --ignore-scripts to install the locked YAML parser.'); }
    try {
      const document = yaml.parseDocument(raw, { uniqueKeys: true });
      if (document.errors.length || document.warnings.length) throw new Error();
      lock = document.toJS({ maxAliasCount: 0 });
    } catch { fail('invalid-lock', 'Invalid or unsupported PNPM YAML.'); }
    if (String(lock?.lockfileVersion) !== '9.0' || !isObject(lock.packages) || !isObject(lock.snapshots)) {
      fail('unsupported-lock', 'Browser verification requires PNPM lockfile version 9.0.');
    }
    if (declaration(lock.importers?.['.'], detected.dependency)?.specifier !== detected.specifier) {
      fail('stale-lock', 'Browser declaration differs from the lock importer.');
    }
  }
  let name = detected.dependency;
  let reference = manager === 'pnpm' ? declaration(lock.importers['.'], name).version : detected.specifier;
  let parentKey = '';
  const chain = [];
  for (let depth = 0; depth < 4; depth++) {
    let entry, version, integrity;
    if (manager === 'npm') {
      let prefix = parentKey;
      let key;
      while (true) {
        key = `${prefix ? prefix + '/' : ''}node_modules/${name}`;
        if (lock.packages[key]) break;
        if (!prefix) fail('invalid-lock-graph', 'Browser package is missing from the npm graph.');
        const cut = prefix.lastIndexOf('/node_modules/');
        prefix = cut < 0 ? '' : prefix.slice(0, cut);
      }
      entry = lock.packages[key];
      parentKey = key;
      version = entry.version;
      integrity = entry.integrity;
      if (entry.link || !semver.validRange(reference) || !semver.satisfies(version ?? '', reference)) {
        fail('invalid-lock-graph', 'Browser package graph has an inconsistent dependency edge.');
      }
    } else {
      if (typeof reference !== 'string') fail('invalid-lock-graph', 'Missing browser snapshot reference.');
      version = reference.split('(')[0];
      entry = lock.snapshots[`${name}@${reference}`];
      integrity = lock.packages[`${name}@${version}`]?.resolution?.integrity;
    }
    if (!isObject(entry) || !exactVersion.test(version ?? '') || typeof integrity !== 'string' || !/^sha(256|384|512)-[A-Za-z0-9+/=]+$/.test(integrity)) {
      fail('invalid-lock-graph', 'Browser graph requires exact versions and integrity records.');
    }
    if (depth === 0 && (!semver.validRange(detected.specifier) || !semver.satisfies(version, detected.specifier))) {
      fail('invalid-lock-graph', 'Resolved browser version does not satisfy the declared constraint.');
    }
    chain.push({ name, version });
    if (name === 'playwright-core') {
      return { ...detected, manager, coreVersion: version, chain, lockSha256: createHash('sha256').update(raw).digest('hex') };
    }
    [name, reference] = nextDependency(name, entry);
  }
  fail('invalid-lock-graph', 'Browser dependency graph does not resolve to playwright-core.');
}

export function inspectInstalledBrowser(packageRoot) {
  const locked = resolveLockedBrowser(packageRoot);
  if (locked.status === 'not-applicable') return locked;
  let resolver = createRequire(path.join(path.resolve(packageRoot), 'package.json'));
  let coreRoot;
  for (const [index, expected] of locked.chain.entries()) {
    // A missing direct installation must not accidentally resolve a parent workspace dependency.
    if (index === 0 && !existsSync(path.join(packageRoot, 'node_modules', expected.name, 'package.json'))) {
      fail('package-not-installed', 'Install the declared package using its frozen lock before browser verification.');
    }
    let manifestPath, installed;
    try {
      manifestPath = resolver.resolve(`${expected.name}/package.json`);
      installed = JSON.parse(readFileSync(manifestPath, 'utf8'));
    } catch { fail('package-not-installed', 'A locked browser package is not installed or readable.'); }
    if (installed.name !== expected.name || installed.version !== expected.version) {
      fail('installed-version-mismatch', 'Installed browser packages differ from the locked graph; reinstall with the frozen lock.');
    }
    resolver = createRequire(manifestPath);
    if (expected.name === 'playwright-core') coreRoot = path.dirname(manifestPath);
  }
  let artifacts;
  try { artifacts = JSON.parse(readFileSync(path.join(coreRoot, 'browsers.json'), 'utf8')).browsers; }
  catch { fail('invalid-artifact-manifest', 'Installed Playwright browser artifact manifest is unreadable.'); }
  if (!Array.isArray(artifacts) || !artifacts.length || artifacts.some(item =>
    !isObject(item) || !/^[a-z][a-z0-9-]*$/.test(item.name ?? '') || !/^\d+$/.test(item.revision ?? ''))) {
    fail('invalid-artifact-manifest', 'Installed Playwright browser artifact manifest is invalid.');
  }
  return { ...locked, coreRoot, artifacts };
}

import { spawnSync, execFileSync } from 'node:child_process';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';

export function classifyBrowserFailure(output) {
  const text = String(output);
  if (/Executable doesn't exist|executable.*(?:not found|ENOENT)/i.test(text)) return { code: 'browser-binary-missing' };
  if (/Host system is missing dependencies|error while loading shared libraries|Library not loaded|cannot open shared object file/i.test(text)) return { code: 'system-libraries-missing' };
  const status = text.match(/(?:status(?: code)?|returned code|HTTP\/\S+)[: ]+(\d{3})/i);
  if (status && ['401', '403', '407'].includes(status[1])) {
    let host;
    for (const match of text.matchAll(/https?:\/\/[^\s'"<>]+/g)) {
      try { host = new URL(match[0]).hostname; break; } catch { /* Ignore malformed URL without displaying it. */ }
    }
    return { code: 'download-denied', ...(host ? { host } : {}), httpStatus: Number(status[1]) };
  }
  if (/ECONNREFUSED|ENOTFOUND|ETIMEDOUT|ECONNRESET|CERT_|certificate|proxy/i.test(text)) return { code: 'network-failure' };
  return { code: 'browser-process-failed' };
}


function checkedProcess(args, env, timeout) {
  const result = spawnSync(process.execPath, args, { env, encoding: 'utf8', timeout, maxBuffer: 2 * 1024 * 1024 });
  if (result.status === 0 && !result.error) return;
  const detail = result.error?.code === 'ETIMEDOUT' ? { code: 'browser-process-timeout' } :
    classifyBrowserFailure(`${result.stdout ?? ''}\n${result.stderr ?? ''}`);
  const error = new BrowserCapabilityError(detail.code, 'Browser process failed; no readiness was established.');
  error.detail = detail;
  throw error;
}

export function verifyBrowser(packageRoot, { install = false, browser = 'chromium', cache, onCommand = () => {} } = {}) {
  if (!['chromium', 'firefox', 'webkit'].includes(browser)) fail('unsupported-browser', 'Choose chromium, firefox, or webkit.');
  const installed = inspectInstalledBrowser(packageRoot);
  if (installed.status === 'not-applicable') return installed;
  const cachePath = cache ?? process.env.PLAYWRIGHT_BROWSERS_PATH ?? path.join(homedir(), '.cache', 'coven', 'playwright', `${installed.coreVersion}-${installed.lockSha256.slice(0, 16)}`);
  if (!path.isAbsolute(cachePath)) fail('invalid-cache', 'Browser cache must be an absolute external path.');
  const relativeCache = path.relative(path.resolve(packageRoot), path.resolve(cachePath));
  if (!relativeCache.startsWith('..' + path.sep) && relativeCache !== '..' && !path.isAbsolute(relativeCache)) {
    fail('invalid-cache', 'Browser cache must be outside the package checkout.');
  }
  const artifactName = browser === 'chromium' && installed.artifacts.some(item => item.name === 'chromium-headless-shell') ? 'chromium-headless-shell' : browser;
  const artifact = installed.artifacts.find(item => item.name === artifactName);
  if (!artifact) fail('invalid-artifact-manifest', 'Selected browser is absent from the locked artifact manifest.');
  const env = { ...process.env, PLAYWRIGHT_BROWSERS_PATH: cachePath };
  const run = (args, timeout) => {
    onCommand({ command: [process.execPath, ...args], PLAYWRIGHT_BROWSERS_PATH: cachePath });
    checkedProcess(args, env, timeout);
  };
  if (install) run([path.join(installed.coreRoot, 'cli.js'), 'install', browser], 180000);
  run([fileURLToPath(new URL('./browser-smoke.cjs', import.meta.url)), installed.coreRoot, browser], 45000);
  return { status: 'ready', browser, coreVersion: installed.coreVersion, lockSha256: installed.lockSha256,
    cache: cachePath, artifact, check: 'headless-local-page' };
}

function cli(argv) {
  let install = false, modeSeen = false, browser = 'chromium', cache;
  const packages = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--check' || arg === '--install') {
      if (modeSeen) fail('usage', 'Choose one of --check or --install.');
      modeSeen = true; install = arg === '--install';
    } else if (['--package', '--browser', '--cache'].includes(arg)) {
      const value = argv[++i];
      if (!value || value.startsWith('--')) fail('usage', 'Option requires a value.');
      if (arg === '--package') packages.push(path.resolve(value));
      if (arg === '--browser') browser = value;
      if (arg === '--cache') cache = value;
    } else if (arg === '--help') {
      console.log('Usage: node scripts/browser-capability.mjs [--check|--install] [--package PATH] [--browser chromium|firefox|webkit] [--cache ABSOLUTE_PATH]');
      return;
    } else fail('usage', 'Unknown browser verification option.');
  }
  if (!packages.length) {
    const repo = fileURLToPath(new URL('../', import.meta.url));
    const tracked = execFileSync('git', ['ls-files', '-z'], { cwd: repo, encoding: 'utf8' }).split('\0');
    packages.push(...tracked.filter(file => /^(packages|npm)\/[^/]+\/package\.json$/.test(file)).map(file => path.join(repo, path.dirname(file))));
    if (!packages.length) fail('package-discovery-empty', 'No tracked product package manifests found.');
  }
  for (const packageRoot of packages) {
    try { console.log(JSON.stringify({ package: packageRoot, ...verifyBrowser(packageRoot, { install, browser, cache, onCommand: detail => console.error(JSON.stringify({ status: 'run', ...detail })) }) })); }
    catch (error) {
      console.log(JSON.stringify({ package: packageRoot, status: 'failed', code: error.code ?? 'browser-verification-failed', ...(error.detail ?? {}) }));
      process.exitCode = 1;
    }
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { cli(process.argv.slice(2)); }
  catch (error) {
    console.error(JSON.stringify({ status: 'failed', code: error.code ?? 'browser-verification-failed' }));
    process.exitCode = 1;
  }
}
