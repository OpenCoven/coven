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
