import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import test from 'node:test';
import { gunzipSync } from 'node:zlib';

import {
  PACKAGE_DEFINITIONS,
  assertChecksumManifest,
  captureCommandOutputToFile,
  canonicalReleaseAssetNames,
  packageGitHubRelease,
  resolveReleaseSource,
  syncGitHubRelease,
  verifyAnnotatedTag,
  verifyNpmRegistrySignatures,
  verifyPackageProvenance,
  verifyReleaseSource,
  verifySourceRun
} from './package-github-release.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'release-github.yml');
const ciWorkflowPath = path.join(repoRoot, '.github', 'workflows', 'ci.yml');
const workflowText = readFileSync(workflowPath, 'utf8');
const ciWorkflowText = readFileSync(ciWorkflowPath, 'utf8');
const fixtureRoot = path.join(repoRoot, 'scripts', 'fixtures', 'package-github-release', 'source-artifacts');
const scratchRoot = path.join(repoRoot, 'npm', 'dist', '.package-github-release-tests');
const SOURCE_DATE_EPOCH = 1_786_939_861;
const RELEASE_TAG = 'v0.4.1';
const NPM_VERSION = '0.4.1';
const HEAD_SHA = '0000000000000000000000000000000000000000';
const SOURCE_RUN_ID = '31993572717';
const SOURCE_RUN_ATTEMPT = 1;
const CHECKOUT_ACTION_SHA = ['3d3c42e5aac5ba805825da76', '410c181273ba90b1'].join('');
const SETUP_NODE_ACTION_SHA = ['820762786026740c76f36085', 'b0efc47a31fe5020'].join('');
const DOWNLOAD_ARTIFACT_ACTION_SHA = ['3e5f45b2cfb9172054b4087a', '40e8e0b5a5461e7c'].join('');
const RELEASE_PACKAGES = [
  '@opencoven/cli',
  '@opencoven/cli-linux-x64',
  '@opencoven/cli-windows',
  '@opencoven/cli-macos-x64',
  '@opencoven/cli-macos'
];
const EXPECTED_ARCHIVES = [
  'coven-v0.4.1-linux-x64.tar.gz',
  'coven-v0.4.1-macos-aarch64.tar.gz',
  'coven-v0.4.1-macos-x64.tar.gz',
  'coven-v0.4.1-windows-x64.zip'
].sort();
const EXPECTED_ASSET_NAMES = [...EXPECTED_ARCHIVES, 'SHA256SUMS'].sort();

function releasePackageVersionMap(version = NPM_VERSION) {
  return Object.fromEntries(RELEASE_PACKAGES.map((packageName) => [packageName, version]));
}

function writeSignatureAuditLockfile(
  auditDir,
  {
    resolvedPackages = RELEASE_PACKAGES,
    rootDependencies = releasePackageVersionMap(),
    versionOverrides = {},
    packageEntryOverrides = {},
    lockfileVersion = 3
  } = {}
) {
  const packageEntries = Object.fromEntries(
    resolvedPackages.map((packageName) => [
      `node_modules/${packageName}`,
      {
        version: versionOverrides[packageName] ?? NPM_VERSION,
        resolved: `https://registry.npmjs.org/${encodeURIComponent(packageName)}/-/${packageName.split('/').at(-1)}-${versionOverrides[packageName] ?? NPM_VERSION}.tgz`,
        integrity: `sha512-${Buffer.from(packageName).toString('base64')}`,
        ...packageEntryOverrides[packageName]
      }
    ])
  );
  const rootDependencyEntries = Object.fromEntries(
    Object.entries(rootDependencies).map(([packageName, version]) => [
      packageName,
      {
        version,
        resolved: `https://registry.npmjs.org/${encodeURIComponent(packageName)}/-/${packageName.split('/').at(-1)}-${version}.tgz`,
        integrity: `sha512-${Buffer.from(packageName).toString('base64')}`
      }
    ])
  );
  writeFileSync(
    path.join(auditDir, 'package-lock.json'),
    `${
      JSON.stringify(
        lockfileVersion === 1
          ? {
              name: 'opencoven-release-npm-signatures-audit',
              version: '0.0.0',
              lockfileVersion,
              requires: true,
              dependencies: rootDependencyEntries
            }
          : {
              name: 'opencoven-release-npm-signatures-audit',
              version: '0.0.0',
              lockfileVersion,
              requires: true,
              packages: {
                '': {
                  name: 'opencoven-release-npm-signatures-audit',
                  version: '0.0.0',
                  dependencies: rootDependencies
                },
                ...packageEntries
              }
            },
        null,
        2
      )
    }\n`
  );
}

function withScratchDir(name, fn) {
  const dir = path.join(scratchRoot, `${name}-${randomUUID()}`);
  mkdirSync(dir, { recursive: true });
  const cleanup = () => rmSync(dir, { recursive: true, force: true });
  let result;
  try {
    result = fn(dir);
  } catch (error) {
    cleanup();
    throw error;
  }
  if (result && typeof result.then === 'function') {
    return result.finally(cleanup);
  }
  cleanup();
  return result;
}

