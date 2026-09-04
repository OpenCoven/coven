import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const profileDir = path.join(
  repositoryRoot,
  'spec',
  'coven-automations',
  'authority',
  'v1'
);
const packageScriptPath = path.join(
  repositoryRoot,
  'scripts',
  'package-automations-authority-profile.mjs'
);
const validatorPath = path.join(
  repositoryRoot,
  'scripts',
  'validate-automations-authority-profile.mjs'
);

const REQUIRED_FILES = [
  'README.md',
  'authority-extension.schema.json',
  'automation-execution-binding.schema.json',
  'automation-receipt-authority-evidence.schema.json',
  'capabilities.json',
  'common.schema.json',
  'compatibility-matrix.json',
  'conformance-manifest.json',
  'coven.automations.authority.v1.d.ts',
  'protocol-version.json',
  'test-vectors.json',
  'upstream-artifacts.json'
];

const REQUIRED_NEGATIVE_VECTOR_IDS = [
  'explicit-null-authority-value',
  'missing-profile',
  'unknown-profile',
  'malformed-binding-capabilities',
  'malformed-extension-receipt-capabilities',
  'malformed-null-execution-binding',
  'malformed-profile',
  'malformed-receipt-capabilities',
  'malformed-authorization-timestamp',
  'authorization-invalid-ordering',
  'authorization-issued-after-dispatch',
  'authorization-valid-until-exclusive',
  'decision-after-dispatch',
  'forged-principal',
  'mismatched-familiar-root',
  'mismatched-familiar-revision',
  'mismatched-familiar-declaration-digest',
  'mismatched-familiar-embodiment-digest',
  'mismatched-familiar-embodiment-binding-id',
  'stale-familiar',
  'stale-familiar-verification',
  'future-familiar-verification',
  'revoked-familiar',
  'retired-familiar',
  'nonce-replay',
  'adoption-replay',
  'approval-reuse',
  'approval-expired',
  'approval-revoked',
  'approval-required-bypass',
  'bounded-recurring-approval-exhausted',
  'bounded-recurring-occurrence-out-of-scope',
  'bounded-recurring-occurrence-replay',
  'terminal-dispatch-adoption-owner-mismatch',
  'terminal-dispatch-binding-owner-mismatch',
  'terminal-dispatch-ownership-missing',
  'terminal-dispatch-fence-owner-mismatch',
  'terminal-committed-adoption-missing',
  'terminal-committed-nonce-missing',
  'terminal-committed-recurring-consumption-missing',
  'terminal-dispatch-nonce-owner-mismatch',
  'terminal-dispatch-run-owner-mismatch',
  'terminal-dispatch-attempt-owner-mismatch',
  'terminal-dispatch-attempt-id-owner-mismatch',
  'terminal-dispatch-per-run-approval-owner-mismatch',
  'malformed-terminal-dispatch-consumption-null',
  'malformed-terminal-dispatch-consumption-object',
  'malformed-unrelated-dispatch-consumption-identifier',
  'malformed-unrelated-dispatch-consumption-approval',
  'terminal-dispatch-occurrence-fence-conflict',
  'binding-id-mismatch',
  'capability-escalation',
  'runtime-downgrade',
  'stale-policy',
  'old-occurrence-fence',
  'tampered-binding-digest',
  'tampered-authentication-signature',
  'trusted-binding-digest-mismatch',
  'missing-trusted-replay-state',
  'missing-trusted-proof-store',
  'missing-trusted-runtime-capabilities',
  'rehashed-payload-reused-proof',
  'unverifiable-authentication',
  'unknown-field',
  'unknown-nested-field',
  'receipt-binding-splice',
  'base-receipt-digest-mismatch',
  'receipt-id-mismatch',
  'receipt-authorization-splice',
  'receipt-approval-splice',
  'receipt-approval-required-bypass',
  'receipt-risk-splice',
  'receipt-grant-splice',
  'missing-terminal-receipt-evidence',
  'ungranted-capability-exercised',
  'unpaired-surrogate-object-key',
  'unpaired-surrogate-runtime-id',
  'unauthorized-sensitive-evidence',
  'binding-privacy-mismatch',
  'receipt-privacy-splice'
];

