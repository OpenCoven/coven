import assert from 'node:assert/strict';
import test from 'node:test';

import {
  checkPackageListing,
  checkPackageMetadata,
  FORBIDDEN_DEPENDENCIES,
  REQUIRED_PACKAGE_FILES
} from './verify-coven-client-package.mjs';

function goodPackage(overrides = {}) {
  return {
    name: 'opencoven-coven-client',
    version: '0.1.0',
    license: 'MIT',
    repository: 'https://github.com/OpenCoven/coven',
    readme: 'README.md',
    description: 'Client for coven.daemon.v1.',
    targets: [{ kind: ['lib'], name: 'coven_client' }],
    dependencies: [{ name: 'serde' }, { name: 'serde_json' }],
    ...overrides
  };
}

test('a correctly described package passes metadata checks', () => {
  assert.deepEqual(checkPackageMetadata(goodPackage()), []);
});

test('a missing package is a single failure', () => {
  assert.equal(checkPackageMetadata(undefined).length, 1);
});

test('identity drift is reported per field', () => {
  const failures = checkPackageMetadata(
    goodPackage({ name: 'coven-client', version: '1.0.0', license: 'Apache-2.0', readme: undefined })
  );
  assert.match(failures.join('\n'), /package name is coven-client/);
  assert.match(failures.join('\n'), /version 1\.0\.0 is not a pre-1\.0/);
  assert.match(failures.join('\n'), /license is Apache-2\.0/);
  assert.match(failures.join('\n'), /readme is unset/);
});

test('the library must keep the coven_client name', () => {
  const failures = checkPackageMetadata(goodPackage({ targets: [{ kind: ['lib'], name: 'opencoven_coven_client' }] }));
  assert.deepEqual(failures, ['lib target is opencoven_coven_client, expected coven_client']);
});

test('every forbidden dependency is refused', () => {
  for (const forbidden of FORBIDDEN_DEPENDENCIES) {
    const failures = checkPackageMetadata(goodPackage({ dependencies: [{ name: forbidden }] }));
    assert.deepEqual(failures, [`forbidden dependency ${forbidden}`]);
  }
});

test('the tarball listing must contain README, fixtures, and the contract test', () => {
  assert.deepEqual(checkPackageListing([...REQUIRED_PACKAGE_FILES, 'Cargo.lock', 'Cargo.toml.orig']), []);
  const failures = checkPackageListing(['Cargo.toml', 'src/lib.rs']);
  assert.ok(failures.includes('tarball is missing fixtures/health.json'));
  assert.ok(failures.includes('tarball is missing README.md'));
  assert.ok(failures.includes('tarball is missing tests/package_contract.rs'));
});
