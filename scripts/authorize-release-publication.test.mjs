import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { authorizeReleasePublication, releasePackageNames } from './authorize-release-publication.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (relativePath) => readFileSync(path.join(repoRoot, relativePath), 'utf8');
const REPOSITORY = 'OpenCoven/coven';
const TAG = 'v0.4.11';
const VERSION = '0.4.11';
const HEAD = '1'.repeat(40);
const TAG_OBJECT = '2'.repeat(40);
const MAIN = '3'.repeat(40);
const POLICY = read('.github/release-governance.json');
const SIGNATURE = '-----BEGIN SSH SIGNATURE-----\nU1NIU0lH\n-----END SSH SIGNATURE-----\n';

/// The 2026-10-19 snapshot with the reviewed rulesets applied and the
/// repository settings switched on: the state the policy asks for.
function compliantSnapshot() {
  const snapshot = JSON.parse(read('scripts/fixtures/release-governance/snapshot-2026-10-19.json'));
  ['main-protection', 'release-tags-immutable', 'release-tags-creation'].forEach((name, index) => {
    const id = 40000001 + index;
    const body = { id, ...JSON.parse(read(`.github/rulesets/${name}.json`)) };
    snapshot.responses.rulesetPages[0].body.push({ id, name });
    snapshot.responses.rulesets.push({ endpoint: `/rulesets/${id}`, status: 200, body });
  });
  snapshot.responses.classicProtection.body.enforce_admins.enabled = true;
  snapshot.responses.branch.body.protection.required_status_checks.enforcement_level = 'everyone';
  snapshot.responses.immutableReleases.body.enabled = true;
  snapshot.responses.actionsPermissions.body.sha_pinning_required = true;
  return snapshot;
}

function binary(extra = '') {
  return Buffer.from(`\0ELF...COVEN_VERSION_DESC=v${VERSION}\0COMMIT=${HEAD}\0${extra}`);
}

function deps(overrides = {}) {
  const files = {
    '.github/release-governance.json': POLICY,
    'npm/coven/package.json': read('npm/coven/package.json'),
    'npm/coven-platform-template/package.json.tpl': read('npm/coven-platform-template/package.json.tpl'),
    'crates/coven-cli/Cargo.toml': read('crates/coven-cli/Cargo.toml'),
    ...overrides.files
  };
  return {
    verifyTag: async () => ({
      tagObjectSha: TAG_OBJECT,
      headSha: HEAD,
      defaultBranchSha: MAIN,
      signer: { githubLogin: 'BunsDev', fingerprint: 'SHA256:pinned' }
    }),
    resolveSourceAcceptance: async () => ({
      workflow: { runId: '101', runAttempt: 1 },
      requiredChecks: [{ name: 'PR gate', conclusion: 'success' }]
    }),
    captureGovernance: async () => compliantSnapshot(),
    readRepositoryFile: (relativePath) => {
      if (!(relativePath in files)) {
        throw new Error(`unexpected read ${relativePath}`);
      }
      return files[relativePath];
    },
    readDefaultBranchFile: (relativePath, sha) => {
      assert.equal(sha, MAIN);
      assert.equal(relativePath, '.github/release-governance.json');
      return overrides.mainPolicy ?? POLICY;
    },
    loadSupport: () => JSON.parse(read('.github/release-support.json')),
    packageVersionPublished: () => false,
    npmDistTagLatest: () => '0.4.10',
    readTagAnnotation: () => ({ subject: `Coven ${TAG}`, body: `Fixes the release gate.\n\n${SIGNATURE}` }),
    readArtifact: () => binary(),
    runBinaryVersion: () => `coven v${VERSION} (engine coven-code not installed)`,
    environment: () => ({ node: 'v24.22.0', npm: '11.19.1', runnerImage: 'ubuntu24 20261012.1' }),
    now: () => new Date('2026-10-19T00:00:00Z'),
    ...overrides.deps
  };
}

async function authorize(overrides) {
  return authorizeReleasePublication({ repository: REPOSITORY, releaseTag: TAG, headSha: HEAD, tagObjectSha: TAG_OBJECT, deps: deps(overrides) });
}