function sha256Hex(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function integrityFor(bytes) {
  return `sha512-${createHash('sha512').update(bytes).digest('base64')}`;
}

function packageSubjectName(packageName, npmVersion = NPM_VERSION) {
  return `pkg:npm/${packageName.split('/').map((segment) => encodeURIComponent(segment)).join('/')}@${npmVersion}`;
}

function baseValidSourceRun() {
  return {
    id: Number(SOURCE_RUN_ID),
    name: 'Release npm packages',
    path: '.github/workflows/release-npm.yml',
    event: 'push',
    status: 'completed',
    conclusion: 'success',
    head_branch: RELEASE_TAG,
    head_sha: HEAD_SHA,
    run_attempt: SOURCE_RUN_ATTEMPT
  };
}

function baseTagRef() {
  return {
    ref: `refs/tags/${RELEASE_TAG}`,
    object: {
      type: 'tag',
      sha: '1111111111111111111111111111111111111111'
    }
  };
}

function baseTagObject() {
  return {
    tag: RELEASE_TAG,
    object: {
      type: 'commit',
      sha: HEAD_SHA
    },
    verification: {
      verified: true,
      reason: 'valid'
    }
  };
}

function trustedPublisherMetadata(overrides = {}) {
  const base = {
    name: 'GitHub Actions',
    email: 'npm-oidc-no-reply@github.com',
    trustedPublisher: {
      id: 'github',
      oidcConfigId: 'oidc:github-actions-config'
    }
  };
  if (overrides.trustedPublisher === null) {
    return {
      ...base,
      ...overrides,
      trustedPublisher: null
    };
  }
  return {
    ...base,
    ...overrides,
    trustedPublisher: {
      ...base.trustedPublisher,
      ...overrides.trustedPublisher
    }
  };
}

function makePackageMetadata(packageName, integrity, { npmUser = trustedPublisherMetadata() } = {}) {
  return {
    name: packageName,
    version: NPM_VERSION,
    dist: {
      integrity,
      attestations: {
        url: `https://registry.npmjs.org/-/npm/v1/attestations/${encodeURIComponent(packageName)}@${NPM_VERSION}`,
        provenance: { predicateType: 'https://slsa.dev/provenance/v1' }
      }
    },
    _npmUser: npmUser
  };
}

function buildAttestations({
  packageName = '@opencoven/cli',
  npmVersion = NPM_VERSION,
  repository = 'https://github.com/OpenCoven/coven',
  workflowPathValue = '.github/workflows/release-npm.yml',
  workflowRef = `refs/tags/${RELEASE_TAG}`,
  gitCommit = HEAD_SHA,
  invocationId = `https://github.com/OpenCoven/coven/actions/runs/${SOURCE_RUN_ID}/attempts/${SOURCE_RUN_ATTEMPT}`,
  subjectDigest,
  subjectName = packageSubjectName(packageName, npmVersion),
  trustedPublisherPackageName = packageName,
  trustedPublisherVersion = npmVersion,
  trustedPublisherRegistry = 'https://registry.npmjs.org',
  trustedPublisherPayloadBase64,
  slsaPayloadBase64,
  eventName = 'push'
}) {
  const statement = {
    _type: 'https://in-toto.io/Statement/v1',
    subject: [
      {
        name: subjectName,
        digest: { sha512: subjectDigest }
      }
    ],
    predicateType: 'https://slsa.dev/provenance/v1',
    predicate: {
      buildDefinition: {
        buildType: 'https://slsa-framework.github.io/github-actions-buildtypes/workflow/v1',
        externalParameters: {
          workflow: {
            repository,
            path: workflowPathValue,
            ref: workflowRef
          }
        },
        internalParameters: {
          github: {
            event_name: eventName,
            repository_id: '1222160568',
            repository_owner_id: '270919577'
          }
        },
        resolvedDependencies: [
          {
            uri: `git+${repository}@${workflowRef}`,
            digest: { gitCommit }
          }
        ]
      },
      runDetails: {
        builder: { id: 'https://github.com/actions/runner/github-hosted' },
        metadata: { invocationId }
      }
    }
  };
  const publishStatement = {
    _type: 'https://in-toto.io/Statement/v0.1',
    subject: [
      {
        name: subjectName,
        digest: { sha512: subjectDigest }
      }
    ],
    predicateType: 'https://github.com/npm/attestation/tree/main/specs/publish/v0.1',
    predicate: {
      name: trustedPublisherPackageName,
      version: trustedPublisherVersion,
      registry: trustedPublisherRegistry
    }
  };
  return {
    attestations: [
      {
        predicateType: 'https://github.com/npm/attestation/tree/main/specs/publish/v0.1',
        bundle: {
          dsseEnvelope: {
            payload:
              trustedPublisherPayloadBase64 ??
              Buffer.from(JSON.stringify(publishStatement)).toString('base64')
          }
        }
      },
      {
        predicateType: 'https://slsa.dev/provenance/v1',
        bundle: {
          dsseEnvelope: {
            payload: slsaPayloadBase64 ?? Buffer.from(JSON.stringify(statement)).toString('base64')
          }
        }
      }
    ]
  };
}

function parseTarGzEntries(archiveBytes) {
  const gzipMtime = archiveBytes.readUInt32LE(4);
  const tarBytes = gunzipSync(archiveBytes);
  const entries = [];
  for (let offset = 0; offset < tarBytes.length; offset += 512) {
    const header = tarBytes.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      break;
    }
    const name = header.subarray(0, 100).toString('utf8').replace(/\0.*$/, '');
    const mode = Number.parseInt(
      header.subarray(100, 108).toString('ascii').replace(/\0.*$/, '').trim() || '0',
      8
    );
    const size = Number.parseInt(
      header.subarray(124, 136).toString('ascii').replace(/\0.*$/, '').trim() || '0',
      8
    );
    const uid = Number.parseInt(
      header.subarray(108, 116).toString('ascii').replace(/\0.*$/, '').trim() || '0',
      8
    );
    const gid = Number.parseInt(
      header.subarray(116, 124).toString('ascii').replace(/\0.*$/, '').trim() || '0',
      8
    );
    const mtime = Number.parseInt(
      header.subarray(136, 148).toString('ascii').replace(/\0.*$/, '').trim() || '0',
      8
    );
    const typeflag = String.fromCharCode(header[156] || 0);
    const dataOffset = offset + 512;
    const data = tarBytes.subarray(dataOffset, dataOffset + size);
    entries.push({ name, mode, uid, gid, mtime, typeflag, data: Buffer.from(data) });
    offset = dataOffset + Math.ceil(size / 512) * 512 - 512;
  }
  return { gzipMtime, entries };
}

function parseZipEntries(archiveBytes) {
  const eocdSignature = 0x06054b50;
  let eocdOffset = -1;
  for (let offset = archiveBytes.length - 22; offset >= 0; offset -= 1) {
    if (archiveBytes.readUInt32LE(offset) === eocdSignature) {
      eocdOffset = offset;
      break;
    }
  }
  assert.notEqual(eocdOffset, -1, 'zip EOCD must exist');
  const totalEntries = archiveBytes.readUInt16LE(eocdOffset + 10);
  const centralDirectoryOffset = archiveBytes.readUInt32LE(eocdOffset + 16);
  const entries = [];
  let offset = centralDirectoryOffset;
  for (let index = 0; index < totalEntries; index += 1) {
    assert.equal(
      archiveBytes.readUInt32LE(offset),
      0x02014b50,
      'central directory header signature must match'
    );
    const method = archiveBytes.readUInt16LE(offset + 10);
    const dosTime = archiveBytes.readUInt16LE(offset + 12);
    const dosDate = archiveBytes.readUInt16LE(offset + 14);
    const compressedSize = archiveBytes.readUInt32LE(offset + 20);
    const fileNameLength = archiveBytes.readUInt16LE(offset + 28);
    const extraLength = archiveBytes.readUInt16LE(offset + 30);
    const commentLength = archiveBytes.readUInt16LE(offset + 32);
    const externalAttributes = archiveBytes.readUInt32LE(offset + 38);
    const localHeaderOffset = archiveBytes.readUInt32LE(offset + 42);
    const name = archiveBytes
      .subarray(offset + 46, offset + 46 + fileNameLength)
      .toString('utf8');
    const localNameLength = archiveBytes.readUInt16LE(localHeaderOffset + 26);
    const localExtraLength = archiveBytes.readUInt16LE(localHeaderOffset + 28);
    const dataStart = localHeaderOffset + 30 + localNameLength + localExtraLength;
    entries.push({
      name,
      method,
      dosTime,
      dosDate,
      externalAttributes,
      data: Buffer.from(archiveBytes.subarray(dataStart, dataStart + compressedSize))
    });
    offset += 46 + fileNameLength + extraLength + commentLength;
  }
  return entries;
}

function expectedZipTimestamp(epochSeconds) {
  const date = new Date(epochSeconds * 1000);
  return {
    dosDate: ((date.getUTCFullYear() - 1980) << 9) | ((date.getUTCMonth() + 1) << 5) | date.getUTCDate(),
    dosTime: (date.getUTCHours() << 11) | (date.getUTCMinutes() << 5) | Math.floor(date.getUTCSeconds() / 2)
  };
}

function fakeReleaseClient({ existingRelease = null, assetBytesByName = {}, revalidateTagState = null } = {}) {
  const state = {
    release: existingRelease,
    created: [],
    revalidated: [],
    downloads: [],
    uploads: []
  };
  return {
    state,
    async getReleaseByTag() {
      return state.release;
    },
    async revalidateTag({ releaseTag, expectedTagObjectSha, expectedHeadSha }) {
      state.revalidated.push({ releaseTag, expectedTagObjectSha, expectedHeadSha });
      if (typeof revalidateTagState === 'function') {
        return revalidateTagState({ releaseTag, expectedTagObjectSha, expectedHeadSha });
      }
      return revalidateTagState;
    },
    async createRelease({ releaseTag, title, notesFromTag, verifyTag }) {
      state.created.push({ releaseTag, title, notesFromTag, verifyTag });
      state.release = { tagName: releaseTag, assets: [] };
      return state.release;
    },
    async downloadAsset(asset, filePath) {
      state.downloads.push(asset.name);
      const bytes = assetBytesByName[asset.name];
      if (!bytes) {
        throw new Error(`missing fake asset bytes for ${asset.name}`);
      }
      writeFileSync(filePath, bytes);
    },
    async uploadAsset({ releaseTag, assetName, filePath }) {
      state.uploads.push({ releaseTag, assetName, bytes: readFileSync(filePath) });
      state.release.assets.push({ id: state.release.assets.length + 1, name: assetName });
    }
  };
}

