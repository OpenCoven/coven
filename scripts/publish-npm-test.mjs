import assert from 'node:assert/strict';
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