function control(result, id) {
  const found = result.controls.find((entry) => entry.id === id);
  assert.ok(found, `no control ${id}`);
  return found;
}

test('a release that passes every fresh check is authorized with its evidence', async () => {
  const result = await authorize();
  assert.equal(result.authorized, true, JSON.stringify(result.controls, null, 2));
  assert.deepEqual(result.controls.map((entry) => entry.id), [
    'tag.reverified',
    'tag.unchanged',
    'source.exact-ci',
    'governance.protection',
    'support.release-line',
    'support.surfaces',
    'version.unpublished',
    'version.monotonic',
    'metadata.placeholders',
    'artifacts.source-commit',
    'artifacts.reported-version',
    'metadata.release-notes'
  ]);
  assert.equal(result.kind, 'coven.release-publication-authorization');
  assert.equal(result.npmVersion, VERSION);
  const { taggedCommit, defaultBranch } = result.evidence.governance;
  assert.equal(taggedCommit.compliant, true);
  assert.equal(defaultBranch.compliant, true);
  assert.match(taggedCommit.policySha256, /^[0-9a-f]{64}$/);
  assert.equal(Object.keys(result.evidence.artifacts).length, 4);
  assert.deepEqual(Object.keys(result.evidence.registryLatest).sort(), releasePackageNames().sort());
  assert.equal(result.evidence.releaseNotes.subject, `Coven ${TAG}`);
  assert.equal(result.evidence.releaseNotes.bodyLength, 'Fixes the release gate.'.length);
  assert.equal(result.observedAt, '2026-10-19T00:00:00.000Z');
});

test('releases publish the wrapper and all four native packages', () => {
  assert.deepEqual(releasePackageNames().sort(), [
    '@opencoven/cli',
    '@opencoven/cli-linux-x64',
    '@opencoven/cli-macos',
    '@opencoven/cli-macos-x64',
    '@opencoven/cli-windows'
  ]);
});