function assertRootOnlyTarEntries(entries, expectedNames) {
  assert.deepEqual(
    entries.map((entry) => entry.name),
    expectedNames
  );
  assert.equal(
    new Set(entries.map((entry) => entry.name)).size,
    expectedNames.length,
    'tar entry names must be unique'
  );
  for (const entry of entries) {
    assert.equal(path.basename(entry.name), entry.name, `${entry.name} must stay at the archive root`);
    assert.equal(entry.name.includes('/'), false, `${entry.name} must not include path separators`);
    assert.equal(entry.name.includes('\\'), false, `${entry.name} must not include Windows separators`);
    assert.equal(entry.name.includes('..'), false, `${entry.name} must not contain traversal segments`);
    assert.equal(entry.uid, 0, `${entry.name} uid must be normalized`);
    assert.equal(entry.gid, 0, `${entry.name} gid must be normalized`);
    assert.equal(entry.mode, 0o755, `${entry.name} mode must be normalized`);
    assert.equal(entry.typeflag, '0', `${entry.name} must be archived as a regular file`);
    assert.equal(entry.mtime, SOURCE_DATE_EPOCH, `${entry.name} mtime must match SOURCE_DATE_EPOCH`);
  }
}

test('release-github workflow supports automatic and recovery triggers with pinned actions', () => {
  assert.match(
    workflowText,
    /^on:\s*\n  workflow_run:\s*\n    workflows:\s*\n      - Release npm packages\s*\n    types:\s*\n      - completed/m
  );
  assert.match(
    workflowText,
    /workflow_dispatch:[\s\S]*release_tag:[\s\S]*required: true[\s\S]*source_run_id:[\s\S]*required: true[\s\S]*source_run_attempt:[\s\S]*required: true/
  );
  assert.match(
    workflowText,
    /if: \$\{\{ github\.event_name != 'workflow_run' \|\| github\.event\.workflow_run\.conclusion == 'success' \}\}/
  );
  assert.match(
    workflowText,
    /SOURCE_RUN_ATTEMPT: \$\{\{ github\.event\.workflow_run\.run_attempt \|\| inputs\.source_run_attempt \}\}/
  );
  assert.match(
    workflowText,
    /node scripts\/package-github-release\.mjs verify-source-run[\s\S]*--source-run-attempt "\$SOURCE_RUN_ATTEMPT"/
  );
  assert.match(
    workflowText,
    /node scripts\/package-github-release\.mjs verify-npm-provenance/
  );
  assert.match(
    workflowText,
    /node scripts\/package-github-release\.mjs verify-npm-signatures/
  );
  assert.match(
    workflowText,
    /--audit-dir github-release-npm-audit/
  );
  assert.match(workflowText, /node scripts\/package-github-release\.mjs package/);
  assert.match(
    workflowText,
    /node scripts\/package-github-release\.mjs sync-release[\s\S]*--expected-tag-object-sha "\$TAG_OBJECT_SHA"[\s\S]*--expected-head-sha "\$HEAD_SHA"/
  );
  assert.match(workflowText, new RegExp(`actions/checkout@${CHECKOUT_ACTION_SHA}`));
  assert.match(workflowText, new RegExp(`actions/setup-node@${SETUP_NODE_ACTION_SHA}`));
  assert.equal(
    (
      workflowText.match(
        new RegExp(`actions/download-artifact@${DOWNLOAD_ARTIFACT_ACTION_SHA}`, 'g')
      ) ?? []
    ).length,
    4
  );
});

test('release-github workflow uses least privilege, default-branch code, and never publishes npm', () => {
  assert.match(workflowText, /permissions:\s*\n  contents: read/);
  assert.match(workflowText, /permissions:[\s\S]*actions: read[\s\S]*contents: write/);
  assert.doesNotMatch(workflowText, /id-token: write/);
  assert.match(workflowText, /cancel-in-progress: false/);
  assert.match(workflowText, /ref: \$\{\{ github\.event\.repository\.default_branch \}\}/);
  assert.match(workflowText, /GitHub release recovery must run from the default branch code/);
  assert.doesNotMatch(workflowText, /npm publish|publish-npm\.mjs --publish/);
  assert.match(ciWorkflowText, new RegExp(`actions/setup-node@${SETUP_NODE_ACTION_SHA}`));
  assert.match(ciWorkflowText, /node --test scripts\/package-github-release-test\.mjs/);
});

test('canonical release asset names and package definitions match the public contract', () => {
  assert.deepEqual(canonicalReleaseAssetNames(RELEASE_TAG).sort(), EXPECTED_ASSET_NAMES);
  assert.deepEqual(Object.keys(PACKAGE_DEFINITIONS).sort(), [
    'linux-x64',
    'macos',
    'macos-x64',
    'windows'
  ]);
});

test('verifySourceRun accepts a successful release-npm run and derives the immutable version', () => {
  assert.deepEqual(verifySourceRun(baseValidSourceRun(), { releaseTag: RELEASE_TAG }), {
    releaseTag: RELEASE_TAG,
    npmVersion: NPM_VERSION,
    sourceRunId: SOURCE_RUN_ID,
    sourceRunAttempt: SOURCE_RUN_ATTEMPT,
    headSha: HEAD_SHA
  });
});

test('verifySourceRun rejects malformed tags and mismatched workflow metadata', () => {
  assert.throws(
    () => verifySourceRun(baseValidSourceRun(), { releaseTag: 'release-0.4.1' }),
    /stable vX\.Y\.Z/
  );
  assert.throws(
    () =>
      verifySourceRun(
        { ...baseValidSourceRun(), path: '.github/workflows/release-github.yml' },
        { releaseTag: RELEASE_TAG }
      ),
    /release-npm\.yml/
  );
  assert.throws(
    () =>
      verifySourceRun(
        { ...baseValidSourceRun(), name: 'Publish GitHub Release' },
        { releaseTag: RELEASE_TAG }
      ),
    /Release npm packages/
  );
  assert.throws(
    () =>
      verifySourceRun(
        { ...baseValidSourceRun(), event: 'workflow_dispatch' },
        { releaseTag: RELEASE_TAG }
      ),
    /event push/
  );
  assert.throws(
    () =>
      verifySourceRun(
        { ...baseValidSourceRun(), conclusion: 'failure' },
        { releaseTag: RELEASE_TAG }
      ),
    /completed successfully/
  );
  assert.throws(
    () =>
      verifySourceRun(
        { ...baseValidSourceRun(), head_branch: 'v0.4.2' },
        { releaseTag: RELEASE_TAG }
      ),
    /source run tag/
  );
  assert.throws(
    () => verifySourceRun({ ...baseValidSourceRun(), run_attempt: 0 }, { releaseTag: RELEASE_TAG }),
    /source run attempt/
  );
});

