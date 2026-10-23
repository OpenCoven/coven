import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';

import { buildReleaseReceipt, npmChannelState, reconcileRelease } from './release-receipt.mjs';

const REPOSITORY = 'OpenCoven/coven';
const TAG = 'v0.4.11';
const VERSION = '0.4.11';
const HEAD = '1'.repeat(40);
const TAG_OBJECT = '2'.repeat(40);
const PACKAGES = ['@opencoven/cli', '@opencoven/cli-linux-x64', '@opencoven/cli-macos', '@opencoven/cli-macos-x64', '@opencoven/cli-windows'];
const SBOM_NAME = `coven-v${VERSION}-sbom.cdx.json`;
const RECEIPT_NAME = `coven-v${VERSION}-release-receipt.json`;
const OBSERVED_AT = '2026-10-19T00:00:00.000Z';
const NOTES = 'Fixes the release gate.\n\nNo migration needed.';

const passing = (ids) => ids.map((id) => ({ id, status: 'pass', detail: 'matches' }));

function inputs(overrides = {}) {
  return {
    repository: REPOSITORY,
    releaseTag: TAG,
    trigger: 'workflow_run',
    sourceRun: { sourceRunId: '5001', sourceRunAttempt: 1, headSha: HEAD },
    tagVerification: {
      schemaVersion: 1,
      kind: 'coven.release-tag-verification',
      repository: REPOSITORY,
      releaseTag: TAG,
      tagObjectSha: TAG_OBJECT,
      headSha: HEAD,
      defaultBranch: 'main',
      defaultBranchSha: '3'.repeat(40),
      github: { verified: true, reason: 'valid' },
      signer: { githubLogin: 'BunsDev', fingerprint: 'SHA256:pinned' },
      signerPins: { path: '.github/release-signers.json', taggedCommitSha256: 'a'.repeat(64), defaultBranchSha256: 'a'.repeat(64) }
    },
    sourceAcceptance: {
      schemaVersion: 1,
      kind: 'coven.release-source-acceptance',
      releaseTag: TAG,
      tagObjectSha: TAG_OBJECT,
      candidateSha: HEAD,
      workflow: { runId: '4001', runAttempt: 2 },
      requiredChecks: [{ name: 'PR gate', conclusion: 'success' }],
      observedAt: OBSERVED_AT
    },
    publicationAuthorization: {
      schemaVersion: 1,
      kind: 'coven.release-publication-authorization',
      releaseTag: TAG,
      headSha: HEAD,
      tagObjectSha: TAG_OBJECT,
      authorized: true,
      controls: passing(['tag.reverified', 'metadata.placeholders', 'artifacts.source-commit', 'artifacts.reported-version', 'metadata.release-notes']),
      evidence: {
        sourceAcceptance: { workflow: { runId: '4001', runAttempt: 2 }, requiredChecks: [{ name: 'PR gate', conclusion: 'success' }], observedAt: OBSERVED_AT },
        governance: { taggedCommit: { policySha256: 'b'.repeat(64), snapshotSha256: 'c'.repeat(64), compliant: true, controls: passing(['branch.pull_request']) } },
        environment: { node: 'v24.22.0', npm: '11.19.1', runnerImage: 'ubuntu24 1' },
        releaseNotes: { subject: `Coven ${TAG}`, bodySha256: createHash('sha256').update(NOTES).digest('hex'), bodyLength: NOTES.length }
      },
      observedAt: OBSERVED_AT
    },
    publicationAuthorizationSha256: 'e'.repeat(64),
    governanceEvaluation: {
      schemaVersion: 1,
      kind: 'coven.release-governance-evaluation',
      policySha256: 'b'.repeat(64),
      snapshotSha256: 'f'.repeat(64),
      snapshotCapturedAt: OBSERVED_AT,
      reader: { credentialSource: 'RELEASE_GOVERNANCE_READ_TOKEN' },
      compliant: true,
      controls: passing(['branch.pull_request', 'tags.creation'])
    },
    npmProvenance: {
      schemaVersion: 1,
      kind: 'coven.npm-provenance-verification',
      packages: PACKAGES.map((packageName) => ({
        packageName,
        npmVersion: VERSION,
        integrity: 'sha512-abc',
        shasum: '0'.repeat(40),
        tarball: `https://registry.npmjs.org/${packageName}/-/x-${VERSION}.tgz`,
        attestationsUrl: `https://registry.npmjs.org/-/npm/v1/attestations/${packageName}@${VERSION}`,
        provenancePredicateType: 'https://slsa.dev/provenance/v1',
        trustedPublisherPredicateType: 'https://github.com/npm/attestation/tree/main/specs/publish/v0.1',
        invocationId: 'https://github.com/OpenCoven/coven/actions/runs/5001/attempts/1'
      }))
    },
    assets: [
      { name: `coven-v${VERSION}-linux-x64.tar.gz`, sha256: '4'.repeat(64), size: 10 },
      { name: SBOM_NAME, sha256: '5'.repeat(64), size: 20 },
      { name: 'SHA256SUMS', sha256: '6'.repeat(64), size: 30 }
    ],
    sbom: {
      bomFormat: 'CycloneDX',
      serialNumber: 'urn:uuid:00000000-0000-5000-8000-000000000000',
      metadata: { properties: [{ name: 'coven:source-commit', value: HEAD }] },
      components: [{}, {}]
    },
    npmLatest: Object.fromEntries(PACKAGES.map((name) => [name, VERSION])),
    supportedReleaseLine: '0.4',
    environment: { node: 'v24.22.0', npm: '11.19.1', runnerImage: 'ubuntu24 2' },
    observedAt: OBSERVED_AT,
    ...overrides
  };
}

