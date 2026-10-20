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