test('verifyAnnotatedTag rejects lightweight, unsigned, or retargeted tags', () => {
  assert.doesNotThrow(() =>
    verifyAnnotatedTag(baseTagRef(), baseTagObject(), {
      releaseTag: RELEASE_TAG,
      expectedHeadSha: HEAD_SHA
    })
  );
  assert.throws(
    () =>
      verifyAnnotatedTag(
        { ...baseTagRef(), object: { type: 'commit', sha: HEAD_SHA } },
        baseTagObject(),
        { releaseTag: RELEASE_TAG, expectedHeadSha: HEAD_SHA }
      ),
    /annotated/
  );
  assert.throws(
    () =>
      verifyAnnotatedTag(baseTagRef(), { ...baseTagObject(), verification: { verified: false, reason: 'unsigned' } }, {
        releaseTag: RELEASE_TAG,
        expectedHeadSha: HEAD_SHA
      }),
    /GitHub-verified signature/
  );
  assert.throws(
    () =>
      verifyAnnotatedTag(baseTagRef(), { ...baseTagObject(), tag: 'v0.4.2' }, {
        releaseTag: RELEASE_TAG,
        expectedHeadSha: HEAD_SHA
      }),
    /must name tag v0\.4\.1/
  );
  assert.throws(
    () =>
      verifyAnnotatedTag(baseTagRef(), { ...baseTagObject(), object: { type: 'tree', sha: HEAD_SHA } }, {
        releaseTag: RELEASE_TAG,
        expectedHeadSha: HEAD_SHA
      }),
    /target a commit/
  );
  assert.throws(
    () =>
      verifyAnnotatedTag(baseTagRef(), { ...baseTagObject(), object: { type: 'commit', sha: 'f'.repeat(40) } }, {
        releaseTag: RELEASE_TAG,
        expectedHeadSha: HEAD_SHA
      }),
    /exact source run commit/
  );
});

test('verifyReleaseSource requires the tagged commit to stay on origin/main and a stable source date epoch', () => {
  assert.deepEqual(
    verifyReleaseSource({
      releaseTag: RELEASE_TAG,
      sourceRun: baseValidSourceRun(),
      tagRef: baseTagRef(),
      tagObject: baseTagObject(),
      localTagObjectSha: baseTagRef().object.sha,
      localHeadSha: HEAD_SHA,
      commitContainedInMain: true,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    }),
    {
      releaseTag: RELEASE_TAG,
      npmVersion: NPM_VERSION,
      sourceRunId: SOURCE_RUN_ID,
      sourceRunAttempt: SOURCE_RUN_ATTEMPT,
      tagObjectSha: baseTagRef().object.sha,
      headSha: HEAD_SHA,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    }
  );
  assert.throws(
    () =>
      verifyReleaseSource({
        releaseTag: RELEASE_TAG,
        sourceRun: baseValidSourceRun(),
        tagRef: baseTagRef(),
        tagObject: baseTagObject(),
        localTagObjectSha: '2'.repeat(40),
        localHeadSha: HEAD_SHA,
        commitContainedInMain: true,
        sourceDateEpoch: SOURCE_DATE_EPOCH
      }),
    /exact GitHub-verified tag object/
  );
  assert.throws(
    () =>
      verifyReleaseSource({
        releaseTag: RELEASE_TAG,
        sourceRun: baseValidSourceRun(),
        tagRef: baseTagRef(),
        tagObject: baseTagObject(),
        localTagObjectSha: baseTagRef().object.sha,
        localHeadSha: 'f'.repeat(40),
        commitContainedInMain: true,
        sourceDateEpoch: SOURCE_DATE_EPOCH
      }),
    /exact source run commit/
  );
  assert.throws(
    () =>
      verifyReleaseSource({
        releaseTag: RELEASE_TAG,
        sourceRun: baseValidSourceRun(),
        tagRef: baseTagRef(),
        tagObject: baseTagObject(),
        localTagObjectSha: baseTagRef().object.sha,
        localHeadSha: HEAD_SHA,
        commitContainedInMain: false,
        sourceDateEpoch: SOURCE_DATE_EPOCH
      }),
    /origin\/main/
  );
  assert.throws(
    () =>
      verifyReleaseSource({
        releaseTag: RELEASE_TAG,
        sourceRun: baseValidSourceRun(),
        tagRef: baseTagRef(),
        tagObject: baseTagObject(),
        localTagObjectSha: baseTagRef().object.sha,
        localHeadSha: HEAD_SHA,
        commitContainedInMain: true,
        sourceDateEpoch: 'bad'
      }),
    /SOURCE_DATE_EPOCH/
  );
});

test('resolveReleaseSource uses the selected run attempt metadata and preserves the verified tag identity', async () => {
  const selectedAttempt = 3;
  const requestedRepository = 'OpenCoven/coven';
  const calls = [];
  const expectedResult = {
    releaseTag: RELEASE_TAG,
    npmVersion: NPM_VERSION,
    sourceRunId: SOURCE_RUN_ID,
    sourceRunAttempt: selectedAttempt,
    tagObjectSha: baseTagRef().object.sha,
    headSha: HEAD_SHA,
    sourceDateEpoch: SOURCE_DATE_EPOCH
  };
  const result = await resolveReleaseSource({
    repository: requestedRepository,
    releaseTag: RELEASE_TAG,
    sourceRunId: SOURCE_RUN_ID,
    sourceRunAttempt: String(selectedAttempt),
    ghApi: async (endpoint) => {
      calls.push(endpoint);
      if (endpoint === `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}`) {
        return { ...baseValidSourceRun(), run_attempt: selectedAttempt };
      }
      if (endpoint === `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}/attempts/${selectedAttempt}`) {
        return { ...baseValidSourceRun(), run_attempt: selectedAttempt };
      }
      if (endpoint === `/repos/${requestedRepository}/git/ref/tags/${encodeURIComponent(RELEASE_TAG)}`) {
        return baseTagRef();
      }
      if (endpoint === `/repos/${requestedRepository}/git/tags/${baseTagRef().object.sha}`) {
        return baseTagObject();
      }
      throw new Error(`unexpected endpoint ${endpoint}`);
    },
    git: {
      verifyLocalTagState(releaseTag) {
        assert.equal(releaseTag, RELEASE_TAG);
        return {
          localTagObjectSha: baseTagRef().object.sha,
          localHeadSha: HEAD_SHA,
          commitContainedInMain: true,
          sourceDateEpoch: SOURCE_DATE_EPOCH
        };
      }
    }
  });
  assert.deepEqual(result, expectedResult);
  assert.deepEqual(calls, [
    `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}`,
    `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}/attempts/${selectedAttempt}`,
    `/repos/${requestedRepository}/git/ref/tags/${encodeURIComponent(RELEASE_TAG)}`,
    `/repos/${requestedRepository}/git/tags/${baseTagRef().object.sha}`
  ]);
});

test('resolveReleaseSource rejects rerun ambiguity before any artifact download can rely on run-id only', async () => {
  const requestedRepository = 'OpenCoven/coven';
  const selectedAttempt = 1;
  const latestAttempt = 2;
  const calls = [];
  await assert.rejects(
    () =>
      resolveReleaseSource({
        repository: requestedRepository,
        releaseTag: RELEASE_TAG,
        sourceRunId: SOURCE_RUN_ID,
        sourceRunAttempt: String(selectedAttempt),
        ghApi: async (endpoint) => {
          calls.push(endpoint);
          if (endpoint === `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}`) {
            return { ...baseValidSourceRun(), run_attempt: latestAttempt };
          }
          if (endpoint === `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}/attempts/${selectedAttempt}`) {
            return { ...baseValidSourceRun(), run_attempt: selectedAttempt };
          }
          throw new Error(`unexpected endpoint ${endpoint}`);
        }
      }),
    /latest run attempt 2 does not match selected attempt 1[\s\S]*run-id only/i
  );
  assert.deepEqual(calls, [
    `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}`,
    `/repos/${requestedRepository}/actions/runs/${SOURCE_RUN_ID}/attempts/${selectedAttempt}`
  ]);
});