test('a consistent release produces a receipt binding every piece of evidence', () => {
  const receipt = buildReleaseReceipt(inputs());
  assert.equal(receipt.kind, 'coven.release-receipt');
  assert.deepEqual(receipt.source, {
    commit: HEAD,
    tagObject: TAG_OBJECT,
    defaultBranch: 'main',
    defaultBranchCommit: '3'.repeat(40),
    signer: { githubLogin: 'BunsDev', fingerprint: 'SHA256:pinned' },
    githubVerification: { verified: true, reason: 'valid' },
    signerPins: { path: '.github/release-signers.json', taggedCommitSha256: 'a'.repeat(64), defaultBranchSha256: 'a'.repeat(64) }
  });
  assert.equal(receipt.checks.beforeRegistryWrites.workflow.runId, '4001');
  assert.equal(receipt.checks.beforeGitHubPublication.requiredChecks[0].conclusion, 'success');
  assert.equal(receipt.protection.beforeGitHubPublication.reader.credentialSource, 'RELEASE_GOVERNANCE_READ_TOKEN');
  assert.equal(receipt.protection.beforeRegistryWrites.taggedCommit.compliant, true);
  assert.deepEqual(receipt.cleanliness.map((entry) => entry.status), ['pass', 'pass', 'pass', 'pass']);
  assert.equal(receipt.publicationAuthorization.sha256, 'e'.repeat(64));
  assert.deepEqual(receipt.npm.packages.map((entry) => entry.name), [...PACKAGES].sort());
  assert.equal(receipt.npm.packages[0].provenance.predicateType, 'https://slsa.dev/provenance/v1');
  assert.deepEqual(receipt.github.assets.map((entry) => entry.name), ['SHA256SUMS', SBOM_NAME, `coven-v${VERSION}-linux-x64.tar.gz`].sort());
  assert.equal(receipt.github.sbom.serialNumber, 'urn:uuid:00000000-0000-5000-8000-000000000000');
  assert.equal(receipt.channels.npm['@opencoven/cli'].state, 'current');
  assert.equal(receipt.toolchains.registryWrites.npm, '11.19.1');
  assert.deepEqual(receipt.recovery, { decision: 'none', detail: 'first publication' });
});

test('a GitHub-only recovery records its decision', () => {
  const receipt = buildReleaseReceipt(inputs({ trigger: 'workflow_dispatch' }));
  assert.equal(receipt.recovery.decision, 'github-release-recovery');
  assert.match(receipt.recovery.detail, /npm workflow run 5001 attempt 1; npm was not republished/);
});

