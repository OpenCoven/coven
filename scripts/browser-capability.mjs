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
    entry?.dependencies?.playwright ? 'playwright' : 'playwright-core';
  const reference = entry?.dependencies?.[next];
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
  let reference = manager === 'pnpm' ? declaration(lock.importers['.'], name).version : null;
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
      if (entry.link || (reference && exactVersion.test(reference) && reference !== version)) {
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
    chain.push({ name, version });
    if (name === 'playwright-core') {
      return { ...detected, manager, coreVersion: version, chain, lockSha256: createHash('sha256').update(raw).digest('hex') };
    }
    [name, reference] = nextDependency(name, entry);
  }
  fail('invalid-lock-graph', 'Browser dependency graph does not resolve to playwright-core.');
}
