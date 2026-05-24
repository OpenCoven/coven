import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { defaultTargetName, isOidcContext, publishArgs, publishEnv, releaseVersion, targetPackageName, validatePublishToken, validatePublishVersion } from './publish-npm.mjs';

const OIDC_ENV = {
  ACTIONS_ID_TOKEN_REQUEST_TOKEN: 'fake-oidc-token',
  ACTIONS_ID_TOKEN_REQUEST_URL: 'https://token.actions.githubusercontent.com/'
};

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

test('windows target publishes under windows native package name', () => {
  assert.equal(targetPackageName('windows'), '@opencoven/cli-windows');
});

test('defaultTargetName maps win32 x64 to windows', () => {
  assert.equal(defaultTargetName('win32', 'x64'), 'windows');
});

test('wrapper declares linux x64 native package as an optional dependency', () => {
  const packagePath = new URL(['..', 'npm', 'coven', 'package.json'].join('/'), import.meta.url);
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  assert.equal(packageJson.optionalDependencies['@opencoven/cli-linux-x64'], '0.0.0');
});

test('wrapper declares windows native package as an optional dependency', () => {
  const packagePath = new URL(['..', 'npm', 'coven', 'package.json'].join('/'), import.meta.url);
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  assert.equal(packageJson.optionalDependencies['@opencoven/cli-windows'], '0.0.0');
});

test('wrapper binary maps linux x64 to linux native package and documents glibc requirement', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /'linux-x64': '@opencoven\/cli-linux-x64'/);
  assert.match(bin, /glibc-based Linux x64/);
});

test('wrapper binary maps windows x64 to windows native package and exe binary', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /'win32-x64': '@opencoven\/cli-windows'/);
  assert.match(bin, /process\.platform === 'win32' \? 'coven\.exe' : 'coven'/);
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

test('release workflow builds and dry-runs windows package', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /npm-target: windows/);
  assert.match(workflow, /rust-target: x86_64-pc-windows-msvc/);
  assert.match(workflow, /runner: windows-latest/);
  assert.match(workflow, /binary: coven\.exe/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=windows --skip-build --dry-run --skip-wrapper/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=windows --skip-build --publish --skip-wrapper/);
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

test('validatePublishToken rejects real publish when neither token nor OIDC is available', () => {
  assert.throws(() => validatePublishToken({}, false), /OIDC trusted publishing/);
});

test('validatePublishToken allows dry-run when no tokens are set', () => {
  assert.doesNotThrow(() => validatePublishToken({}, true));
});

test('isOidcContext requires both OIDC env vars to be present', () => {
  assert.equal(isOidcContext(OIDC_ENV), true);
  assert.equal(isOidcContext({ ACTIONS_ID_TOKEN_REQUEST_TOKEN: 'only-token' }), false);
  assert.equal(isOidcContext({ ACTIONS_ID_TOKEN_REQUEST_URL: 'only-url' }), false);
  assert.equal(isOidcContext({}), false);
});

test('validatePublishToken accepts OIDC context without any npm token', () => {
  assert.doesNotThrow(() => validatePublishToken(OIDC_ENV, false));
});

test('publishEnv leaves OIDC context untouched and does not smuggle in NODE_AUTH_TOKEN', () => {
  const result = publishEnv(false, { ...OIDC_ENV, NPM_TOKEN: 'should-not-be-used' });
  assert.equal(result.NODE_AUTH_TOKEN, undefined, 'OIDC publish must not inherit NODE_AUTH_TOKEN');
  assert.equal(result.NPM_TOKEN, 'should-not-be-used', 'unrelated env keys must pass through unchanged');
});

test('publishArgs adds --provenance and --access public only when OIDC is active for real publish', () => {
  assert.deepEqual(publishArgs(false, OIDC_ENV), ['publish', '--access', 'public', '--provenance']);
  assert.deepEqual(publishArgs(false, {}), ['publish', '--access', 'public']);
  assert.deepEqual(publishArgs(true, OIDC_ENV), ['publish', '--dry-run']);
  assert.deepEqual(publishArgs(true, {}), ['publish', '--dry-run']);
});

