import assert from 'node:assert/strict';
import test from 'node:test';

import { releaseVersion, validatePublishVersion } from './publish-npm.mjs';

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