const refusals = [
  ['an unauthorized npm publication', (input) => { input.publicationAuthorization.authorized = false; }, /was not authorized/],
  ['evidence about another tag object', (input) => { input.publicationAuthorization.tagObjectSha = '9'.repeat(40); }, /publication authorization names/],
  ['source acceptance for another commit', (input) => { input.sourceAcceptance.candidateSha = '9'.repeat(40); }, /source acceptance names/],
  ['protection that drifted before GitHub publication', (input) => {
    input.governanceEvaluation.compliant = false;
    input.governanceEvaluation.controls[1].status = 'fail';
  }, /no longer matches the governance policy \(tags\.creation\)/],
  ['an npm run that built another commit', (input) => { input.sourceRun.headSha = '9'.repeat(40); }, /npm source run built/],
  ['provenance missing a package', (input) => { input.npmProvenance.packages.pop(); }, /must cover exactly/],
  ['provenance for another version', (input) => { input.npmProvenance.packages[0].npmVersion = '0.4.10'; }, /must cover exactly/],
  ['a missing SBOM', (input) => { input.assets = input.assets.filter((entry) => entry.name !== SBOM_NAME); }, /must be present/],
  ['an SBOM for another commit', (input) => { input.sbom.metadata.properties[0].value = '9'.repeat(40); }, /SBOM describes/],
  ['a receipt asked to list itself', (input) => { input.assets.push({ name: RECEIPT_NAME, sha256: '7'.repeat(64), size: 1 }); }, /cannot list itself/],
  ['npm still serving an older latest', (input) => { input.npmLatest['@opencoven/cli-windows'] = '0.4.10'; }, /cli-windows latest 0\.4\.10 \(behind\)/],
  ['a version off the supported line', (input) => { input.supportedReleaseLine = '0.5'; }, /not on the supported 0\.5\.x line/],
  ['an evidence file of the wrong kind', (input) => { input.tagVerification.kind = 'other'; }, /tag verification must be coven\.release-tag-verification/]
];

for (const [label, mutate, expected] of refusals) {
  test(`the receipt refuses ${label}`, () => {
    const input = structuredClone(inputs());
    mutate(input);
    assert.throws(() => buildReleaseReceipt(input), expected);
  });
}

test('npm channel states distinguish a later release from incoherence', () => {
  assert.equal(npmChannelState('0.4.11', '0.4.11'), 'current');
  assert.equal(npmChannelState('0.4.12', '0.4.11'), 'superseded');
  assert.equal(npmChannelState('0.4.10', '0.4.11'), 'behind');
  assert.equal(npmChannelState(undefined, '0.4.11'), 'unreadable');
  assert.equal(npmChannelState('0.4.11-rc.1', '0.4.11'), 'unreadable');
});

function published(receipt, receiptSha256, overrides = {}) {
  return {
    id: 7,
    tag_name: TAG,
    draft: false,
    immutable: true,
    published_at: '2026-10-19T01:00:00Z',
    html_url: `https://github.com/OpenCoven/coven/releases/tag/${TAG}`,
    body: `Coven ${TAG}\n\n${NOTES}\n`,
    assets: [
      ...receipt.github.assets.map((entry) => ({ name: entry.name, digest: `sha256:${entry.sha256}` })),
      { name: RECEIPT_NAME, digest: `sha256:${receiptSha256}` }
    ],
    ...overrides
  };
}

function reconcileInputs(overrides = {}) {
  const receipt = buildReleaseReceipt(inputs());
  const receiptSha256 = '8'.repeat(64);
  return {
    receipt,
    receiptSha256,
    release: published(receipt, receiptSha256),
    tagRef: { object: { type: 'tag', sha: TAG_OBJECT } },
    npmLatest: Object.fromEntries(PACKAGES.map((name) => [name, VERSION])),
    supportedReleaseLine: '0.4',
    latestReleaseTag: TAG,
    observedAt: OBSERVED_AT,
    ...overrides
  };
}