test('verifyNpmRegistrySignatures writes an isolated cross-platform exact-version audit context and invokes real npm audit signatures', () => {
  withScratchDir('npm-signatures-audit', (scratchDir) => {
    const auditDir = path.join(scratchDir, 'audit');
    const calls = [];
    const result = verifyNpmRegistrySignatures({
      npmVersion: NPM_VERSION,
      auditDir,
      commandRunner(command, args, options = {}) {
        calls.push({ command, args, cwd: options.cwd });
        if (args[0] === 'install') {
          writeSignatureAuditLockfile(auditDir);
        }
      }
    });

    assert.equal(result.auditDir, path.resolve(auditDir));
    assert.deepEqual(result.packageNames, RELEASE_PACKAGES);
    assert.equal(result.npmVersion, NPM_VERSION);
    assert.deepEqual(
      JSON.parse(readFileSync(path.join(auditDir, 'package.json'), 'utf8')),
      {
        name: 'opencoven-release-npm-signatures-audit',
        private: true,
        version: '0.0.0',
        dependencies: releasePackageVersionMap()
      }
    );
    assert.deepEqual(calls, [
      {
        command: 'npm',
        args: ['install', '--package-lock-only', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      },
      {
        command: 'npm',
        args: ['install', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      },
      {
        command: 'npm',
        args: ['audit', 'signatures'],
        cwd: path.resolve(auditDir)
      }
    ]);
  });
});

test('verifyNpmRegistrySignatures rejects extra root dependencies after the full install rewrites package-lock.json', () => {
  withScratchDir('npm-signatures-audit-extra-root-postinstall', (scratchDir) => {
    const auditDir = path.join(scratchDir, 'audit');
    const calls = [];
    assert.throws(
      () =>
        verifyNpmRegistrySignatures({
          npmVersion: NPM_VERSION,
          auditDir,
          commandRunner(command, args, options = {}) {
            calls.push({ command, args, cwd: options.cwd });
            if (args[0] === 'install' && args.includes('--package-lock-only')) {
              writeSignatureAuditLockfile(auditDir);
              return;
            }
            if (args[0] === 'install') {
              writeSignatureAuditLockfile(auditDir, {
                rootDependencies: {
                  ...releasePackageVersionMap(),
                  'left-pad': '1.3.0'
                }
              });
            }
          }
        }),
      /left-pad/
    );
    assert.deepEqual(calls, [
      {
        command: 'npm',
        args: ['install', '--package-lock-only', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      },
      {
        command: 'npm',
        args: ['install', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      }
    ]);
  });
});

test('verifyNpmRegistrySignatures fails closed before auditing when package-lock generation fails', () => {
  withScratchDir('npm-signatures-audit-fail', (scratchDir) => {
    const auditDir = path.join(scratchDir, 'audit');
    const calls = [];
    assert.throws(
      () =>
        verifyNpmRegistrySignatures({
          npmVersion: NPM_VERSION,
          auditDir,
          commandRunner(command, args) {
            calls.push([command, ...args].join(' '));
            throw new Error('npm install failed');
          }
        }),
      /npm install failed/
    );
    assert.deepEqual(calls, [
      'npm install --package-lock-only --ignore-scripts --force --no-audit --no-fund'
    ]);
  });
});

test('verifyNpmRegistrySignatures fails closed when a native package is absent from package-lock.json', () => {
  withScratchDir('npm-signatures-audit-missing-native', (scratchDir) => {
    const auditDir = path.join(scratchDir, 'audit');
    const calls = [];
    const missingPackage = '@opencoven/cli-windows';
    assert.throws(
      () =>
        verifyNpmRegistrySignatures({
          npmVersion: NPM_VERSION,
          auditDir,
          commandRunner(command, args, options = {}) {
            calls.push({ command, args, cwd: options.cwd });
            if (args[0] === 'install') {
              writeSignatureAuditLockfile(auditDir, {
                resolvedPackages: RELEASE_PACKAGES.filter((packageName) => packageName !== missingPackage)
              });
            }
          }
        }),
      new RegExp(`package-lock\\.json.*${missingPackage}`)
    );
    assert.deepEqual(calls, [
      {
        command: 'npm',
        args: ['install', '--package-lock-only', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      }
    ]);
  });
});

test('verifyNpmRegistrySignatures rejects wrong-version package-lock entries before auditing', () => {
  withScratchDir('npm-signatures-audit-wrong-version', (scratchDir) => {
    const auditDir = path.join(scratchDir, 'audit');
    const calls = [];
    const wrongPackage = '@opencoven/cli-macos';
    assert.throws(
      () =>
        verifyNpmRegistrySignatures({
          npmVersion: NPM_VERSION,
          auditDir,
          commandRunner(command, args, options = {}) {
            calls.push({ command, args, cwd: options.cwd });
            if (args[0] === 'install') {
              writeSignatureAuditLockfile(auditDir, {
                versionOverrides: {
                  [wrongPackage]: '9.9.9'
                }
              });
            }
          }
        }),
      new RegExp(`${wrongPackage}.*0\\.4\\.1`)
    );
    assert.deepEqual(calls, [
      {
        command: 'npm',
        args: ['install', '--package-lock-only', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      }
    ]);
  });
});

test('verifyNpmRegistrySignatures rejects package-lock entries missing resolved tarball URLs before auditing', () => {
  withScratchDir('npm-signatures-audit-missing-resolved', (scratchDir) => {
    const auditDir = path.join(scratchDir, 'audit');
    const calls = [];
    const wrongPackage = '@opencoven/cli-macos';
    assert.throws(
      () =>
        verifyNpmRegistrySignatures({
          npmVersion: NPM_VERSION,
          auditDir,
          commandRunner(command, args, options = {}) {
            calls.push({ command, args, cwd: options.cwd });
            if (args[0] === 'install') {
              writeSignatureAuditLockfile(auditDir, {
                packageEntryOverrides: {
                  [wrongPackage]: {
                    resolved: undefined
                  }
                }
              });
            }
          }
        }),
      new RegExp(`package-lock\\.json entry node_modules/${wrongPackage}.*missing a resolved tarball URL`)
    );
    assert.deepEqual(calls, [
      {
        command: 'npm',
        args: ['install', '--package-lock-only', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
        cwd: path.resolve(auditDir)
      }
    ]);
  });
});

test('verifyPackageProvenance accepts the real npm attestation shape for every release package', async () => {
  await assert.doesNotReject(async () => {
    await Promise.all(
      RELEASE_PACKAGES.map((packageName, index) => {
        const tarballBytes = Buffer.from(`${packageName} tarball fixture ${index}`);
        return verifyPackageProvenance({
          packageName,
          npmVersion: NPM_VERSION,
          releaseTag: RELEASE_TAG,
          headSha: HEAD_SHA,
          sourceRunId: SOURCE_RUN_ID,
          sourceRunAttempt: SOURCE_RUN_ATTEMPT,
          packageMetadata: makePackageMetadata(packageName, integrityFor(tarballBytes)),
          attestationDocument: buildAttestations({
            packageName,
            subjectDigest: createHash('sha512').update(tarballBytes).digest('hex')
          })
        });
      })
    );
  });
});

test('verifyPackageProvenance rejects malformed or mismatched attestation payloads, workflow provenance, and GitHub trusted publisher metadata', async () => {
  const packageName = '@opencoven/cli';
  const tarballBytes = Buffer.from('npm package tarball fixture');
  const goodDigest = createHash('sha512').update(tarballBytes).digest('hex');
  const metadata = makePackageMetadata(packageName, integrityFor(tarballBytes));
  const base = buildAttestations({ packageName, subjectDigest: goodDigest });
  const cases = [
    {
      name: 'missing trusted publisher attestation',
      attestationDocument: { attestations: base.attestations.slice(1) },
      error: /trusted publisher/
    },
    {
      name: 'missing slsa attestation',
      attestationDocument: { attestations: base.attestations.slice(0, 1) },
      error: /exactly one npm SLSA provenance attestation/
    },
    {
      name: 'malformed slsa dsse payload',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        slsaPayloadBase64: Buffer.from('{not valid json').toString('base64')
      }),
      error: /not valid JSON/
    },
    {
      name: 'duplicate outer-labeled slsa attestations',
      attestationDocument: {
        attestations: [
          ...base.attestations,
          {
            predicateType: 'https://slsa.dev/provenance/v1',
            bundle: {
              dsseEnvelope: {
                payload: base.attestations[1].bundle.dsseEnvelope.payload
              }
            }
          }
        ]
      },
      error: /exactly one npm SLSA provenance attestation/
    },
    {
      name: 'missing inner slsa predicate type',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        slsaPayloadBase64: Buffer.from(
          JSON.stringify({
            ...JSON.parse(Buffer.from(base.attestations[1].bundle.dsseEnvelope.payload, 'base64').toString('utf8')),
            predicateType: undefined
          })
        ).toString('base64')
      }),
      error: /decoded SLSA predicateType must be/
    },
    {
      name: 'mismatched inner slsa predicate type',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        slsaPayloadBase64: Buffer.from(
          JSON.stringify({
            ...JSON.parse(Buffer.from(base.attestations[1].bundle.dsseEnvelope.payload, 'base64').toString('utf8')),
            predicateType: 'https://github.com/npm/attestation/tree/main/specs/publish/v0.1'
          })
        ).toString('base64')
      }),
      error: /decoded SLSA predicateType must be/
    },
    {
      name: 'legacy percent-encoded slash subject',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        subjectName: `pkg:npm/${encodeURIComponent(packageName)}@${NPM_VERSION}`
      }),
      error: /missing subject/
    },
    {
      name: 'digest mismatch',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: '0'.repeat(128)
      }),
      error: /dist\.integrity/
    },
    {
      name: 'repository mismatch',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        repository: 'https://github.com/OpenCoven/not-coven'
      }),
      error: /provenance repository/
    },
    {
      name: 'workflow path mismatch',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        workflowPathValue: '.github/workflows/release-github.yml'
      }),
      error: /release-npm\.yml/
    },
    {
      name: 'tag ref mismatch',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        workflowRef: 'refs/tags/v0.4.2'
      }),
      error: /refs\/tags\/v0\.4\.1/
    },
    {
      name: 'git commit mismatch',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        gitCommit: 'f'.repeat(40)
      }),
      error: /gitCommit must be/
    },
    {
      name: 'run attempt mismatch',
      attestationDocument: buildAttestations({
        packageName,
        subjectDigest: goodDigest,
        invocationId: `https://github.com/OpenCoven/coven/actions/runs/${SOURCE_RUN_ID}/attempts/2`
      }),
      error: /run attempt/
    },
    {
      name: 'missing GitHub trusted publisher metadata',
      packageMetadata: makePackageMetadata(packageName, integrityFor(tarballBytes), {
        npmUser: trustedPublisherMetadata({ trustedPublisher: null })
      }),
      error: /trusted publisher metadata/
    },
    {
      name: 'wrong trusted publisher id',
      packageMetadata: makePackageMetadata(packageName, integrityFor(tarballBytes), {
        npmUser: trustedPublisherMetadata({ trustedPublisher: { id: 'gitlab' } })
      }),
      error: /GitHub trusted publisher/
    }
  ];

  for (const { name, attestationDocument, packageMetadata, error } of cases) {
    await assert.rejects(
      () =>
        verifyPackageProvenance({
          packageName,
          npmVersion: NPM_VERSION,
          releaseTag: RELEASE_TAG,
          headSha: HEAD_SHA,
          sourceRunId: SOURCE_RUN_ID,
          sourceRunAttempt: SOURCE_RUN_ATTEMPT,
          packageMetadata: packageMetadata ?? metadata,
          attestationDocument: attestationDocument ?? base
        }),
      error,
      name
    );
  }
});