const REQUIRED_NEGOTIATION_VECTOR_IDS = [
  'generic-unknown-extension-preserved',
  'generic-non-finite-number-refused',
  'generic-undefined-refused',
  'generic-non-json-object-refused',
  'generic-sparse-array-refused',
  'generic-array-property-refused',
  'generic-symbol-key-refused',
  'runtime-authority-advertised',
  'runtime-authority-pre-dispatch-advertised',
  'runtime-authority-capability-missing',
  'runtime-authority-terminal-evidence-missing',
  'runtime-authority-unknown-profile'
];

const REQUIRED_POSITIVE_VECTOR_IDS = [
  'astral-runtime-id-64-code-points',
  'valid-terminal-unrelated-and-prior-fence-history',
  'valid-terminal-unrelated-recurring-approval-history'
];

function readJson(relativePath) {
  return JSON.parse(readFileSync(path.join(profileDir, relativePath), 'utf8'));
}

function walkSchema(node, visit, location = '#') {
  if (!node || typeof node !== 'object') {
    return;
  }
  visit(node, location);
  if (Array.isArray(node)) {
    node.forEach((value, index) => walkSchema(value, visit, `${location}/${index}`));
    return;
  }
  for (const [key, value] of Object.entries(node)) {
    walkSchema(value, visit, `${location}/${key}`);
  }
}

function runGit(cwd, args, env = process.env) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', env });
  assert.equal(
    result.status,
    0,
    `git ${args.join(' ')} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`
  );
  return result.stdout.trim();
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function withScratchDir(name, callback) {
  const scratchDir = mkdtempSync(path.join(tmpdir(), `coven-${name}-`));
  try {
    return callback(scratchDir);
  } finally {
    rmSync(scratchDir, { recursive: true, force: true });
  }
}

function createAuthorityFixtureRepository(scratchDir) {
  const repoRoot = path.join(scratchDir, 'repo');
  const specDir = path.join(repoRoot, 'spec', 'coven-automations', 'authority', 'v1');
  mkdirSync(path.join(specDir, 'nested'), { recursive: true });
  writeFileSync(path.join(specDir, 'README.md'), '# Authority Contract\n');
  writeFileSync(path.join(specDir, 'b.json'), '{"b":2}\n');
  writeFileSync(path.join(specDir, 'nested', 'a.json'), '{"a":1}\n');
  runGit(repoRoot, ['init']);
  runGit(repoRoot, ['config', 'user.name', 'Authority Profile Test']);
  runGit(repoRoot, ['config', 'user.email', 'authority-profile@example.invalid']);
  runGit(repoRoot, ['add', '.']);
  runGit(repoRoot, ['commit', '-m', 'test: seed authority profile'], {
    ...process.env,
    GIT_AUTHOR_DATE: '2026-01-01T00:00:00Z',
    GIT_COMMITTER_DATE: '2026-01-01T00:00:00Z'
  });
  return {
    repoRoot,
    sourceCommit: runGit(repoRoot, ['rev-parse', 'HEAD'])
  };
}