test('release workflow uses OIDC trusted publishing instead of a long-lived npm token', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(
    workflow,
    /npm-publish:[\s\S]*?permissions:[\s\S]*?id-token: write/,
    'npm-publish job must request id-token: write so npm publish --provenance can use OIDC'
  );
  // Build the legacy token reference at runtime so this test file itself does not
  // contain a literal NPM_TOKEN line that could be mistaken for a check that
  // expects the old behaviour.
  const legacyTokenRef = ['NPM', '_', 'TOKEN', ':'].join('');
  assert.equal(
    workflow.includes(legacyTokenRef),
    false,
    'release workflow must not reference NPM_TOKEN once OIDC trusted publishing is configured'
  );
  const legacySecretRef = ['secrets.', 'NPM', '_', 'ACCESS', '_', 'TOKEN'].join('');
  assert.equal(
    workflow.includes(legacySecretRef),
    false,
    'release workflow must not read the legacy NPM_ACCESS_TOKEN secret under OIDC'
  );
});

test('release workflow verifies the signed release tag before building or publishing', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /verify-tag:/, 'workflow must declare a verify-tag job');
  assert.match(
    workflow,
    /build-platform:[\s\S]*?needs:[\s\S]*?verify-tag/,
    'build-platform must depend on verify-tag so unsigned tags never reach the build matrix'
  );
  assert.match(
    workflow,
    /\.verification\.verified/,
    'verify-tag must consult GitHub`s signature verification API'
  );
  assert.match(
    workflow,
    /lightweight tag/,
    'verify-tag must explicitly reject lightweight (unsigned) tags'
  );
  assert.match(
    workflow,
    /git merge-base --is-ancestor .* origin\/main/,
    'verify-tag must ensure the tagged commit is contained in origin/main'
  );
  assert.match(
    workflow,
    /vars\.NPM_RELEASE_ALLOWED_SIGNERS/,
    'verify-tag must require an explicit SSH allowed-signers allowlist via NPM_RELEASE_ALLOWED_SIGNERS'
  );
  assert.match(
    workflow,
    /git verify-tag "\$TAG_NAME"/,
    'verify-tag must locally verify the tag against the SSH allowed signers file'
  );
  assert.match(
    workflow,
    /empty line/,
    'verify-tag must reject empty allowed signer entries before authorization checks'
  );
  assert.match(
    workflow,
    /gpg\.ssh\.allowedSignersFile/,
    'verify-tag must configure git with the release SSH allowed signers file'
  );
  assert.doesNotMatch(
    workflow,
    /tagger_email_lc|tagger_email=\$\(jq -r '\.tagger\.email|NPM_RELEASE_SIGNERS/,
    'verify-tag must not authorize releases by mutable tagger email allowlist'
  );
  assert.doesNotMatch(
    workflow,
    /\.signature\s*\{/,
    'verify-tag must not query the removed GitHub GraphQL Tag.signature field'
  );
  assert.match(
    workflow,
    /tag_object_type=\$\(jq -r '\.object\.type/,
    'verify-tag must inspect the annotated tag target type and SHA'
  );
  assert.match(
    workflow,
    /tag_object_type.*commit/s,
    'verify-tag must reject annotated tags that do not target commits'
  );
});

test('release workflow triggers only on signed v* tag pushes (no workflow_dispatch fallback)', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /on:\s*\n\s*push:\s*\n\s*tags:\s*\n\s*- 'v\*'/);
  assert.equal(
    /workflow_dispatch:/.test(workflow),
    false,
    'workflow_dispatch trigger must be removed so manual unsigned publishes are impossible'
  );
});

test('release workflow pins all third-party actions to immutable commit SHAs', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  const usesLines = workflow.split('\n').filter((line) => /^\s*-\s*uses:\s/.test(line));
  assert.ok(usesLines.length > 0, 'expected at least one `uses:` line in the release workflow');
  for (const line of usesLines) {
    const match = line.match(/uses:\s*([^@]+)@([^\s#]+)/);
    assert.ok(match, `could not parse uses line: ${line}`);
    const ref = match[2];
    assert.match(
      ref,
      /^[0-9a-f]{40}$/,
      `action ${match[1]} must be pinned to a 40-char commit SHA, found "${ref}" on line: ${line}`
    );
  }
});

test('release workflow concurrency keeps overlapping releases from interleaving', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /^concurrency:\s*\n\s*group:\s*release-npm/m);
  assert.match(workflow, /cancel-in-progress:\s*false/);
});

test('prepublish smoke has explicit dry-run version override and registry failure message', () => {
  const scriptPath = new URL('test-cli-prepublish.mjs', import.meta.url);
  const script = readFileSync(scriptPath, 'utf8');

  assert.match(script, /COVEN_NPM_DRY_RUN_VERSION/);
  assert.match(script, /Could not read current \$\{packageName\} version/);
});