test('verifyPackageProvenance rejects malformed sha512 SRI digests before comparing attestations', async () => {
  const packageName = '@opencoven/cli';
  const tarballBytes = Buffer.from('npm package tarball fixture');
  const goodDigest = createHash('sha512').update(tarballBytes).digest('hex');
  const attestationDocument = buildAttestations({ packageName, subjectDigest: goodDigest });
  const canonicalIntegrity = integrityFor(tarballBytes);
  const cases = [
    {
      name: 'empty digest',
      integrity: 'sha512-'
    },
    {
      name: 'malformed base64',
      integrity: 'sha512-***'
    },
    {
      name: 'noncanonical base64 without required padding',
      integrity: canonicalIntegrity.replace(/=+$/, '')
    },
    {
      name: 'wrong-length digest that decodes to 63 bytes',
      integrity: `sha512-${Buffer.alloc(63).toString('base64')}`
    },
    {
      name: 'wrong-length digest that decodes to 65 bytes',
      integrity: `sha512-${Buffer.alloc(65).toString('base64')}`
    }
  ];

  for (const { name, integrity } of cases) {
    await assert.rejects(
      () =>
        verifyPackageProvenance({
          packageName,
          npmVersion: NPM_VERSION,
          releaseTag: RELEASE_TAG,
          headSha: HEAD_SHA,
          sourceRunId: SOURCE_RUN_ID,
          sourceRunAttempt: SOURCE_RUN_ATTEMPT,
          packageMetadata: makePackageMetadata(packageName, integrity),
          attestationDocument
        }),
      /canonical sha512 SRI string/,
      name
    );
  }
});

test('packageGitHubRelease emits deterministic canonical assets with normalized archive metadata', () => {
  withScratchDir('canonical-package', (scratchDir) => {
    const artifactsDir = path.join(scratchDir, 'artifacts');
    const outputDir = path.join(scratchDir, 'out');
    cpSync(fixtureRoot, artifactsDir, { recursive: true });

    const produced = packageGitHubRelease({
      releaseTag: RELEASE_TAG,
      artifactsDir,
      outputDir,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    });

    assert.deepEqual(readdirSync(outputDir).sort(), EXPECTED_ASSET_NAMES);
    assert.deepEqual(produced.assetNames.sort(), EXPECTED_ASSET_NAMES);

    const checksums = readFileSync(path.join(outputDir, 'SHA256SUMS'), 'utf8');
    assert.doesNotThrow(() => assertChecksumManifest(checksums, EXPECTED_ARCHIVES));

    const macosArm = parseTarGzEntries(
      readFileSync(path.join(outputDir, 'coven-v0.4.1-macos-aarch64.tar.gz'))
    );
    assert.equal(macosArm.gzipMtime, SOURCE_DATE_EPOCH);
    assertRootOnlyTarEntries(macosArm.entries, ['coven', 'coven-afs-serve']);

    const linux = parseTarGzEntries(
      readFileSync(path.join(outputDir, 'coven-v0.4.1-linux-x64.tar.gz'))
    );
    assert.equal(linux.gzipMtime, SOURCE_DATE_EPOCH);
    assertRootOnlyTarEntries(linux.entries, ['coven']);

    const windows = parseZipEntries(
      readFileSync(path.join(outputDir, 'coven-v0.4.1-windows-x64.zip'))
    );
    const zipTimestamp = expectedZipTimestamp(SOURCE_DATE_EPOCH);
    assert.deepEqual(windows.map((entry) => entry.name), ['coven.exe']);
    assert.equal(windows[0].method, 0, 'zip output should use stable store mode');
    assert.equal((windows[0].externalAttributes >>> 16) & 0o777, 0o755);
    assert.equal(windows[0].dosDate, zipTimestamp.dosDate);
    assert.equal(windows[0].dosTime, zipTimestamp.dosTime);
  });
});