test('publishes the complete separately versioned authority companion profile', () => {
  assert.equal(existsSync(profileDir), true, 'authority profile directory is missing');
  assert.deepEqual(readdirSync(profileDir).sort(), REQUIRED_FILES);

  const version = readJson('protocol-version.json');
  assert.equal(version.contractProfile, 'coven.automations.authority.v1');
  assert.equal(version.baseProfile, 'coven.automations.v1');
  assert.equal(version.transport.extensionKey, 'coven.automations.authority.v1');
  assert.deepEqual(version.transport.requiredOn, ['AutomationRun.extensions']);
  assert.equal(
    version.transport.receiptEvidence,
    'receipt-correlated-sidecar-inside-authority-extension'
  );
  assert.equal(version.unknownProfileBehavior, 'fail-closed');
  assert.equal(
    version.normativeInputs.familiarContract.commit,
    '13d150a32a817da19bb4e5053f2205b15db0bb0a'
  );
  assert.equal(
    version.normativeInputs.covenThreads.commit,
    'c3bd46bcadb6396db8436c47411a4d0eac17192b'
  );
  const upstreamArtifacts = readJson('upstream-artifacts.json');
  assert.deepEqual(upstreamArtifacts, {
    profile: 'coven.automations.authority.v1',
    artifacts: [
      {
        repository: 'OpenCoven/familiar-contract',
        commit: '13d150a32a817da19bb4e5053f2205b15db0bb0a',
        path: 'schemas/familiar-embodiment-binding.schema.json',
        sha256: '9cafde21a460f688add8e796651e6af98cec26361c839f29400cb21f65afafdf',
        size: 10999
      },
      {
        repository: 'OpenCoven/familiar-contract',
        commit: '13d150a32a817da19bb4e5053f2205b15db0bb0a',
        path: 'tests/conformance/embodiment-bindings/manifest.json',
        sha256: '14aea1848afa808b64688f3b5379a37a26253f0d846a2cd655d9651e32348a80',
        size: 10844
      },
      {
        repository: 'OpenCoven/familiar-contract',
        commit: '13d150a32a817da19bb4e5053f2205b15db0bb0a',
        path:
          'tests/conformance/embodiment-bindings/positive/09-scheduled-automation-run.json',
        sha256: '5980fc5b312a3e1dd1d90fa7967166a57bc941101aa88e58683223180ad5ed60',
        size: 4544
      },
      {
        repository: 'OpenCoven/familiar-contract',
        commit: '13d150a32a817da19bb4e5053f2205b15db0bb0a',
        path:
          'tests/conformance/embodiment-bindings/negative/02-stale-cached-revision.json',
        sha256: '996ca1e225993c11e4cee190dfe024d54b7df70c970d5482e4356059b64548a9',
        size: 4572
      },
      {
        repository: 'OpenCoven/familiar-contract',
        commit: '13d150a32a817da19bb4e5053f2205b15db0bb0a',
        path:
          'tests/conformance/embodiment-bindings/negative/14-unverifiable-authentication.json',
        sha256: '6c995e8ac33b03c20205f16ae5636b4e29e4b212d9bc75fcf23a8666eec072c0',
        size: 4454
      },
      {
        repository: 'OpenCoven/coven-threads',
        commit: 'c3bd46bcadb6396db8436c47411a4d0eac17192b',
        path: 'profiles/automation-authority/v1/manifest.json',
        sha256: 'f58778c2661549e7ca9e69003906c3c116c1dda7f5a4487f6594279706fbf03a',
        size: 42629
      },
      {
        repository: 'OpenCoven/coven-threads',
        commit: 'c3bd46bcadb6396db8436c47411a4d0eac17192b',
        path: 'profiles/automation-authority/v1/schemas/decision.schema.json',
        sha256: '8602dc0156becd6f0fdd85fcafe81a968c4e38ce4b8123f27813aed6df2e7369',
        size: 7138
      },
      {
        repository: 'OpenCoven/coven-threads',
        commit: 'c3bd46bcadb6396db8436c47411a4d0eac17192b',
        path: 'profiles/automation-authority/v1/vectors/01-r0-read-permit.json',
        sha256: 'e05338aa37d97944445fb3ec24fa99c99ba64e8cc23663eea44a6c34b188101c',
        size: 7004
      },
      {
        repository: 'OpenCoven/coven-threads',
        commit: 'c3bd46bcadb6396db8436c47411a4d0eac17192b',
        path:
          'profiles/automation-authority/v1/vectors/14-runtime-capability-downgrade.json',
        sha256: 'db2214f672dfc01a179e881d637df4098442f494ad21403353160e1f743e1994',
        size: 9308
      },
      {
        repository: 'OpenCoven/coven-threads',
        commit: 'c3bd46bcadb6396db8436c47411a4d0eac17192b',
        path:
          'profiles/automation-authority/v1/vectors/15-stale-fence-before-dispatch.json',
        sha256: '9d4974407abab985ec91bdc9ea52d0c25497a75fcc07ceb011a46c47ca72de65',
        size: 9309
      }
    ]
  });
  assert.equal(version.historicalBaseArtifact.artifactId, 9909975069);
  assert.equal(
    version.historicalBaseArtifact['bundleSha256'],
    '512460db71d4257d7a4d33ea306578e66d9ac499d9384eb9c2b8e2b4e2e32363'
  );
  assert.equal(
    version.historicalBaseArtifact['contractContentSha256'],
    '3c145eb92a93426ed64631f6487a8cd12903b0a49a6e752269f594ac50a779f5'
  );
});

