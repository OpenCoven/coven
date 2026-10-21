import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const moduleUrl = new URL('./browser-capability.mjs', import.meta.url);

function fixture(t, manifest, files = {}) {
  const root = mkdtempSync(path.join(tmpdir(), 'coven-browser-contract-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(path.join(root, 'package.json'), JSON.stringify(manifest));
  for (const [name, content] of Object.entries(files)) writeFileSync(path.join(root, name), content);
  return root;
}

async function detector() {
  const { existsSync } = await import('node:fs');
  assert.ok(existsSync(moduleUrl), 'browser capability detector is not implemented');
  return (await import(moduleUrl)).detectDeclaration;
}

test('no browser declaration needs no lockfile or installed packages', async (t) => {
  const detect = await detector();
  const root = fixture(t, { scripts: { test: 'node --test' } });
  assert.deepEqual(detect(root), { status: 'not-applicable', reason: 'no-browser-test-declaration' });
});

test('transitive Playwright and optional lock peers do not activate browser tests', async (t) => {
  const detect = await detector();
  const root = fixture(t, { devDependencies: { vitest: '^5.0.5' } }, {
    'pnpm-lock.yaml': "packages:\n  playwright-core@1.63.0: {}\n  vitest@5.0.5:\n    peerDependencies:\n      '@vitest/browser-playwright': 5.0.5\n",
  });
  assert.equal(detect(root).status, 'not-applicable');
});

test('a direct Playwright test runner is an explicit declaration', async (t) => {
  const detect = await detector();
  const root = fixture(t, { devDependencies: { '@playwright/test': '^1.63.0' } });
  assert.deepEqual(detect(root), { status: 'applicable', dependency: '@playwright/test', specifier: '^1.63.0' });
});

test('direct Playwright automation without browser tests remains optional', async (t) => {
  const detect = await detector();
  const root = fixture(t, { dependencies: { playwright: '1.63.0' }, scripts: { scrape: 'node scrape.mjs' } });
  assert.equal(detect(root).status, 'not-applicable');
});

test('explicit browser test script activates its direct core dependency', async (t) => {
  const detect = await detector();
  const root = fixture(t, { devDependencies: { 'playwright-core': '1.63.0' }, scripts: { 'test:browser': 'node browser-test.mjs' } });
  assert.equal(detect(root).dependency, 'playwright-core');
});

test('configuration without a provider fails instead of claiming not applicable', async (t) => {
  const detect = await detector();
  const root = fixture(t, {}, { 'playwright.config.ts': 'export default {};' });
  assert.throws(() => detect(root), { code: 'browser-dependency-missing' });
});

test('declared Vitest Playwright provider activates capability', async (t) => {
  const detect = await detector();
  const root = fixture(t, { devDependencies: { '@vitest/browser-playwright': '5.0.5' } });
  assert.equal(detect(root).dependency, '@vitest/browser-playwright');
});

test('conflicting dependency declarations fail closed', async (t) => {
  const detect = await detector();
  const root = fixture(t, { dependencies: { '@playwright/test': '1.62.0' }, devDependencies: { '@playwright/test': '1.63.0' } });
  assert.throws(() => detect(root), { code: 'ambiguous-dependency' });
});

test('malformed manifest errors do not echo input contents', async (t) => {
  const detect = await detector();
  const root = fixture(t, {});
  const sentinel = 'do-not-echo-malformed-input';
  writeFileSync(path.join(root, 'package.json'), '{' + sentinel);
  assert.throws(() => detect(root), (error) => error.code === 'invalid-manifest' && !error.message.includes(sentinel));
});

test('non-object manifests and dependency maps are rejected', async (t) => {
  const detect = await detector();
  for (const manifest of [null, [], { devDependencies: [] }, { scripts: 'test:browser' }]) {
    const root = fixture(t, manifest);
    assert.throws(() => detect(root), { code: 'invalid-manifest' });
  }
});

async function lockedFixture(t, kind) {
  const { cpSync } = await import('node:fs');
  const root = fixture(t, {});
  cpSync(new URL(`./fixtures/browser-${kind}/`, import.meta.url), root, { recursive: true, filter: source => !source.split(path.sep).includes('node_modules') });
  return root;
}

for (const kind of ['npm', 'pnpm']) {
  test(`resolves exact browser core from real ${kind} lock graph`, async t => {
    const { resolveLockedBrowser } = await import(moduleUrl);
    const result = resolveLockedBrowser(await lockedFixture(t, kind));
    assert.equal(result.status, 'applicable');
    assert.equal(result.coreVersion, '1.63.0');
    assert.equal(result.manager, kind);
    assert.match(result.lockSha256, /^[a-f0-9]{64}$/);
  });
  test(`rejects stale ${kind} root declaration`, async t => {
    const { resolveLockedBrowser } = await import(moduleUrl);
    const root = await lockedFixture(t, kind);
    writeFileSync(path.join(root, 'package.json'), JSON.stringify({ devDependencies: { '@playwright/test': '1.62.0' }, packageManager: 'pnpm@10.11.1' }));
    assert.throws(() => resolveLockedBrowser(root), { code: 'stale-lock' });
  });
}

test('absent browser declaration bypasses lock and parser requirements', async t => {
  const { resolveLockedBrowser } = await import(moduleUrl);
  assert.equal(resolveLockedBrowser(fixture(t, {})).status, 'not-applicable');
});

test('declared browser fails on missing or competing locks', async t => {
  const { resolveLockedBrowser } = await import(moduleUrl);
  const root = fixture(t, { devDependencies: { '@playwright/test': '1.63.0' } });
  assert.throws(() => resolveLockedBrowser(root), { code: 'missing-lock' });
  writeFileSync(path.join(root, 'package-lock.json'), '{}');
  writeFileSync(path.join(root, 'pnpm-lock.yaml'), '{}');
  assert.throws(() => resolveLockedBrowser(root), { code: 'ambiguous-lock' });
});

test('missing npm graph edge fails closed', async t => {
  const { readFileSync } = await import('node:fs');
  const { resolveLockedBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'npm');
  const lock = JSON.parse(readFileSync(path.join(root, 'package-lock.json')));
  delete lock.packages['node_modules/playwright-core'];
  writeFileSync(path.join(root, 'package-lock.json'), JSON.stringify(lock));
  assert.throws(() => resolveLockedBrowser(root), { code: 'invalid-lock-graph' });
});

test('duplicate PNPM YAML keys fail without echoing input', async t => {
  const { resolveLockedBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'pnpm');
  writeFileSync(path.join(root, 'pnpm-lock.yaml'), 'private-sentinel: one\nprivate-sentinel: two\n');
  assert.throws(() => resolveLockedBrowser(root), e => e.code === 'invalid-lock' && !e.message.includes('private-sentinel'));
});

test('PNPM requires an exact manager pin and a complete core snapshot', async t => {
  const { readFileSync } = await import('node:fs');
  const { resolveLockedBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'pnpm');
  const manifestPath = path.join(root, 'package.json');
  const manifest = JSON.parse(readFileSync(manifestPath));
  delete manifest.packageManager;
  writeFileSync(manifestPath, JSON.stringify(manifest));
  assert.throws(() => resolveLockedBrowser(root), { code: 'manager-pin-missing' });
  manifest.packageManager = 'pnpm@10.11.1';
  writeFileSync(manifestPath, JSON.stringify(manifest));
  const lockPath = path.join(root, 'pnpm-lock.yaml');
  writeFileSync(lockPath, readFileSync(lockPath, 'utf8').replace('  playwright-core@1.63.0: {}', ''));
  assert.throws(() => resolveLockedBrowser(root), { code: 'invalid-lock-graph' });
});

async function installMetadata(root, coreVersion = '1.63.0') {
  const { mkdirSync } = await import('node:fs');
  for (const name of ['@playwright/test', 'playwright', 'playwright-core']) {
    const dir = path.join(root, 'node_modules', name);
    mkdirSync(dir, { recursive: true });
    writeFileSync(path.join(dir, 'package.json'), JSON.stringify({ name, version: name === 'playwright-core' ? coreVersion : '1.63.0' }));
  }
  writeFileSync(path.join(root, 'node_modules/playwright-core/browsers.json'), JSON.stringify({ browsers: [{ name: 'chromium', revision: '1234', browserVersion: 'test-only', installByDefault: true }] }));
}

test('installed package metadata must match every locked browser package', async t => {
  const { inspectInstalledBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'npm');
  assert.throws(() => inspectInstalledBrowser(root), { code: 'package-not-installed' });
  await installMetadata(root, '1.62.0');
  assert.throws(() => inspectInstalledBrowser(root), { code: 'installed-version-mismatch' });
  await installMetadata(root);
  const result = inspectInstalledBrowser(root);
  assert.equal(result.coreVersion, '1.63.0');
  assert.equal(result.artifacts[0].revision, '1234');
  assert.equal(result.artifacts[0].browserVersion, 'test-only');
});

test('sanitized browser failures distinguish missing binaries, libraries and denied downloads', async () => {
  const { classifyBrowserFailure } = await import(moduleUrl);
  assert.equal(classifyBrowserFailure("Executable doesn't exist at /private/path").code, 'browser-binary-missing');
  assert.equal(classifyBrowserFailure('error while loading shared libraries: libexample.so').code, 'system-libraries-missing');
  const denied = classifyBrowserFailure('Error: Download failed: server returned code 403 body private-value. URL: https://user:password@cdn.example.test/private/path?credential=private-value');
  assert.deepEqual(denied, { code: 'download-denied', host: 'cdn.example.test', httpStatus: 403 });
  assert.equal(classifyBrowserFailure('connect ECONNREFUSED proxy.example.test').code, 'network-failure');
  assert.deepEqual(classifyBrowserFailure('unknown private-value'), { code: 'browser-process-failed' });
});

test('CLI reports not-applicable without requiring a lock or downloading', async t => {
  const { spawnSync } = await import('node:child_process');
  const { fileURLToPath } = await import('node:url');
  const root = fixture(t, {});
  const result = spawnSync(process.execPath, [fileURLToPath(moduleUrl), '--check', '--package', root], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout.trim()).status, 'not-applicable');
});

test('readiness classifies simulated missing OS libraries and closes a launched browser', async t => {
  const { readFileSync } = await import('node:fs');
  const { verifyBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'npm');
  await installMetadata(root);
  const entry = path.join(root, 'node_modules/playwright-core/index.js');
  writeFileSync(entry, "exports.chromium = { launch: async () => { throw new Error('Host system is missing dependencies'); } };\n");
  assert.throws(() => verifyBrowser(root, { cache: path.join(tmpdir(), 'coven-mock-browser-cache') }), { code: 'system-libraries-missing' });
  const closed = path.join(root, 'closed');
  writeFileSync(entry, `exports.chromium = { launch: async () => ({ newPage: async () => ({setContent:async()=>{}, title:async()=> 'Coven browser readiness'}), close: async () => require('node:fs').writeFileSync(${JSON.stringify(closed)}, 'closed') }) };`);
  assert.equal(verifyBrowser(root, { cache: path.join(tmpdir(), 'coven-mock-browser-cache') }).status, 'ready');
  assert.equal(readFileSync(closed, 'utf8'), 'closed');
});

test('simulated denied installer is nonzero and exposes only host and HTTP status', async t => {
  const { verifyBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'npm');
  await installMetadata(root);
  writeFileSync(path.join(root, 'node_modules/playwright-core/cli.js'), "console.error('Download failed: server returned code 403. URL: https://user:password@cdn.example.test/path?diagnostic=private-sentinel'); process.exitCode=1;");
  assert.throws(() => verifyBrowser(root, { install: true, cache: path.join(tmpdir(), 'coven-mock-browser-cache') }), error =>
    error.code === 'download-denied' && error.detail.host === 'cdn.example.test' && error.detail.httpStatus === 403 && !JSON.stringify(error).includes('private-sentinel'));
});

test('real npm Vitest provider resolves its Playwright peer', async t => {
  const { resolveLockedBrowser } = await import(moduleUrl);
  const result = resolveLockedBrowser(await lockedFixture(t, 'vitest-npm'));
  assert.equal(result.coreVersion, '1.63.0');
  assert.equal(result.chain[0].name, '@vitest/browser-playwright');
});

test('npm root and transitive constraints must match resolved versions', async t => {
  const { readFileSync } = await import('node:fs');
  const { resolveLockedBrowser } = await import(moduleUrl);
  for (const constraint of ['1.62.0', '^2.0.0', 'file:../local']) {
    const root = await lockedFixture(t, 'npm');
    const manifestPath = path.join(root, 'package.json');
    const lockPath = path.join(root, 'package-lock.json');
    const manifest = JSON.parse(readFileSync(manifestPath));
    const lock = JSON.parse(readFileSync(lockPath));
    manifest.devDependencies['@playwright/test'] = constraint;
    lock.packages[''].devDependencies['@playwright/test'] = constraint;
    writeFileSync(manifestPath, JSON.stringify(manifest));
    writeFileSync(lockPath, JSON.stringify(lock));
    assert.throws(() => resolveLockedBrowser(root), { code: 'invalid-lock-graph' });
  }
});

test('real PNPM Vitest provider resolves peer-qualified snapshots', async t => {
  const { resolveLockedBrowser } = await import(moduleUrl);
  const result = resolveLockedBrowser(await lockedFixture(t, 'vitest-pnpm'));
  assert.equal(result.coreVersion, '1.63.0');
  assert.equal(result.chain[0].name, '@vitest/browser-playwright');
});

test('npm transitive range constraints reject an inconsistent graph', async t => {
  const { readFileSync } = await import('node:fs');
  const { resolveLockedBrowser } = await import(moduleUrl);
  const root = await lockedFixture(t, 'npm');
  const lockPath = path.join(root, 'package-lock.json');
  const lock = JSON.parse(readFileSync(lockPath));
  lock.packages['node_modules/playwright'].dependencies['playwright-core'] = '^2.0.0';
  writeFileSync(lockPath, JSON.stringify(lock));
  assert.throws(() => resolveLockedBrowser(root), { code: 'invalid-lock-graph' });
});