const refusals = [
  ['a tag moved after the builds started', 'tag.unchanged', /now names tag object 4{40}/, {
    deps: { verifyTag: async () => ({ tagObjectSha: '4'.repeat(40), headSha: HEAD, defaultBranchSha: MAIN, signer: {} }) }
  }],
  ['a signer revoked during the build', 'tag.reverified', /not signed by a trusted signer/, {
    deps: { verifyTag: async () => { throw new Error('Refusing release: v0.4.11 is not signed by a trusted signer'); } }
  }],
  ['a CI re-run that failed after the preflight', 'source.exact-ci', /must have completed successfully/, {
    deps: { resolveSourceAcceptance: async () => { throw new Error('Refusing release: exact-source CI run 101 must have completed successfully'); } }
  }],
  ['protection that drifted from policy', 'governance.protection', /tagged commit tags\.deletion/, {
    deps: { captureGovernance: async () => JSON.parse(read('scripts/fixtures/release-governance/snapshot-2026-10-19.json')) }
  }],
  ['a stricter policy on main than live protection meets', 'governance.protection', /^main branch\.pull_request: .*required_approving_review_count is 0, policy requires at least 1/, {
    mainPolicy: POLICY.replace('"required_approving_review_count": 0', '"required_approving_review_count": 1')
  }],
  ['an unreadable governance snapshot', 'governance.protection', /No GitHub credential/, {
    deps: { captureGovernance: async () => { throw new Error('No GitHub credential available to read repository protection.'); } }
  }],
  ['a version off the supported line', 'support.release-line', /supports the 0\.3\.x line, not 0\.4\.x/, {
    deps: { loadSupport: () => ({ ...JSON.parse(read('.github/release-support.json')), supportedReleaseLine: '0.3' }) }
  }],
  ['a security-blocked surface', 'support.surfaces', /@opencoven\/cli-windows: \.github\/release-support\.json marks it security-blocked/, {
    deps: {
      loadSupport: () => {
        const support = JSON.parse(read('.github/release-support.json'));
        support.packages['@opencoven/cli-windows'] = { state: 'security-blocked', reason: 'test', issue: 'https://github.com/OpenCoven/coven/issues/805' };
        return support;
      }
    }
  }],
  ['a package already at this version', 'version.unpublished', /already on npm: @opencoven\/cli-macos@0\.4\.11/, {
    deps: { packageVersionPublished: (name) => name === '@opencoven/cli-macos' }
  }],
  ['a registry that cannot say what is published', 'version.unpublished', /could not say whether/, {
    deps: { packageVersionPublished: () => { throw new Error('npm view could not say whether it is already published'); } }
  }],
  ['a version that would move latest backwards', 'version.monotonic', /not newer than @opencoven\/cli latest 0\.4\.12/, {
    deps: { npmDistTagLatest: (name) => (name === '@opencoven/cli' ? '0.4.12' : '0.4.10') }
  }],
  ['a version equal to latest', 'version.monotonic', /not newer than/, {
    deps: { npmDistTagLatest: () => VERSION }
  }],
  ['a hand-bumped wrapper version', 'metadata.placeholders', /npm\/coven\/package\.json version is 0\.4\.11/, {
    files: { 'npm/coven/package.json': read('npm/coven/package.json').replace('"version": "0.0.0"', '"version": "0.4.11"') }
  }],
  ['a hand-bumped crate version', 'metadata.placeholders', /coven-cli\/Cargo\.toml is no longer the 0\.0\.0 placeholder/, {
    files: { 'crates/coven-cli/Cargo.toml': read('crates/coven-cli/Cargo.toml').replace('version = "0.0.0"', 'version = "0.4.11"') }
  }],
  ['a binary built from another commit', 'artifacts.source-commit', /coven\.exe does not embed the tagged commit/, {
    deps: { readArtifact: (relativePath) => (relativePath.endsWith('.exe') ? Buffer.from(`v${VERSION} ${'9'.repeat(40)}`) : binary()) }
  }],
  ['a missing native binary', 'artifacts.source-commit', /x86_64-apple-darwin\/release\/coven is missing/, {
    deps: { readArtifact: (relativePath) => { if (relativePath.includes('x86_64-apple')) throw new Error('ENOENT'); return binary(); } }
  }],
  ['a binary that reports another version', 'artifacts.reported-version', /printed "coven v0\.4\.10 \(engine\)"/, {
    deps: { runBinaryVersion: () => 'coven v0.4.10 (engine)' }
  }],
  ['a version prefix that only looks right', 'artifacts.reported-version', /expected coven v0\.4\.11/, {
    deps: { runBinaryVersion: () => 'coven v0.4.110' }
  }],
  ['an annotation titled for another version', 'metadata.release-notes', /titled "Coven v0\.4\.10"/, {
    deps: { readTagAnnotation: () => ({ subject: 'Coven v0.4.10', body: 'notes' }) }
  }],
  ['an annotation with no notes, like v0.4.10', 'metadata.release-notes', /has no release notes/, {
    deps: { readTagAnnotation: () => ({ subject: `Coven ${TAG}`, body: SIGNATURE }) }
  }]
];

for (const [label, id, detail, overrides] of refusals) {
  test(`refuses ${label}`, async () => {
    const result = await authorize(overrides);
    assert.equal(result.authorized, false);
    const found = control(result, id);
    assert.equal(found.status, 'fail');
    assert.match(found.detail, detail);
  });
}

test('one refusal does not hide the others', async () => {
  const result = await authorize({
    deps: {
      npmDistTagLatest: () => '0.5.0',
      readTagAnnotation: () => ({ subject: 'wrong', body: '' })
    }
  });
  assert.deepEqual(
    result.controls.filter((entry) => entry.status === 'fail').map((entry) => entry.id),
    ['version.monotonic', 'metadata.release-notes']
  );
});

test('the version comes from a stable release tag', async () => {
  await assert.rejects(
    authorizeReleasePublication({ repository: REPOSITORY, releaseTag: 'v0.4.11-rc.1', headSha: HEAD, tagObjectSha: TAG_OBJECT, deps: deps() }),
    /must be a stable vX\.Y\.Z tag/
  );
});