test('uses closed draft-2020-12 schemas with bounded authority values', () => {
  assert.equal(existsSync(profileDir), true, 'authority profile directory is missing');
  for (const schemaName of [
    'common.schema.json',
    'authority-extension.schema.json',
    'automation-execution-binding.schema.json',
    'automation-receipt-authority-evidence.schema.json'
  ]) {
    const schema = readJson(schemaName);
    assert.equal(schema.$schema, 'https://json-schema.org/draft/2020-12/schema');
    walkSchema(schema, (node, location) => {
      if (node.type === 'object' && node.properties) {
        assert.equal(
          node.additionalProperties,
          false,
          `${schemaName}${location} must reject unknown fields`
        );
      }
    });
  }

  const common = readJson('common.schema.json');
  const baseCommon = JSON.parse(
    readFileSync(
      path.join(repositoryRoot, 'spec', 'coven-automations', 'v1', 'common.schema.json'),
      'utf8'
    )
  );
  const baseOccurrence = JSON.parse(
    readFileSync(
      path.join(
        repositoryRoot,
        'spec',
        'coven-automations',
        'v1',
        'automation-occurrence.schema.json'
      ),
      'utf8'
    )
  );
  for (const name of [
    'automationId',
    'occurrenceId',
    'runId',
    'attemptId',
    'receiptId',
    'adoptionKey'
  ]) {
    assert.deepEqual(common.$defs[`base${name[0].toUpperCase()}${name.slice(1)}`], baseCommon.$defs[name]);
  }
  assert.deepEqual(common.$defs.baseOccurrenceKey, baseOccurrence.properties.occurrenceKey);
  assert.deepEqual(
    common.$defs.basePrincipalId,
    baseCommon.$defs.principalRef.properties.principalId
  );
  assert.deepEqual(
    common.$defs.baseRuntimeId,
    baseCommon.$defs.runtimeDescriptor.properties.runtimeId
  );
  assert.equal(common.$defs.opaqueIdentifier.minLength, 1);
  assert.equal(common.$defs.opaqueIdentifier.maxLength, 256);
  assert.match(common.$defs.opaqueIdentifier.pattern, /^\^/);
  assert.equal(common.$defs.digest.properties.value.pattern, '^[0-9a-f]{64}$');
  assert.match(common.$defs.timestamp.pattern, /Z\$$/);
  assert.equal(common.$defs.capability.maxLength, 96);
  assert.equal(common.$defs.projectionIds.maxItems, 128);
});