test('packageGitHubRelease is byte-identical across repeated runs with identical fixtures', () => {
  withScratchDir('deterministic-package', (scratchDir) => {
    const runOnce = (name) => {
      const artifactsDir = path.join(scratchDir, `${name}-artifacts`);
      const outputDir = path.join(scratchDir, `${name}-out`);
      cpSync(fixtureRoot, artifactsDir, { recursive: true });
      packageGitHubRelease({
        releaseTag: RELEASE_TAG,
        artifactsDir,
        outputDir,
        sourceDateEpoch: SOURCE_DATE_EPOCH
      });
      return Object.fromEntries(
        readdirSync(outputDir)
          .sort()
          .map((assetName) => [assetName, readFileSync(path.join(outputDir, assetName))])
      );
    };

    const first = runOnce('first');
    const second = runOnce('second');
    assert.deepEqual(Object.keys(first), Object.keys(second));
    for (const assetName of Object.keys(first)) {
      assert.equal(
        sha256Hex(first[assetName]),
        sha256Hex(second[assetName]),
        `${assetName} should be byte-identical across reruns`
      );
      assert.deepEqual(
        first[assetName],
        second[assetName],
        `${assetName} bytes should not drift across reruns`
      );
    }
  });
});

test('packageGitHubRelease rejects missing or extra source artifact files', () => {
  withScratchDir('invalid-artifacts', (scratchDir) => {
    const missingArtifactsDir = path.join(scratchDir, 'missing');
    cpSync(fixtureRoot, missingArtifactsDir, { recursive: true });
    rmSync(path.join(missingArtifactsDir, 'coven-macos', 'coven-afs-serve'));
    assert.throws(
      () =>
        packageGitHubRelease({
          releaseTag: RELEASE_TAG,
          artifactsDir: missingArtifactsDir,
          outputDir: path.join(scratchDir, 'missing-out'),
          sourceDateEpoch: SOURCE_DATE_EPOCH
        }),
      /missing required file/
    );

    const extraArtifactsDir = path.join(scratchDir, 'extra');
    cpSync(fixtureRoot, extraArtifactsDir, { recursive: true });
    writeFileSync(path.join(extraArtifactsDir, 'coven-linux-x64', 'extra.txt'), 'unexpected\n');
    assert.throws(
      () =>
        packageGitHubRelease({
          releaseTag: RELEASE_TAG,
          artifactsDir: extraArtifactsDir,
          outputDir: path.join(scratchDir, 'extra-out'),
          sourceDateEpoch: SOURCE_DATE_EPOCH
        }),
      /unexpected files/
    );
  });
});

test('assertChecksumManifest rejects self entries, duplicates, path-qualified names, missing assets, and non-lexical order', () => {
  assert.throws(() => assertChecksumManifest('', EXPECTED_ARCHIVES), /exactly four non-empty entries/);
  assert.throws(
    () =>
      assertChecksumManifest(
        [
          `${'a'.repeat(64)}  ${EXPECTED_ARCHIVES[0]}`,
          `${'b'.repeat(64)}  SHA256SUMS`,
          `${'c'.repeat(64)}  ${EXPECTED_ARCHIVES[1]}`,
          `${'d'.repeat(64)}  ${EXPECTED_ARCHIVES[2]}`
        ].join('\n'),
        EXPECTED_ARCHIVES
      ),
    /must not checksum SHA256SUMS/
  );
  assert.throws(
    () =>
      assertChecksumManifest(
        [
          `${'a'.repeat(64)}  ${EXPECTED_ARCHIVES[0]}`,
          `${'b'.repeat(64)}  ${EXPECTED_ARCHIVES[0]}`,
          `${'c'.repeat(64)}  ${EXPECTED_ARCHIVES[1]}`,
          `${'d'.repeat(64)}  ${EXPECTED_ARCHIVES[2]}`
        ].join('\n'),
        EXPECTED_ARCHIVES
      ),
    /duplicate/
  );
  assert.throws(
    () =>
      assertChecksumManifest(
        [
          `${'a'.repeat(64)}  archives/${EXPECTED_ARCHIVES[0]}`,
          `${'b'.repeat(64)}  ${EXPECTED_ARCHIVES[1]}`,
          `${'c'.repeat(64)}  ${EXPECTED_ARCHIVES[2]}`,
          `${'d'.repeat(64)}  ${EXPECTED_ARCHIVES[3]}`
        ].join('\n'),
        EXPECTED_ARCHIVES
      ),
    /bare filenames/
  );
  assert.throws(
    () =>
      assertChecksumManifest(
        [
          `${'a'.repeat(64)}  ${EXPECTED_ARCHIVES[1]}`,
          `${'b'.repeat(64)}  ${EXPECTED_ARCHIVES[0]}`,
          `${'c'.repeat(64)}  ${EXPECTED_ARCHIVES[2]}`,
          `${'d'.repeat(64)}  ${EXPECTED_ARCHIVES[3]}`
        ].join('\n'),
        EXPECTED_ARCHIVES
      ),
    /lexically sorted/
  );
  assert.throws(
    () =>
      assertChecksumManifest(
        [
          `${'a'.repeat(64)}  ${EXPECTED_ARCHIVES[0]}`,
          `${'b'.repeat(64)}  ${EXPECTED_ARCHIVES[1]}`,
          `${'c'.repeat(64)}  ${EXPECTED_ARCHIVES[2]}`,
          `${'d'.repeat(64)}  z-extra.tar.gz`
        ].join('\n'),
        EXPECTED_ARCHIVES
      ),
    /exact canonical asset names/
  );
});

test('captureCommandOutputToFile writes stdout larger than spawnSync maxBuffer directly to disk', () => {
  withScratchDir('capture-command-output', (scratchDir) => {
    const outputPath = path.join(scratchDir, 'large-stdout.bin');
    const byteCount = (1024 * 1024) + 4096;
    captureCommandOutputToFile(
      process.execPath,
      ['-e', `process.stdout.write(Buffer.alloc(${byteCount}, 0x61));`],
      { filePath: outputPath }
    );
    const outputBytes = readFileSync(outputPath);
    assert.equal(outputBytes.length, byteCount);
    assert.equal(outputBytes.subarray(0, 4).toString('utf8'), 'aaaa');
  });
});

test('syncGitHubRelease revalidates the verified remote tag before creating a missing release', async () => {
  await withScratchDir('sync-create', async (scratchDir) => {
    const artifactsDir = path.join(scratchDir, 'artifacts');
    const outputDir = path.join(scratchDir, 'out');
    cpSync(fixtureRoot, artifactsDir, { recursive: true });
    packageGitHubRelease({
      releaseTag: RELEASE_TAG,
      artifactsDir,
      outputDir,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    });

    const client = fakeReleaseClient();
    const result = await syncGitHubRelease({
      releaseTag: RELEASE_TAG,
      outputDir,
      expectedTagObjectSha: baseTagRef().object.sha,
      expectedHeadSha: HEAD_SHA,
      releaseClient: client
    });

    assert.deepEqual(client.state.revalidated, [
      {
        releaseTag: RELEASE_TAG,
        expectedTagObjectSha: baseTagRef().object.sha,
        expectedHeadSha: HEAD_SHA
      }
    ]);
    assert.deepEqual(client.state.created, [
      { releaseTag: RELEASE_TAG, title: 'Coven v0.4.1', notesFromTag: true, verifyTag: true }
    ]);
    assert.deepEqual(result.uploaded.sort(), EXPECTED_ASSET_NAMES);
    assert.deepEqual(result.skipped, []);
  });
});

