import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { publishEnv, releaseVersion, targetPackageName, validatePublishToken, validatePublishVersion } from './publish-npm.mjs';

test('releaseVersion prefers explicit COVEN_NPM_VERSION and strips a leading v', () => {
  assert.equal(
    releaseVersion({ COVEN_NPM_VERSION: 'v1.2.3', GITHUB_REF_NAME: 'v9.9.9' }, '0.0.0'),
    '1.2.3'
  );
});

test('releaseVersion falls back to tag ref for tag-triggered dry runs', () => {
  assert.equal(releaseVersion({ GITHUB_REF_NAME: 'v2.0.1' }, '0.0.0'), '2.0.1');
});

test('releaseVersion falls back to package placeholder for local dry runs', () => {
  assert.equal(releaseVersion({}, '0.0.0'), '0.0.0');
});

test('validatePublishVersion rejects real publish with placeholder version', () => {
  assert.throws(() => validatePublishVersion('0.0.0', false), /Refusing real npm publish/);
});

test('validatePublishVersion allows dry-run with placeholder version', () => {
  assert.doesNotThrow(() => validatePublishVersion('0.0.0', true));
});

test('validatePublishVersion allows real publish with explicit release version', () => {
  assert.doesNotThrow(() => validatePublishVersion('1.2.3', false));
});

test('macOS target publishes under human-facing native package name', () => {
  assert.equal(targetPackageName('macos'), '@opencoven/cli-macos');
});

test('linux x64 target publishes under linux native package name', () => {
  assert.equal(targetPackageName('linux-x64'), '@opencoven/cli-linux-x64');
});

test('wrapper declares linux x64 native package as an optional dependency', () => {
  const packagePath = new URL(['..', 'npm', 'coven', 'package.json'].join('/'), import.meta.url);
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  assert.equal(packageJson.optionalDependencies['@opencoven/cli-linux-x64'], '0.0.0');
});

test('wrapper binary maps linux x64 to linux native package and documents glibc requirement', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /'linux-x64': '@opencoven\/cli-linux-x64'/);
  assert.match(bin, /glibc-based Linux x64/);
});

test('release workflow builds and dry-runs linux x64 package', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /npm-target: linux-x64/);
  assert.match(workflow, /rust-target: x86_64-unknown-linux-gnu/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=linux-x64 --skip-build --dry-run --skip-wrapper/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=linux-x64 --skip-build --publish --skip-wrapper/);
});

test('publishEnv preserves setup-node NODE_AUTH_TOKEN when NPM_TOKEN is absent', () => {
  assert.equal(publishEnv(false, { NODE_AUTH_TOKEN: 'from-setup-node', NPM_TOKEN: '' }).NODE_AUTH_TOKEN, 'from-setup-node');
});

test('publishEnv prefers explicit NPM_TOKEN when present', () => {
  assert.equal(publishEnv(false, { NODE_AUTH_TOKEN: 'from-setup-node', NPM_TOKEN: 'from-secret' }).NODE_AUTH_TOKEN, 'from-secret');
});

test('validatePublishToken allows real publish when only NODE_AUTH_TOKEN is set', () => {
  assert.doesNotThrow(() => validatePublishToken({ NODE_AUTH_TOKEN: 'from-setup-node' }, false));
});

test('validatePublishToken allows real publish when only NPM_TOKEN is set', () => {
  assert.doesNotThrow(() => validatePublishToken({ NPM_TOKEN: 'from-secret' }, false));
});

test('validatePublishToken rejects real publish when neither token is set', () => {
  assert.throws(() => validatePublishToken({}, false), /Refusing real npm publish without NPM_TOKEN or NODE_AUTH_TOKEN/);
});

test('validatePublishToken allows dry-run when no tokens are set', () => {
  assert.doesNotThrow(() => validatePublishToken({}, true));
});

test('release workflow passes the configured npm publish secret to the publish script', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  const configuredSecret = ['NPM', 'ACCESS', 'TOKEN'].join('_');
  const expectedLine = ['NPM_TOKEN:', '${{', `secrets.${configuredSecret}`, '}}'].join(' ');
  assert.ok(workflow.includes(expectedLine));
});