test('models every immutable execution binding anchor and minimized receipt evidence', () => {
  assert.equal(existsSync(profileDir), true, 'authority profile directory is missing');
  assert.equal(
    existsSync(path.join(profileDir, 'authority-extension.schema.json')),
    true,
    'authority extension envelope schema is missing'
  );
  const extension = readJson('authority-extension.schema.json');
  assert.deepEqual(extension.required, ['profile', 'kind', 'executionBinding', 'receiptEvidence']);
  assert.equal(
    extension.properties.executionBinding.$ref,
    'automation-execution-binding.schema.json'
  );
  assert.equal(
    extension.properties.receiptEvidence.oneOf[1].$ref,
    'automation-receipt-authority-evidence.schema.json'
  );

  const common = readJson('common.schema.json');
  const binding = readJson('automation-execution-binding.schema.json');
  for (const required of [
    'profile',
    'kind',
    'bindingId',
    'base',
    'principal',
    'authorization',
    'familiar',
    'contextProjection',
    'threads',
    'capabilities',
    'approval',
    'risk',
    'runtime',
    'versions',
    'decisionTimestamp',
    'producer',
    'privacy',
    'integrity',
    'authentication'
  ]) {
    assert.ok(binding.required.includes(required), `binding requires ${required}`);
  }
  const receipt = readJson('automation-receipt-authority-evidence.schema.json');
  assert.deepEqual(binding.properties.familiar.properties.statusAtDecision.enum, [
    'active',
    'revoked',
    'retired',
    'stale'
  ]);
  assert.deepEqual(receipt.properties.familiar.properties.statusAtDecision.enum, [
    'active',
    'revoked',
    'retired',
    'stale'
  ]);
  for (const approval of common.$defs.approvalBinding.oneOf.filter(
    (candidate) => candidate.properties.evidence.type === 'object'
  )) {
    assert.deepEqual(approval.properties.evidence.properties.state.enum, ['approved', 'revoked']);
  }
  const declarations = readFileSync(
    path.join(profileDir, 'coven.automations.authority.v1.d.ts'),
    'utf8'
  );
  assert.match(declarations, /statusAtDecision: "active" \| "revoked" \| "retired" \| "stale";/u);
  assert.match(declarations, /state: "approved" \| "revoked";/u);
  for (const required of ['validTime', 'revocation', 'retirement']) {
    assert.ok(
      binding.properties.familiar.required.includes(required),
      `binding familiar requires ${required}`
    );
    assert.ok(
      receipt.properties.familiar.required.includes(required),
      `receipt familiar requires ${required}`
    );
  }

  for (const required of [
    'receiptId',
    'automationId',
    'automationRevision',
    'occurrenceId',
    'occurrenceFenceGeneration',
    'runId',
    'attemptId',
    'baseReceiptDigest',
    'bindingId',
    'bindingDigest',
    'principalId',
    'familiar',
    'authorization',
    'capabilities',
    'risk',
    'runtime',
    'decisionTimestamp',
    'producer',
    'privacy',
    'integrity',
    'authentication'
  ]) {
    assert.ok(receipt.required.includes(required), `receipt evidence requires ${required}`);
  }

  const serialized = JSON.stringify({ binding, receipt });
  for (const forbidden of [
    '"credential"',
    '"credentialValue"',
    '"prompt"',
    '"memory"',
    '"filesystemPath"',
    '"absolutePath"'
  ]) {
    assert.equal(serialized.includes(forbidden), false, `forbidden sensitive field ${forbidden}`);
  }
});

test('runs positive and explicit fail-closed authority vectors', async () => {
  assert.equal(existsSync(profileDir), true, 'authority profile directory is missing');
  assert.equal(existsSync(validatorPath), true, 'authority vector validator is missing');
  const vectors = readJson('test-vectors.json');
  const capabilities = readJson('capabilities.json');
  const negativeIds = vectors.cases
    .filter((vector) => vector.expected === 'refuse')
    .map((vector) => vector.id)
    .sort();
  assert.deepEqual(negativeIds, [...REQUIRED_NEGATIVE_VECTOR_IDS].sort());
  const positiveIds = vectors.cases
    .filter((vector) => vector.expected === 'accept')
    .map((vector) => vector.id);
  for (const id of REQUIRED_POSITIVE_VECTOR_IDS) {
    assert.equal(positiveIds.includes(id), true, `missing positive vector ${id}`);
  }
  assert.deepEqual(
    vectors.negotiationCases.map((vector) => vector.id).sort(),
    [...REQUIRED_NEGOTIATION_VECTOR_IDS].sort()
  );

  const { runAuthorityVectors } = await import(pathToFileURL(validatorPath));
  const summary = runAuthorityVectors(vectors);
  assert.deepEqual(summary, {
    total: 100,
    accepted: 11,
    refused: 89
  });
  for (const vector of vectors.cases.filter((entry) => entry.expected === 'refuse')) {
    assert.equal(
      capabilities.errorCodes.includes(vector.errorCode),
      true,
      `${vector.id} uses unadvertised refusal code ${vector.errorCode}`
    );
  }
  assert.equal(capabilities.errorCodes.includes('AUTHORITY_UNVERIFIABLE'), false);
  assert.equal(capabilities.errorCodes.includes('AUTHORITY_ADAPTER_MISSING'), true);
  assert.equal(capabilities.errorCodes.includes('AUTHORITY_TRUSTED_STATE_UNAVAILABLE'), true);
});