test('a published immutable release that matches its receipt reconciles', () => {
  const result = reconcileRelease(reconcileInputs());
  assert.equal(result.reconciled, true, JSON.stringify(result.controls, null, 2));
  assert.deepEqual(result.controls.map((entry) => entry.id), [
    'github.published', 'github.assets', 'tag.unchanged', 'npm.latest', 'github.release-notes', 'support.line', 'github.latest'
  ]);
  assert.deepEqual(result.receipt, { name: RECEIPT_NAME, sha256: '8'.repeat(64) });
  assert.equal(result.release.immutable, true);
});

test('a later release superseding this one still reconciles', () => {
  const result = reconcileRelease(reconcileInputs({
    npmLatest: Object.fromEntries(PACKAGES.map((name) => [name, '0.4.12'])),
    latestReleaseTag: 'v0.4.12'
  }));
  assert.equal(result.reconciled, true);
});

const reconcileFailures = [
  ['a mutable release', (input) => { input.release.immutable = false; }, 'github.published', /not immutable/],
  ['a draft', (input) => { input.release.draft = true; }, 'github.published', /still a draft/],
  ['a replaced asset', (input) => { input.release.assets[0].digest = `sha256:${'0'.repeat(64)}`; }, 'github.assets', /served as sha256:0{64}/],
  ['a missing receipt asset', (input) => { input.release.assets.pop(); }, 'github.assets', /release-receipt\.json is not served/],
  ['an asset outside the receipt', (input) => { input.release.assets.push({ name: 'extra.zip', digest: 'sha256:00' }); }, 'github.assets', /extra\.zip is served but not in the receipt/],
  ['a moved tag', (input) => { input.tagRef = { object: { type: 'tag', sha: '9'.repeat(40) } }; }, 'tag.unchanged', /now names tag 9{40}/],
  ['npm behind the release', (input) => { input.npmLatest['@opencoven/cli'] = '0.4.10'; }, 'npm.latest', /@opencoven\/cli latest 0\.4\.10 \(behind\)/],
  ['a dropped support line', (input) => { input.supportedReleaseLine = '0.5'; }, 'support.line', /outside the supported 0\.5\.x line/],
  ['notes that leak the tag signature', (input) => { input.release.body += '-----BEGIN SSH SIGNATURE-----\nU1NI\n-----END SSH SIGNATURE-----\n'; }, 'github.release-notes', /raw signature block/],
  ['notes edited after authorization', (input) => { input.release.body = `Coven ${TAG}\n\nDifferent notes.\n`; }, 'github.release-notes', /published notes hash to/],
  ['notes titled for another version', (input) => { input.release.body = `Coven v0.4.10\n\n${NOTES}\n`; }, 'github.release-notes', /titled "Coven v0\.4\.10"/],
  ['an older release marked latest', (input) => { input.latestReleaseTag = 'v0.4.10'; }, 'github.latest', /marks v0\.4\.10 as latest/]
];

for (const [label, mutate, id, expected] of reconcileFailures) {
  test(`reconciliation fails on ${label}`, () => {
    const input = reconcileInputs();
    mutate(input);
    const result = reconcileRelease(input);
    assert.equal(result.reconciled, false);
    const found = result.controls.find((entry) => entry.id === id);
    assert.equal(found.status, 'fail');
    assert.match(found.detail, expected);
  });
}

test('release notes drop the signature block GitHub appends to tag messages', async () => {
  const { releaseNotesFromTagMessage } = await import('./package-github-release.mjs');
  const signed = `Coven ${TAG}\n\n${NOTES}\n-----BEGIN SSH SIGNATURE-----\nU1NIU0lH\nAAAA==\n-----END SSH SIGNATURE-----\n`;
  assert.equal(releaseNotesFromTagMessage(signed), `Coven ${TAG}\n\n${NOTES}\n`);
  assert.equal(releaseNotesFromTagMessage(`Coven ${TAG}\n\nnotes\n-----BEGIN PGP SIGNATURE-----\nx\n-----END PGP SIGNATURE-----`), `Coven ${TAG}\n\nnotes\n`);
  assert.equal(releaseNotesFromTagMessage(`Coven ${TAG}\n\nplain\n\n`), `Coven ${TAG}\n\nplain\n`);
});