test('syncGitHubRelease refuses release creation when the verified remote tag was deleted or replaced', async () => {
  await withScratchDir('sync-create-race', async (scratchDir) => {
    const artifactsDir = path.join(scratchDir, 'artifacts');
    const outputDir = path.join(scratchDir, 'out');
    cpSync(fixtureRoot, artifactsDir, { recursive: true });
    packageGitHubRelease({
      releaseTag: RELEASE_TAG,
      artifactsDir,
      outputDir,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    });

    const cases = [
      {
        name: 'deleted',
        error: /no longer resolves to refs\/tags\/v0\.4\.1/i,
        revalidateTagState() {
          throw new Error(
            `Refusing GitHub release: ${RELEASE_TAG} no longer resolves to refs/tags/${RELEASE_TAG}.`
          );
        }
      },
      {
        name: 'replaced',
        error: /tag object SHA/i,
        revalidateTagState() {
          throw new Error(
            `Refusing GitHub release: ${RELEASE_TAG} tag object SHA changed from ${baseTagRef().object.sha} to ${'2'.repeat(40)}.`
          );
        }
      }
    ];

    for (const { name, error, revalidateTagState } of cases) {
      const client = fakeReleaseClient({ revalidateTagState });
      await assert.rejects(
        () =>
          syncGitHubRelease({
            releaseTag: RELEASE_TAG,
            outputDir,
            expectedTagObjectSha: baseTagRef().object.sha,
            expectedHeadSha: HEAD_SHA,
            releaseClient: client
          }),
        error,
        name
      );
      assert.deepEqual(client.state.created, [], `${name} must not create the release`);
      assert.deepEqual(client.state.uploads, [], `${name} must not upload release assets`);
    }
  });
});

test('syncGitHubRelease skips a fully matching rerun, including >1 MiB assets, without uploading', async () => {
  await withScratchDir('sync-large-match', async (scratchDir) => {
    const artifactsDir = path.join(scratchDir, 'artifacts');
    const outputDir = path.join(scratchDir, 'out');
    cpSync(fixtureRoot, artifactsDir, { recursive: true });

    const windowsDefinition = PACKAGE_DEFINITIONS.windows;
    const windowsBinaryPath = path.join(artifactsDir, windowsDefinition.artifactName, 'coven.exe');
    writeFileSync(
      windowsBinaryPath,
      Buffer.concat([readFileSync(windowsBinaryPath), Buffer.alloc(1_250_000, 0x61)])
    );

    packageGitHubRelease({
      releaseTag: RELEASE_TAG,
      artifactsDir,
      outputDir,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    });

    const windowsArchiveName = windowsDefinition.assetName(NPM_VERSION);
    assert.ok(readFileSync(path.join(outputDir, windowsArchiveName)).length > 1024 * 1024);

    const matchingClient = fakeReleaseClient({
      existingRelease: {
        tagName: RELEASE_TAG,
        assets: EXPECTED_ASSET_NAMES.map((name, index) => ({ id: index + 1, name }))
      },
      assetBytesByName: Object.fromEntries(
        EXPECTED_ASSET_NAMES.map((assetName) => [assetName, readFileSync(path.join(outputDir, assetName))])
      )
    });
    const matchingResult = await syncGitHubRelease({
      releaseTag: RELEASE_TAG,
      outputDir,
      releaseClient: matchingClient
    });

    assert.deepEqual(matchingResult.skipped, EXPECTED_ASSET_NAMES);
    assert.deepEqual(matchingResult.uploaded, []);
    assert.equal(matchingClient.state.uploads.length, 0);
  });
});

test('syncGitHubRelease skips matching assets, uploads only missing ones, and fails closed on mismatches or extras', async () => {
  await withScratchDir('sync-recover', async (scratchDir) => {
    const artifactsDir = path.join(scratchDir, 'artifacts');
    const outputDir = path.join(scratchDir, 'out');
    cpSync(fixtureRoot, artifactsDir, { recursive: true });
    packageGitHubRelease({
      releaseTag: RELEASE_TAG,
      artifactsDir,
      outputDir,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    });

    const checksumBytes = readFileSync(path.join(outputDir, 'SHA256SUMS'));
    const matchingClient = fakeReleaseClient({
      existingRelease: {
        tagName: RELEASE_TAG,
        assets: [{ id: 1, name: 'SHA256SUMS' }]
      },
      assetBytesByName: { SHA256SUMS: checksumBytes }
    });
    const matchingResult = await syncGitHubRelease({
      releaseTag: RELEASE_TAG,
      outputDir,
      releaseClient: matchingClient
    });
    assert.deepEqual(matchingResult.skipped, ['SHA256SUMS']);
    assert.equal(matchingClient.state.uploads.length, EXPECTED_ASSET_NAMES.length - 1);

    const mismatchedClient = fakeReleaseClient({
      existingRelease: {
        tagName: RELEASE_TAG,
        assets: [{ id: 1, name: 'SHA256SUMS' }]
      },
      assetBytesByName: { SHA256SUMS: Buffer.from('wrong checksums\n') }
    });
    await assert.rejects(
      () =>
        syncGitHubRelease({
          releaseTag: RELEASE_TAG,
          outputDir,
          releaseClient: mismatchedClient
        }),
      /observed hash .* expected hash .* delete only the mismatched GitHub asset/i
    );
    assert.equal(mismatchedClient.state.uploads.length, 0);

    const extraClient = fakeReleaseClient({
      existingRelease: {
        tagName: RELEASE_TAG,
        assets: [{ id: 1, name: 'unexpected.txt' }]
      }
    });
    await assert.rejects(
      () =>
        syncGitHubRelease({
          releaseTag: RELEASE_TAG,
          outputDir,
          releaseClient: extraClient
        }),
      /unexpected release assets/
    );

    const duplicateClient = fakeReleaseClient({
      existingRelease: {
        tagName: RELEASE_TAG,
        assets: [
          { id: 1, name: 'SHA256SUMS' },
          { id: 2, name: 'SHA256SUMS' }
        ]
      },
      assetBytesByName: { SHA256SUMS: checksumBytes }
    });
    await assert.rejects(
      () =>
        syncGitHubRelease({
          releaseTag: RELEASE_TAG,
          outputDir,
          releaseClient: duplicateClient
        }),
      /duplicate release assets/
    );
  });
});

test('syncGitHubRelease preflights later mismatches before uploading earlier missing assets', async () => {
  await withScratchDir('sync-preflight-mismatch', async (scratchDir) => {
    const artifactsDir = path.join(scratchDir, 'artifacts');
    const outputDir = path.join(scratchDir, 'out');
    cpSync(fixtureRoot, artifactsDir, { recursive: true });
    packageGitHubRelease({
      releaseTag: RELEASE_TAG,
      artifactsDir,
      outputDir,
      sourceDateEpoch: SOURCE_DATE_EPOCH
    });

    const windowsAssetName = 'coven-v0.4.1-windows-x64.zip';
    const client = fakeReleaseClient({
      existingRelease: {
        tagName: RELEASE_TAG,
        assets: [{ id: 1, name: windowsAssetName }]
      },
      assetBytesByName: {
        [windowsAssetName]: Buffer.from('wrong windows bytes\n')
      }
    });

    await assert.rejects(
      () =>
        syncGitHubRelease({
          releaseTag: RELEASE_TAG,
          outputDir,
          releaseClient: client
        }),
      /asset .*windows.* mismatch/i
    );
    assert.deepEqual(client.state.downloads, [windowsAssetName]);
    assert.equal(client.state.uploads.length, 0);
  });
});