test('publishes strict I-JSON conformance vectors consumable by jq', () => {
  const result = spawnSync('jq', ['empty', path.join(profileDir, 'test-vectors.json')], {
    cwd: repositoryRoot,
    encoding: 'utf8'
  });
  assert.equal(
    result.status,
    0,
    `jq rejected published authority vectors\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`
  );
});

test('packages and independently verifies a deterministic authority profile bundle', async () => {
  assert.equal(existsSync(packageScriptPath), true, 'authority package script is missing');
  const {
    packageAutomationsAuthorityProfile,
    verifyAutomationsAuthorityProfileBundle
  } = await import(pathToFileURL(packageScriptPath));

  withScratchDir('automation-authority-package', (scratchDir) => {
    const fixture = createAuthorityFixtureRepository(scratchDir);
    const first = packageAutomationsAuthorityProfile({
      repoRoot: fixture.repoRoot,
      outputDir: path.join(scratchDir, 'first'),
      sourceCommit: fixture.sourceCommit
    });
    const second = packageAutomationsAuthorityProfile({
      repoRoot: fixture.repoRoot,
      outputDir: path.join(scratchDir, 'second'),
      sourceCommit: fixture.sourceCommit
    });

    assert.equal(
      path.basename(first.bundlePath),
      `coven-automations-authority-v1-contract-${fixture.sourceCommit}.tar.gz`
    );
    assert.deepEqual(readFileSync(first.bundlePath), readFileSync(second.bundlePath));
    assert.deepEqual(readFileSync(first.manifestPath), readFileSync(second.manifestPath));

    const manifest = JSON.parse(readFileSync(first.manifestPath, 'utf8'));
    assert.equal(manifest.schemaVersion, 'coven.contract-profile.bundle.v1');
    assert.equal(manifest.contractProfile, 'coven.automations.authority.v1');
    assert.equal(manifest.sourceCommit, fixture.sourceCommit);
    assert.deepEqual(
      manifest.files.map((file) => file.path),
      ['README.md', 'b.json', 'nested/a.json']
    );

    const verified = verifyAutomationsAuthorityProfileBundle({
      bundlePath: first.bundlePath,
      expectedSourceCommit: fixture.sourceCommit,
      expectedBundleSha256: sha256(readFileSync(first.bundlePath))
    });
    assert.equal(verified.contractContentSha256, first.contractContentSha256);
    assert.equal(verified.fileCount, 3);
  });
});

test('wires CI and release publication without relabeling the historical base artifact', () => {
  const ci = readFileSync(path.join(repositoryRoot, '.github', 'workflows', 'ci.yml'), 'utf8');
  const release = readFileSync(
    path.join(repositoryRoot, '.github', 'workflows', 'release-github.yml'),
    'utf8'
  );
  const releaseScript = readFileSync(
    path.join(repositoryRoot, 'scripts', 'package-github-release.mjs'),
    'utf8'
  );

  assert.match(ci, /automations-authority-profile-bundle:/);
  assert.match(ci, /coven-automations-authority-v1-contract-\$\{\{ github\.sha \}\}/);
  assert.match(release, /--authority-profile-bundle "\$AUTHORITY_PROFILE_BUNDLE"/);
  assert.match(releaseScript, /coven-automations-authority-v1-contract-/);
  assert.doesNotMatch(releaseScript, /9909975069/);
  assert.doesNotMatch(release, /9909975069/);
});
