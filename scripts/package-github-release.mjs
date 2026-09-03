#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { gzipSync } from 'node:zlib';

import { parseReleaseTag } from './release-npm-context.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const WORKFLOW_NAME = 'Release npm packages';
const WORKFLOW_PATH = '.github/workflows/release-npm.yml';
const REPOSITORY_URL = 'https://github.com/OpenCoven/coven';
const NPM_REGISTRY_URL = 'https://registry.npmjs.org';
const TRUSTED_PUBLISHER_PREDICATE =
  'https://github.com/npm/attestation/tree/main/specs/publish/v0.1';
const SLSA_PREDICATE = 'https://slsa.dev/provenance/v1';
const CHECKSUMS_NAME = 'SHA256SUMS';
const ZIP_VERSION = 20;
const ZIP_UTF8_FLAG = 0x0800;
const ZIP_STORE_METHOD = 0;
const TRUSTED_PUBLISHER_NAME = 'GitHub Actions';
const TRUSTED_PUBLISHER_EMAIL = 'npm-oidc-no-reply@github.com';
const TRUSTED_PUBLISHER_ID = 'github';
const AUTOMATIONS_PROTOCOL_FIRST_RELEASE = [0, 4, 4];
const RELEASE_PACKAGES = [
  '@opencoven/cli',
  '@opencoven/cli-linux-x64',
  '@opencoven/cli-windows',
  '@opencoven/cli-macos-x64',
  '@opencoven/cli-macos'
];

export const PACKAGE_DEFINITIONS = {
  macos: {
    artifactName: 'coven-macos',
    expectedFiles: ['coven', 'coven-afs-serve'],
    assetName: (npmVersion) => `coven-v${npmVersion}-macos-aarch64.tar.gz`,
    format: 'tar.gz'
  },
  'macos-x64': {
    artifactName: 'coven-macos-x64',
    expectedFiles: ['coven', 'coven-afs-serve'],
    assetName: (npmVersion) => `coven-v${npmVersion}-macos-x64.tar.gz`,
    format: 'tar.gz'
  },
  'linux-x64': {
    artifactName: 'coven-linux-x64',
    expectedFiles: ['coven'],
    assetName: (npmVersion) => `coven-v${npmVersion}-linux-x64.tar.gz`,
    format: 'tar.gz'
  },
  windows: {
    artifactName: 'coven-windows',
    expectedFiles: ['coven.exe'],
    assetName: (npmVersion) => `coven-v${npmVersion}-windows-x64.zip`,
    format: 'zip'
  }
};

const CRC32_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = (value & 1) === 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table[index] = value >>> 0;
  }
  return table;
})();

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

if (isMainModule()) {
  main().catch((error) => {
    console.error(error?.message ?? String(error));
    process.exit(1);
  });
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  if (!command) {
    throw new Error(
      'Usage: package-github-release.mjs <verify-source-run|verify-source-run-attempt|verify-npm-provenance|verify-npm-signatures|package|sync-release> [--option value ...]'
    );
  }

  const options = parseOptions(args);
  switch (command) {
    case 'verify-source-run': {
      const result = await resolveReleaseSource({
        repository: requiredOption(options, 'repository'),
        releaseTag: requiredOption(options, 'release-tag'),
        sourceRunId: requiredOption(options, 'source-run-id'),
        sourceRunAttempt: requiredOption(options, 'source-run-attempt')
      });
      process.stdout.write(
        [
          `release_tag=${result.releaseTag}`,
          `npm_version=${result.npmVersion}`,
          `head_sha=${result.headSha}`,
          `tag_object_sha=${result.tagObjectSha}`,
          `source_run_id=${result.sourceRunId}`,
          `source_run_attempt=${result.sourceRunAttempt}`,
          `source_date_epoch=${result.sourceDateEpoch}`,
          ''
        ].join('\n')
      );
      return;
    }
    case 'verify-source-run-attempt': {
      await verifyLatestSourceRunAttempt({
        repository: requiredOption(options, 'repository'),
        releaseTag: requiredOption(options, 'release-tag'),
        sourceRunId: requiredOption(options, 'source-run-id'),
        sourceRunAttempt: requiredOption(options, 'source-run-attempt')
      });
      return;
    }
    case 'verify-npm-provenance': {
      await verifyAllPackageProvenance({
        releaseTag: requiredOption(options, 'release-tag'),
        npmVersion: requiredOption(options, 'npm-version'),
        headSha: requiredOption(options, 'head-sha'),
        sourceRunId: requiredOption(options, 'source-run-id'),
        sourceRunAttempt: requiredOption(options, 'source-run-attempt')
      });
      return;
    }
    case 'verify-npm-signatures': {
      verifyNpmRegistrySignatures({
        npmVersion: requiredOption(options, 'npm-version'),
        auditDir: requiredOption(options, 'audit-dir')
      });
      return;
    }
    case 'package': {
      packageGitHubRelease({
        releaseTag: requiredOption(options, 'release-tag'),
        artifactsDir: requiredOption(options, 'artifacts-dir'),
        outputDir: requiredOption(options, 'output-dir'),
        sourceDateEpoch: requiredOption(options, 'source-date-epoch'),
        sourceCommit: options.get('source-commit'),
        protocolBundlePath: options.get('protocol-bundle')
      });
      return;
    }
    case 'sync-release': {
      await syncGitHubRelease({
        releaseTag: requiredOption(options, 'release-tag'),
        outputDir: requiredOption(options, 'output-dir'),
        expectedTagObjectSha: requiredOption(options, 'expected-tag-object-sha'),
        expectedHeadSha: requiredOption(options, 'expected-head-sha'),
        releaseClient: createGhReleaseClient({
          repository: requiredOption(options, 'repository')
        })
      });
      return;
    }
    default:
      throw new Error(`Unknown command ${JSON.stringify(command)}.`);
  }
}

function parseOptions(args) {
  const options = new Map();
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg?.startsWith('--')) {
      throw new Error(`Unexpected argument ${JSON.stringify(arg)}.`);
    }
    const key = arg.slice(2);
    const value = args[index + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`Missing value for --${key}.`);
    }
    options.set(key, value);
    index += 1;
  }
  return options;
}

function requiredOption(options, key) {
  const value = options.get(key);
  if (!value) {
    throw new Error(`Missing required option --${key}.`);
  }
  return value;
}

export function automationsProtocolBundleName(sourceCommit) {
  const normalized = String(sourceCommit).trim();
  if (!/^[0-9a-f]{40}$/.test(normalized)) {
    throw new Error(`Automations protocol source commit must be a lowercase 40-character Git SHA: ${normalized}`);
  }
  return `coven-automations-v1-contract-${normalized}.tar.gz`;
}

function releaseIncludesAutomationsProtocol(releaseTag) {
  const { npmVersion } = parseReleaseTag(releaseTag);
  const version = npmVersion.split('.').map(Number);
  for (let index = 0; index < AUTOMATIONS_PROTOCOL_FIRST_RELEASE.length; index += 1) {
    if (version[index] > AUTOMATIONS_PROTOCOL_FIRST_RELEASE[index]) {
      return true;
    }
    if (version[index] < AUTOMATIONS_PROTOCOL_FIRST_RELEASE[index]) {
      return false;
    }
  }
  return true;
}

export function canonicalReleaseAssetNames(releaseTag, sourceCommit) {
  const { npmVersion } = parseReleaseTag(releaseTag);
  return [
    ...Object.values(PACKAGE_DEFINITIONS).map((definition) => definition.assetName(npmVersion)),
    ...(releaseIncludesAutomationsProtocol(releaseTag)
      ? [automationsProtocolBundleName(sourceCommit)]
      : []),
    CHECKSUMS_NAME
  ];
}

export function verifySourceRun(sourceRun, {
  releaseTag,
  expectedSourceRunId,
  expectedSourceRunAttempt
}) {
  const { npmVersion } = parseReleaseTag(releaseTag);
  const sourceRunId = toPositiveIntegerString(sourceRun?.id, 'source run id');
  const sourceRunAttempt = toPositiveInteger(sourceRun?.run_attempt, 'source run attempt');
  if (expectedSourceRunId && sourceRunId !== toPositiveIntegerString(expectedSourceRunId, 'source run id')) {
    throw new Error(
      `Refusing GitHub release: source run payload ${sourceRunId} does not match requested run ${expectedSourceRunId}.`
    );
  }
  if (
    expectedSourceRunAttempt &&
    sourceRunAttempt !== toPositiveInteger(expectedSourceRunAttempt, 'source run attempt')
  ) {
    throw new Error(
      `Refusing GitHub release: source run attempt payload ${sourceRunAttempt} does not match requested attempt ${expectedSourceRunAttempt}.`
    );
  }
  if (sourceRun?.name !== WORKFLOW_NAME) {
    throw new Error(
      `Refusing GitHub release: source run ${sourceRunId} must be ${WORKFLOW_NAME}, got ${JSON.stringify(sourceRun?.name)}.`
    );
  }
  if (sourceRun?.path !== WORKFLOW_PATH) {
    throw new Error(
      `Refusing GitHub release: source run ${sourceRunId} must use ${WORKFLOW_PATH}, got ${JSON.stringify(sourceRun?.path)}.`
    );
  }
  if (sourceRun?.event !== 'push') {
    throw new Error(
      `Refusing GitHub release: source run ${sourceRunId} must come from event push, got ${JSON.stringify(sourceRun?.event)}.`
    );
  }
  if (sourceRun?.status !== 'completed' || sourceRun?.conclusion !== 'success') {
    throw new Error(
      `Refusing GitHub release: source run ${sourceRunId} must have completed successfully, got status=${JSON.stringify(sourceRun?.status)} conclusion=${JSON.stringify(sourceRun?.conclusion)}.`
    );
  }
  if (sourceRun?.head_branch !== releaseTag) {
    throw new Error(
      `Refusing GitHub release: source run tag ${JSON.stringify(sourceRun?.head_branch)} does not match requested tag ${releaseTag}.`
    );
  }
  if (!isSha(sourceRun?.head_sha)) {
    throw new Error(
      `Refusing GitHub release: source run ${sourceRunId} returned malformed head SHA ${JSON.stringify(sourceRun?.head_sha)}.`
    );
  }
  return {
    releaseTag,
    npmVersion,
    sourceRunId,
    sourceRunAttempt,
    headSha: sourceRun.head_sha
  };
}

export function verifyAnnotatedTag(tagRef, tagObject, { releaseTag, expectedHeadSha }) {
  if (tagRef?.ref !== `refs/tags/${releaseTag}`) {
    throw new Error(
      `Refusing GitHub release: expected tag ref refs/tags/${releaseTag}, got ${JSON.stringify(tagRef?.ref)}.`
    );
  }
  if (tagRef?.object?.type !== 'tag') {
    throw new Error(
      `Refusing GitHub release: ${releaseTag} must be an annotated tag, not ${JSON.stringify(tagRef?.object?.type)}.`
    );
  }
  if (!isSha(tagRef?.object?.sha)) {
    throw new Error(
      `Refusing GitHub release: ${releaseTag} must resolve to a GitHub tag object SHA, got ${JSON.stringify(tagRef?.object?.sha)}.`
    );
  }
  if (tagObject?.tag !== releaseTag) {
    throw new Error(
      `Refusing GitHub release: GitHub tag API payload must name tag ${releaseTag}, got ${JSON.stringify(tagObject?.tag)}.`
    );
  }
  if (tagObject?.verification?.verified !== true) {
    throw new Error(
      `Refusing GitHub release: ${releaseTag} must have a GitHub-verified signature (reason=${tagObject?.verification?.reason ?? 'unknown'}).`
    );
  }
  if (tagObject?.object?.type !== 'commit') {
    throw new Error(
      `Refusing GitHub release: ${releaseTag} must target a commit, not ${JSON.stringify(tagObject?.object?.type)}.`
    );
  }
  if (tagObject?.object?.sha !== expectedHeadSha) {
    throw new Error(
      `Refusing GitHub release: ${releaseTag} must resolve to the exact source run commit ${expectedHeadSha}, got ${tagObject?.object?.sha}.`
    );
  }
  return {
    releaseTag,
    tagObjectSha: tagRef.object.sha,
    headSha: expectedHeadSha
  };
}

export function verifyReleaseSource({
  releaseTag,
  sourceRun,
  tagRef,
  tagObject,
  localTagObjectSha,
  localHeadSha,
  commitContainedInMain,
  sourceDateEpoch
}) {
  const sourceContext = verifySourceRun(sourceRun, { releaseTag });
  const tagContext = verifyAnnotatedTag(tagRef, tagObject, {
    releaseTag,
    expectedHeadSha: sourceContext.headSha
  });
  if (String(localTagObjectSha ?? '').trim() !== tagContext.tagObjectSha) {
    throw new Error(
      `Refusing GitHub release: local tag ${releaseTag} must resolve to the exact GitHub-verified tag object ${tagContext.tagObjectSha}, got ${localTagObjectSha}.`
    );
  }
  if (String(localHeadSha ?? '').trim() !== sourceContext.headSha) {
    throw new Error(
      `Refusing GitHub release: local tag ${releaseTag} must resolve to the exact source run commit ${sourceContext.headSha}, got ${localHeadSha}.`
    );
  }
  if (commitContainedInMain !== true) {
    throw new Error(
      `Refusing GitHub release: tagged commit ${sourceContext.headSha} is not contained in origin/main.`
    );
  }
  const normalizedSourceDateEpoch = toNonNegativeInteger(sourceDateEpoch, 'SOURCE_DATE_EPOCH');
  return {
    ...sourceContext,
    tagObjectSha: tagContext.tagObjectSha,
    sourceDateEpoch: normalizedSourceDateEpoch
  };
}

export function assertRemoteTagMatchesVerifiedContext({
  releaseTag,
  expectedTagObjectSha,
  expectedHeadSha,
  tagRef,
  tagObject
}) {
  if (!isSha(expectedTagObjectSha)) {
    throw new Error(
      `Refusing GitHub release: expected verified tag object SHA for ${releaseTag}, got ${JSON.stringify(expectedTagObjectSha)}.`
    );
  }
  if (!isSha(expectedHeadSha)) {
    throw new Error(
      `Refusing GitHub release: expected verified source commit SHA for ${releaseTag}, got ${JSON.stringify(expectedHeadSha)}.`
    );
  }
  const remoteTagContext = verifyAnnotatedTag(tagRef, tagObject, {
    releaseTag,
    expectedHeadSha
  });
  if (remoteTagContext.tagObjectSha !== expectedTagObjectSha) {
    throw new Error(
      `Refusing GitHub release: ${releaseTag} tag object SHA changed from ${expectedTagObjectSha} to ${remoteTagContext.tagObjectSha}.`
    );
  }
  return remoteTagContext;
}

function assertLatestSourceRunAttempt(latestSourceRun, {
  releaseTag,
  expectedSourceRunId,
  expectedSourceRunAttempt
}) {
  const normalizedSourceRunId = toPositiveIntegerString(expectedSourceRunId, 'source run id');
  const normalizedSourceRunAttempt = toPositiveIntegerString(expectedSourceRunAttempt, 'source run attempt');
  const latestRunAttempt = toPositiveInteger(latestSourceRun?.run_attempt, 'latest source run attempt');
  if (latestRunAttempt !== toPositiveInteger(normalizedSourceRunAttempt, 'source run attempt')) {
    throw new Error(
      `Refusing GitHub release: source run ${normalizedSourceRunId} latest run attempt ${latestRunAttempt} does not match selected attempt ${normalizedSourceRunAttempt}; actions/download-artifact is run-id only, so old-attempt artifacts are ambiguous or unavailable after a rerun.`
    );
  }
  return verifySourceRun(latestSourceRun, {
    releaseTag,
    expectedSourceRunId: normalizedSourceRunId,
    expectedSourceRunAttempt: normalizedSourceRunAttempt
  });
}

export async function verifyLatestSourceRunAttempt({
  repository,
  releaseTag,
  sourceRunId,
  sourceRunAttempt,
  ghApi = ghApiJson
}) {
  const normalizedSourceRunId = toPositiveIntegerString(sourceRunId, 'source run id');
  const latestSourceRun = await ghApi(`/repos/${repository}/actions/runs/${normalizedSourceRunId}`);
  return assertLatestSourceRunAttempt(latestSourceRun, {
    releaseTag,
    expectedSourceRunId: normalizedSourceRunId,
    expectedSourceRunAttempt: sourceRunAttempt
  });
}

export async function resolveReleaseSource({
  repository,
  releaseTag,
  sourceRunId,
  sourceRunAttempt,
  ghApi = ghApiJson,
  git = createGitClient()
}) {
  const normalizedSourceRunId = toPositiveIntegerString(sourceRunId, 'source run id');
  const normalizedSourceRunAttempt = toPositiveIntegerString(sourceRunAttempt, 'source run attempt');
  const latestSourceRun = await ghApi(`/repos/${repository}/actions/runs/${normalizedSourceRunId}`);
  const sourceRun = await ghApi(
    `/repos/${repository}/actions/runs/${normalizedSourceRunId}/attempts/${normalizedSourceRunAttempt}`
  );
  const sourceContext = verifySourceRun(sourceRun, {
    releaseTag,
    expectedSourceRunId: normalizedSourceRunId,
    expectedSourceRunAttempt: normalizedSourceRunAttempt
  });
  assertLatestSourceRunAttempt(latestSourceRun, {
    releaseTag,
    expectedSourceRunId: normalizedSourceRunId,
    expectedSourceRunAttempt: sourceContext.sourceRunAttempt
  });
  const tagRef = await ghApi(`/repos/${repository}/git/ref/tags/${encodeURIComponent(releaseTag)}`);
  if (tagRef?.object?.type !== 'tag') {
    return verifyReleaseSource({
      releaseTag,
      sourceRun,
      tagRef,
      tagObject: { object: { type: tagRef?.object?.type }, verification: { verified: false, reason: 'lightweight' } },
      commitContainedInMain: false,
      sourceDateEpoch: 0
    });
  }
  const tagObject = await ghApi(`/repos/${repository}/git/tags/${tagRef.object.sha}`);
  const gitState = git.verifyLocalTagState(releaseTag);
  return verifyReleaseSource({
    releaseTag,
    sourceRun,
    tagRef,
    tagObject,
    localTagObjectSha: gitState.localTagObjectSha,
    localHeadSha: gitState.localHeadSha,
    commitContainedInMain: gitState.commitContainedInMain,
    sourceDateEpoch: gitState.sourceDateEpoch
  });
}

export async function verifyPackageProvenance({
  packageName,
  npmVersion,
  releaseTag,
  headSha,
  sourceRunId,
  sourceRunAttempt,
  packageMetadata,
  attestationDocument
}) {
  if (packageMetadata?.name !== packageName) {
    throw new Error(
      `Refusing GitHub release: expected npm metadata for ${packageName}, got ${JSON.stringify(packageMetadata?.name)}.`
    );
  }
  if (packageMetadata?.version !== npmVersion) {
    throw new Error(
      `Refusing GitHub release: expected ${packageName}@${npmVersion}, got ${JSON.stringify(packageMetadata?.version)}.`
    );
  }
  const integrityHex = integrityDigestHex(packageMetadata?.dist?.integrity);
  const expectedSubjectName = npmPackageSubjectName(packageName, npmVersion);
  const slsaPointer = packageMetadata?.dist?.attestations?.provenance?.predicateType;
  if (slsaPointer !== SLSA_PREDICATE) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} must expose npm SLSA provenance.`
    );
  }
  const attestations = attestationDocument?.attestations;
  if (!Array.isArray(attestations) || attestations.length === 0) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} is missing npm attestation data.`
    );
  }
  const trustedPublisherAttestation = attestations.find(
    (entry) => entry?.predicateType === TRUSTED_PUBLISHER_PREDICATE
  );
  if (!trustedPublisherAttestation) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} is missing npm trusted publisher attestation.`
    );
  }
  validateTrustedPublisherMetadata(packageMetadata, { packageName, npmVersion });
  validateTrustedPublisherAttestation(
    decodeDssePayload(trustedPublisherAttestation?.bundle?.dsseEnvelope),
    { packageName, npmVersion, integrityHex, expectedSubjectName }
  );
  const slsaAttestation = selectSingleAttestationByPredicateType(attestations, SLSA_PREDICATE, {
    packageName,
    npmVersion,
    label: 'npm SLSA provenance attestation'
  });
  const statement = decodeDssePayload(slsaAttestation?.bundle?.dsseEnvelope);
  validateSlsaProvenanceStatement(statement, {
    packageName,
    npmVersion,
    releaseTag,
    headSha,
    sourceRunId,
    sourceRunAttempt,
    integrityHex,
    expectedSubjectName
  });
  return {
    packageName,
    npmVersion,
    integrityHex
  };
}

function validateTrustedPublisherMetadata(packageMetadata, { packageName, npmVersion }) {
  const npmUser = packageMetadata?._npmUser;
  const trustedPublisher = npmUser?.trustedPublisher;
  if (
    !trustedPublisher ||
    typeof trustedPublisher !== 'object' ||
    typeof trustedPublisher.oidcConfigId !== 'string' ||
    !trustedPublisher.oidcConfigId.startsWith('oidc:')
  ) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} npm metadata must include trusted publisher metadata from GitHub Actions.`
    );
  }
  if (
    npmUser?.name !== TRUSTED_PUBLISHER_NAME ||
    npmUser?.email !== TRUSTED_PUBLISHER_EMAIL ||
    trustedPublisher.id !== TRUSTED_PUBLISHER_ID
  ) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} npm metadata must record the GitHub trusted publisher.`
    );
  }
}

function validateTrustedPublisherAttestation(statement, {
  packageName,
  npmVersion,
  integrityHex,
  expectedSubjectName
}) {
  validatePackageSubject(statement, {
    packageName,
    npmVersion,
    integrityHex,
    expectedSubjectName,
    label: 'trusted publisher attestation'
  });
  if (statement?.predicateType !== TRUSTED_PUBLISHER_PREDICATE) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} trusted publisher attestation predicate must be ${TRUSTED_PUBLISHER_PREDICATE}.`
    );
  }
  if (statement?.predicate?.name !== packageName) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} trusted publisher attestation must name ${packageName}.`
    );
  }
  if (statement?.predicate?.version !== npmVersion) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} trusted publisher attestation version must be ${npmVersion}.`
    );
  }
  if (statement?.predicate?.registry !== NPM_REGISTRY_URL) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} trusted publisher attestation registry must be ${NPM_REGISTRY_URL}.`
    );
  }
}

function validateSlsaProvenanceStatement(statement, {
  packageName,
  npmVersion,
  releaseTag,
  headSha,
  sourceRunId,
  sourceRunAttempt,
  integrityHex,
  expectedSubjectName
}) {
  validatePackageSubject(statement, {
    packageName,
    npmVersion,
    integrityHex,
    expectedSubjectName,
    label: 'provenance'
  });
  if (statement?.predicateType !== SLSA_PREDICATE) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} decoded SLSA predicateType must be ${SLSA_PREDICATE}.`
    );
  }
  const workflow = statement?.predicate?.buildDefinition?.externalParameters?.workflow;
  if (workflow?.repository !== REPOSITORY_URL) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance repository must be ${REPOSITORY_URL}.`
    );
  }
  if (workflow?.path !== WORKFLOW_PATH) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance workflow path must be ${WORKFLOW_PATH}.`
    );
  }
  if (workflow?.ref !== `refs/tags/${releaseTag}`) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance ref must be refs/tags/${releaseTag}.`
    );
  }
  if (statement?.predicate?.buildDefinition?.internalParameters?.github?.event_name !== 'push') {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance must come from event push.`
    );
  }
  const expectedDependencyUri = `git+${REPOSITORY_URL}@refs/tags/${releaseTag}`;
  const dependency = statement?.predicate?.buildDefinition?.resolvedDependencies?.find(
    (entry) => entry?.uri === expectedDependencyUri
  );
  if (!dependency) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance must include ${expectedDependencyUri}.`
    );
  }
  if (dependency?.digest?.gitCommit !== headSha) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance gitCommit must be ${headSha}.`
    );
  }
  const expectedInvocationId =
    `https://github.com/OpenCoven/coven/actions/runs/${toPositiveIntegerString(sourceRunId, 'source run id')}/attempts/` +
    `${toPositiveInteger(sourceRunAttempt, 'source run attempt')}`;
  if (statement?.predicate?.runDetails?.metadata?.invocationId !== expectedInvocationId) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} provenance run attempt must be ${expectedInvocationId}.`
    );
  }
}

function validatePackageSubject(statement, {
  packageName,
  npmVersion,
  integrityHex,
  expectedSubjectName,
  label
}) {
  const subjects = Array.isArray(statement?.subject) ? statement.subject : [];
  if (subjects.length !== 1 || subjects[0]?.name !== expectedSubjectName) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} ${label} is missing subject ${expectedSubjectName}.`
    );
  }
  if (subjects[0]?.digest?.sha512 !== integrityHex) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} ${label} subject digest must match npm dist.integrity.`
    );
  }
}

function selectSingleAttestationByPredicateType(attestations, predicateType, { packageName, npmVersion, label }) {
  const matches = attestations.filter((entry) => entry?.predicateType === predicateType);
  if (matches.length !== 1) {
    throw new Error(
      `Refusing GitHub release: ${packageName}@${npmVersion} must expose exactly one ${label} (${predicateType}), got ${matches.length}.`
    );
  }
  return matches[0];
}

function npmPackageSubjectName(packageName, npmVersion) {
  const encodedName = String(packageName)
    .split('/')
    .map((segment) => encodeURIComponent(segment))
    .join('/');
  return `pkg:npm/${encodedName}@${npmVersion}`;
}

export async function verifyAllPackageProvenance({
  releaseTag,
  npmVersion,
  headSha,
  sourceRunId,
  sourceRunAttempt,
  fetchJson = fetchJsonFromNetwork
}) {
  for (const packageName of RELEASE_PACKAGES) {
    const encodedPackageName = encodeURIComponent(packageName);
    const metadata = await fetchJson(`https://registry.npmjs.org/${encodedPackageName}/${npmVersion}`);
    const attestations = await fetchJson(metadata?.dist?.attestations?.url);
    await verifyPackageProvenance({
      packageName,
      npmVersion,
      releaseTag,
      headSha,
      sourceRunId,
      sourceRunAttempt,
      packageMetadata: metadata,
      attestationDocument: attestations
    });
  }
}

export function verifyNpmRegistrySignatures({
  npmVersion,
  auditDir,
  commandRunner = runCommand
}) {
  const normalizedVersion = String(npmVersion ?? '').trim();
  if (!normalizedVersion) {
    throw new Error('Refusing GitHub release: npm version is required to verify npm registry signatures.');
  }
  const normalizedAuditDirInput = String(auditDir ?? '').trim();
  if (!normalizedAuditDirInput) {
    throw new Error('Refusing GitHub release: audit directory is required to verify npm registry signatures.');
  }
  const normalizedAuditDir = path.resolve(normalizedAuditDirInput);
  rmSync(normalizedAuditDir, { recursive: true, force: true });
  mkdirSync(normalizedAuditDir, { recursive: true });
  writeFileSync(
    path.join(normalizedAuditDir, 'package.json'),
    `${JSON.stringify({
      name: 'opencoven-release-npm-signatures-audit',
      private: true,
      version: '0.0.0',
      dependencies: Object.fromEntries(
        RELEASE_PACKAGES.map((packageName) => [packageName, normalizedVersion])
      )
    }, null, 2)}\n`
  );
  commandRunner(
    'npm',
    ['install', '--package-lock-only', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
    { cwd: normalizedAuditDir }
  );
  assertReleasePackagesResolvedInLockfile({
    auditDir: normalizedAuditDir,
    npmVersion: normalizedVersion
  });
  commandRunner(
    'npm',
    ['install', '--ignore-scripts', '--force', '--no-audit', '--no-fund'],
    { cwd: normalizedAuditDir }
  );
  const packageLockEntries = assertReleasePackagesResolvedInLockfile({
    auditDir: normalizedAuditDir,
    npmVersion: normalizedVersion
  });
  commandRunner('npm', ['audit', 'signatures'], {
    cwd: normalizedAuditDir
  });
  return {
    auditDir: normalizedAuditDir,
    packageNames: [...RELEASE_PACKAGES],
    npmVersion: normalizedVersion,
    packageLockEntries
  };
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function packageLockRootDependencies(packageLock) {
  const rootPackageDependencies = packageLock?.packages?.['']?.dependencies;
  if (isPlainObject(rootPackageDependencies)) {
    return {
      dependencies: rootPackageDependencies,
      source: 'packages[""].dependencies'
    };
  }
  if (!isPlainObject(packageLock?.packages) && isPlainObject(packageLock?.dependencies)) {
    return {
      dependencies: Object.fromEntries(
        Object.entries(packageLock.dependencies).map(([packageName, entry]) => [
          packageName,
          typeof entry === 'string' ? entry : entry?.version
        ])
      ),
      source: 'dependencies'
    };
  }
  throw new Error(
    'Refusing GitHub release: package-lock.json must record the exact five @opencoven dependencies in packages[""].dependencies (lockfile v2+) or dependencies (lockfile v1).'
  );
}

function assertExactReleaseRootDependencies({ rootDependencies, source, npmVersion }) {
  const expectedNames = [...RELEASE_PACKAGES].sort();
  const actualNames = Object.keys(rootDependencies).sort();
  const missingNames = expectedNames.filter((packageName) => !actualNames.includes(packageName));
  const extraNames = actualNames.filter((packageName) => !expectedNames.includes(packageName));
  if (missingNames.length > 0 || extraNames.length > 0) {
    const details = [
      missingNames.length > 0 ? `missing root dependencies: ${missingNames.join(', ')}` : null,
      extraNames.length > 0 ? `unexpected root dependencies: ${extraNames.join(', ')}` : null
    ].filter(Boolean);
    throw new Error(
      `Refusing GitHub release: package-lock.json must declare exactly the five release packages in ${source}; ${details.join('; ')}.`
    );
  }
  for (const packageName of expectedNames) {
    const declaredVersion = rootDependencies[packageName];
    if (declaredVersion !== npmVersion) {
      throw new Error(
        `Refusing GitHub release: package-lock.json must declare ${packageName}@${npmVersion} in ${source}, got ${JSON.stringify(declaredVersion)}.`
      );
    }
  }
}

function resolvedPackageLockEntry(packageLock, packageName) {
  const packagePath = `node_modules/${packageName}`;
  const packageEntry = packageLock?.packages?.[packagePath];
  if (isPlainObject(packageEntry)) {
    return {
      entry: packageEntry,
      source: packagePath
    };
  }
  if (!isPlainObject(packageLock?.packages) && isPlainObject(packageLock?.dependencies?.[packageName])) {
    return {
      entry: packageLock.dependencies[packageName],
      source: `dependencies.${packageName}`
    };
  }
  return {
    entry: null,
    source: isPlainObject(packageLock?.packages) ? packagePath : `dependencies.${packageName}`
  };
}

function assertReleasePackagesResolvedInLockfile({ auditDir, npmVersion }) {
  const packageLockPath = path.join(auditDir, 'package-lock.json');
  let packageLock;
  try {
    packageLock = JSON.parse(readFileSync(packageLockPath, 'utf8'));
  } catch (error) {
    throw new Error(
      `Refusing GitHub release: failed to read ${packageLockPath}: ${error?.message ?? String(error)}`
    );
  }
  const { dependencies: rootDependencies, source: rootDependencySource } = packageLockRootDependencies(packageLock);
  assertExactReleaseRootDependencies({
    rootDependencies,
    source: rootDependencySource,
    npmVersion
  });

  const entries = {};
  for (const packageName of RELEASE_PACKAGES) {
    const { entry: packageEntry, source: packageEntrySource } = resolvedPackageLockEntry(packageLock, packageName);
    if (!packageEntry) {
      throw new Error(
        `Refusing GitHub release: package-lock.json is missing resolved entry ${packageEntrySource} for ${packageName}@${npmVersion}.`
      );
    }
    if (packageEntry.version !== npmVersion) {
      throw new Error(
        `Refusing GitHub release: package-lock.json resolved ${packageName}@${packageEntry.version ?? '<missing>'}; expected ${npmVersion}.`
      );
    }
    if (typeof packageEntry.resolved !== 'string' || packageEntry.resolved.trim() === '') {
      throw new Error(
        `Refusing GitHub release: package-lock.json entry ${packageEntrySource} for ${packageName}@${npmVersion} is missing a resolved tarball URL.`
      );
    }
    entries[packageName] = {
      version: packageEntry.version,
      resolved: packageEntry.resolved
    };
  }
  return entries;
}

export function packageGitHubRelease({
  releaseTag,
  artifactsDir,
  outputDir,
  sourceDateEpoch,
  sourceCommit,
  protocolBundlePath
}) {
  const { npmVersion } = parseReleaseTag(releaseTag);
  const normalizedArtifactsDir = path.resolve(String(artifactsDir));
  const normalizedOutputDir = path.resolve(String(outputDir));
  const epoch = toNonNegativeInteger(sourceDateEpoch, 'SOURCE_DATE_EPOCH');
  rmSync(normalizedOutputDir, { recursive: true, force: true });
  mkdirSync(normalizedOutputDir, { recursive: true });

  const checksumRecords = [];
  for (const definition of Object.values(PACKAGE_DEFINITIONS)) {
    const assetName = definition.assetName(npmVersion);
    const archiveBytes =
      definition.format === 'zip'
        ? createZipArchive(readArtifactFiles(normalizedArtifactsDir, definition), epoch)
        : createTarGzArchive(readArtifactFiles(normalizedArtifactsDir, definition), epoch);
    writeFileSync(path.join(normalizedOutputDir, assetName), archiveBytes);
    checksumRecords.push({ assetName, sha256: sha256Hex(archiveBytes) });
  }

  const checksumText = renderChecksumManifest(checksumRecords);
  assertChecksumManifest(
    checksumText,
    checksumRecords.map((record) => record.assetName)
  );
  writeFileSync(path.join(normalizedOutputDir, CHECKSUMS_NAME), checksumText);
  const protocolAssetNames = [];
  if (releaseIncludesAutomationsProtocol(releaseTag)) {
    if (!sourceCommit || !protocolBundlePath) {
      throw new Error(
        'GitHub releases v0.4.4 and later require sourceCommit and protocolBundlePath for the Automations protocol asset.'
      );
    }
    const protocolBundleName = automationsProtocolBundleName(sourceCommit);
    const normalizedProtocolBundlePath = path.resolve(String(protocolBundlePath));
    if (
      path.basename(normalizedProtocolBundlePath) !== protocolBundleName ||
      !existsSync(normalizedProtocolBundlePath) ||
      !statSync(normalizedProtocolBundlePath).isFile()
    ) {
      throw new Error(
        `Refusing GitHub release: protocol bundle must be a regular file named ${protocolBundleName}.`
      );
    }
    copyFileSync(
      normalizedProtocolBundlePath,
      path.join(normalizedOutputDir, protocolBundleName)
    );
    protocolAssetNames.push(protocolBundleName);
  }

  return {
    assetNames: [
      ...checksumRecords.map((record) => record.assetName),
      ...protocolAssetNames,
      CHECKSUMS_NAME
    ].sort(),
    checksums: checksumRecords
  };
}

function readArtifactFiles(artifactsDir, definition) {
  const artifactDir = path.join(artifactsDir, definition.artifactName);
  let dirents;
  try {
    dirents = readdirSync(artifactDir, { withFileTypes: true });
  } catch (error) {
    throw new Error(
      `Refusing GitHub release: source artifact directory ${artifactDir} is missing for ${definition.artifactName}.`
    );
  }
  const actualNames = dirents.map((dirent) => dirent.name).sort();
  const expectedNames = [...definition.expectedFiles].sort();
  const missingNames = expectedNames.filter((name) => !actualNames.includes(name));
  if (missingNames.length > 0) {
    throw new Error(
      `Refusing GitHub release: ${definition.artifactName} is missing required file(s): ${missingNames.join(', ')}.`
    );
  }
  const unexpectedNames = actualNames.filter((name) => !expectedNames.includes(name));
  if (unexpectedNames.length > 0) {
    throw new Error(
      `Refusing GitHub release: ${definition.artifactName} has unexpected files: ${unexpectedNames.join(', ')}.`
    );
  }
  return expectedNames.map((name) => {
    const dirent = dirents.find((candidate) => candidate.name === name);
    if (!dirent?.isFile()) {
      throw new Error(
        `Refusing GitHub release: ${definition.artifactName}/${name} must be a file.`
      );
    }
    const absolutePath = path.join(artifactDir, name);
    if (!statSync(absolutePath).isFile()) {
      throw new Error(
        `Refusing GitHub release: ${definition.artifactName}/${name} must be a regular file.`
      );
    }
    return {
      name,
      data: readFileSync(absolutePath)
    };
  });
}

function createTarGzArchive(files, sourceDateEpoch) {
  const tarParts = [];
  const sortedFiles = [...files].sort((left, right) => left.name.localeCompare(right.name));
  for (const file of sortedFiles) {
    tarParts.push(createTarHeader(file.name, file.data.length, sourceDateEpoch));
    tarParts.push(file.data);
    const remainder = file.data.length % 512;
    if (remainder !== 0) {
      tarParts.push(Buffer.alloc(512 - remainder, 0));
    }
  }
  tarParts.push(Buffer.alloc(1024, 0));
  const gzipBytes = gzipSync(Buffer.concat(tarParts), {
    level: 9
  });
  gzipBytes.writeUInt32LE(sourceDateEpoch, 4);
  return gzipBytes;
}

function createTarHeader(name, size, sourceDateEpoch) {
  if (Buffer.byteLength(name, 'utf8') > 100) {
    throw new Error(`Refusing GitHub release: tar entry ${name} exceeds the ustar name limit.`);
  }
  const header = Buffer.alloc(512, 0);
  writeString(header, 0, 100, name);
  writeOctal(header, 100, 8, 0o755);
  writeOctal(header, 108, 8, 0);
  writeOctal(header, 116, 8, 0);
  writeOctal(header, 124, 12, size);
  writeOctal(header, 136, 12, sourceDateEpoch);
  header.fill(0x20, 148, 156);
  header[156] = '0'.charCodeAt(0);
  writeString(header, 257, 6, 'ustar');
  writeString(header, 263, 2, '00');
  const checksum = header.reduce((total, byte) => total + byte, 0);
  const checksumText = checksum.toString(8).padStart(6, '0');
  writeString(header, 148, 6, checksumText);
  header[154] = 0;
  header[155] = 0x20;
  return header;
}

function createZipArchive(files, sourceDateEpoch) {
  const localRecords = [];
  const centralRecords = [];
  let offset = 0;
  const timestamp = zipTimestamp(sourceDateEpoch);

  for (const file of [...files].sort((left, right) => left.name.localeCompare(right.name))) {
    const nameBuffer = Buffer.from(file.name, 'utf8');
    const data = Buffer.from(file.data);
    const crc32 = crc32ForBuffer(data);
    const localHeader = Buffer.alloc(30 + nameBuffer.length);
    localHeader.writeUInt32LE(0x04034b50, 0);
    localHeader.writeUInt16LE(ZIP_VERSION, 4);
    localHeader.writeUInt16LE(ZIP_UTF8_FLAG, 6);
    localHeader.writeUInt16LE(ZIP_STORE_METHOD, 8);
    localHeader.writeUInt16LE(timestamp.dosTime, 10);
    localHeader.writeUInt16LE(timestamp.dosDate, 12);
    localHeader.writeUInt32LE(crc32, 14);
    localHeader.writeUInt32LE(data.length, 18);
    localHeader.writeUInt32LE(data.length, 22);
    localHeader.writeUInt16LE(nameBuffer.length, 26);
    localHeader.writeUInt16LE(0, 28);
    nameBuffer.copy(localHeader, 30);
    localRecords.push(localHeader, data);

    const centralHeader = Buffer.alloc(46 + nameBuffer.length);
    centralHeader.writeUInt32LE(0x02014b50, 0);
    centralHeader.writeUInt16LE((3 << 8) | ZIP_VERSION, 4);
    centralHeader.writeUInt16LE(ZIP_VERSION, 6);
    centralHeader.writeUInt16LE(ZIP_UTF8_FLAG, 8);
    centralHeader.writeUInt16LE(ZIP_STORE_METHOD, 10);
    centralHeader.writeUInt16LE(timestamp.dosTime, 12);
    centralHeader.writeUInt16LE(timestamp.dosDate, 14);
    centralHeader.writeUInt32LE(crc32, 16);
    centralHeader.writeUInt32LE(data.length, 20);
    centralHeader.writeUInt32LE(data.length, 24);
    centralHeader.writeUInt16LE(nameBuffer.length, 28);
    centralHeader.writeUInt16LE(0, 30);
    centralHeader.writeUInt16LE(0, 32);
    centralHeader.writeUInt16LE(0, 34);
    centralHeader.writeUInt16LE(0, 36);
    centralHeader.writeUInt32LE(0o755 << 16, 38);
    centralHeader.writeUInt32LE(offset, 42);
    nameBuffer.copy(centralHeader, 46);
    centralRecords.push(centralHeader);

    offset += localHeader.length + data.length;
  }

  const centralDirectory = Buffer.concat(centralRecords);
  const endOfCentralDirectory = Buffer.alloc(22);
  endOfCentralDirectory.writeUInt32LE(0x06054b50, 0);
  endOfCentralDirectory.writeUInt16LE(0, 4);
  endOfCentralDirectory.writeUInt16LE(0, 6);
  endOfCentralDirectory.writeUInt16LE(centralRecords.length, 8);
  endOfCentralDirectory.writeUInt16LE(centralRecords.length, 10);
  endOfCentralDirectory.writeUInt32LE(centralDirectory.length, 12);
  endOfCentralDirectory.writeUInt32LE(offset, 16);
  endOfCentralDirectory.writeUInt16LE(0, 20);

  return Buffer.concat([...localRecords, centralDirectory, endOfCentralDirectory]);
}

function zipTimestamp(sourceDateEpoch) {
  const date = new Date(sourceDateEpoch * 1000);
  const year = Math.max(date.getUTCFullYear(), 1980);
  return {
    dosDate: ((year - 1980) << 9) | ((date.getUTCMonth() + 1) << 5) | date.getUTCDate(),
    dosTime:
      (date.getUTCHours() << 11) |
      (date.getUTCMinutes() << 5) |
      Math.floor(date.getUTCSeconds() / 2)
  };
}

export function renderChecksumManifest(records) {
  const normalized = [...records]
    .map((record) => ({
      assetName: record.assetName,
      sha256: String(record.sha256).toLowerCase()
    }))
    .sort((left, right) => left.assetName.localeCompare(right.assetName));
  const expectedNames = normalized.map((record) => record.assetName);
  const text = normalized.map((record) => `${record.sha256}  ${record.assetName}`).join('\n');
  assertChecksumManifest(text, expectedNames);
  return `${text}\n`;
}

export function assertChecksumManifest(text, expectedNames) {
  const expected = [...expectedNames].sort();
  const trimmed = String(text).replace(/\s+$/, '');
  const lines = trimmed.length === 0 ? [] : trimmed.split(/\r?\n/);
  if (lines.length !== expected.length || lines.some((line) => line.trim().length === 0)) {
    const expectedCount = expected.length === 4 ? 'four' : String(expected.length);
    throw new Error(`SHA256SUMS must contain exactly ${expectedCount} non-empty entries.`);
  }
  const names = [];
  for (const line of lines) {
    const match = /^([0-9a-f]{64})  (.+)$/.exec(line);
    if (!match) {
      throw new Error('SHA256SUMS entries must be <sha256><two spaces><filename>.');
    }
    const name = match[2];
    if (name === CHECKSUMS_NAME) {
      throw new Error('SHA256SUMS must not checksum SHA256SUMS itself.');
    }
    if (name.includes('/') || name.includes('\\') || path.basename(name) !== name) {
      throw new Error('SHA256SUMS entries must use bare filenames only.');
    }
    names.push(name);
  }
  const sortedNames = [...names].sort();
  if (names.join('\n') !== sortedNames.join('\n')) {
    throw new Error('SHA256SUMS entries must be lexically sorted.');
  }
  if (new Set(names).size !== names.length) {
    throw new Error('SHA256SUMS must not contain duplicate filenames.');
  }
  if (sortedNames.join('\n') !== expected.join('\n')) {
    throw new Error('SHA256SUMS entries must use the exact canonical asset names.');
  }
}

function assertExistingReleaseMetadata(release, releaseTag) {
  const actualTagName = String(release?.tag_name ?? release?.tagName ?? '').trim();
  if (actualTagName !== releaseTag) {
    throw new Error(
      `Refusing GitHub release: existing release tag must be ${releaseTag}, got ${JSON.stringify(actualTagName || null)}.`
    );
  }
  const expectedTitle = `Coven ${releaseTag}`;
  if (release?.name !== expectedTitle) {
    throw new Error(
      `Refusing GitHub release: existing release title must be ${JSON.stringify(expectedTitle)}, got ${JSON.stringify(release?.name)}.`
    );
  }
  if (release?.draft !== false) {
    throw new Error(
      'Refusing GitHub release: existing release must not be a draft; draft=false is required for recovery.'
    );
  }
  if (release?.prerelease !== false) {
    throw new Error(
      'Refusing GitHub release: existing release must not be a prerelease; prerelease=false is required for recovery.'
    );
  }
}

export async function syncGitHubRelease({
  releaseTag,
  outputDir,
  expectedTagObjectSha,
  expectedHeadSha,
  releaseClient = createGhReleaseClient()
}) {
  const expectedAssetNames = canonicalReleaseAssetNames(releaseTag, expectedHeadSha).sort();
  const normalizedOutputDir = path.resolve(String(outputDir));
  const presentAssetNames = readdirSync(normalizedOutputDir).sort();
  if (presentAssetNames.join('\n') !== expectedAssetNames.join('\n')) {
    throw new Error(
      `Refusing GitHub release: ${normalizedOutputDir} must contain the exact canonical assets ${expectedAssetNames.join(', ')}.`
    );
  }
  const expectedHashesByName = new Map(
    expectedAssetNames.map((assetName) => [assetName, sha256Hex(readFileSync(path.join(normalizedOutputDir, assetName)))])
  );

  let release = await releaseClient.getReleaseByTag(releaseTag);
  let createdRelease = false;
  if (!release) {
    await releaseClient.revalidateTag({
      releaseTag,
      expectedTagObjectSha,
      expectedHeadSha
    });
    release = await releaseClient.createRelease({
      releaseTag,
      title: `Coven ${releaseTag}`,
      notesFromTag: true,
      verifyTag: true
    });
    createdRelease = true;
  } else {
    assertExistingReleaseMetadata(release, releaseTag);
  }

  const existingAssets = Array.isArray(release?.assets) ? release.assets : [];
  const existingAssetNames = existingAssets.map((asset) => asset.name);
  const duplicateAssets = [...new Set(existingAssetNames.filter(
    (assetName, index) => existingAssetNames.indexOf(assetName) !== index
  ))].sort();
  if (duplicateAssets.length > 0) {
    throw new Error(
      `Refusing GitHub release: duplicate release assets ${duplicateAssets.join(', ')}. ` +
        'Delete only the duplicate GitHub asset through an audited operator action, then rerun this GitHub-only workflow. Never move/reuse the tag and never republish npm.'
    );
  }
  const existingAssetsByName = new Map(existingAssets.map((asset) => [asset.name, asset]));
  const extraAssets = [...existingAssetsByName.keys()]
    .filter((assetName) => !expectedHashesByName.has(assetName))
    .sort();
  if (extraAssets.length > 0) {
    throw new Error(
      `Refusing GitHub release: unexpected release assets ${extraAssets.join(', ')}. ` +
        'Delete only the unexpected or mismatched GitHub asset through an audited operator action, then rerun this GitHub-only workflow. Never move/reuse the tag and never republish npm.'
    );
  }

  const uploaded = [];
  const skipped = [];
  const missingAssetNames = [];
  const downloadDir = path.join(normalizedOutputDir, '.release-sync-downloads');
  rmSync(downloadDir, { recursive: true, force: true });
  mkdirSync(downloadDir, { recursive: true });
  try {
    for (const assetName of expectedAssetNames) {
      const existingAsset = existingAssetsByName.get(assetName);
      if (!existingAsset) {
        missingAssetNames.push(assetName);
        continue;
      }
      const defaultDownloadPath = path.join(downloadDir, assetName);
      const downloadResult = await releaseClient.downloadAsset(existingAsset, defaultDownloadPath);
      let observedAssetPath = defaultDownloadPath;
      if (Buffer.isBuffer(downloadResult)) {
        writeFileSync(defaultDownloadPath, downloadResult);
      } else if (typeof downloadResult === 'string' && downloadResult.trim() !== '') {
        observedAssetPath = path.resolve(downloadResult);
      }
      const observedHash = sha256Hex(readFileSync(observedAssetPath));
      const expectedHash = expectedHashesByName.get(assetName);
      if (observedHash !== expectedHash) {
        throw new Error(
          `Refusing GitHub release: asset ${assetName} mismatch: observed hash ${observedHash}, expected hash ${expectedHash}. ` +
            'Record the observed hash, expected hash, and reason, delete only the mismatched GitHub asset through an audited operator action, then rerun this GitHub-only workflow. Never move/reuse the tag and never republish npm.'
        );
      }
      skipped.push(assetName);
    }
    if (!createdRelease || missingAssetNames.length > 0) {
      await releaseClient.revalidateTag({
        releaseTag,
        expectedTagObjectSha,
        expectedHeadSha
      });
    }
    for (const assetName of missingAssetNames) {
      await releaseClient.uploadAsset({
        releaseTag,
        assetName,
        filePath: path.join(normalizedOutputDir, assetName)
      });
      uploaded.push(assetName);
    }
  } finally {
    rmSync(downloadDir, { recursive: true, force: true });
  }

  return {
    createdRelease,
    uploaded,
    skipped
  };
}

function createGitClient() {
  return {
    verifyLocalTagState(releaseTag) {
      runCommand('git', [
        'fetch',
        '--force',
        '--no-tags',
        'origin',
        'main',
        `refs/tags/${releaseTag}:refs/tags/${releaseTag}`
      ]);
      const tagObjectSha = runCommand('git', ['rev-parse', releaseTag]).trim();
      const tagObjectType = runCommand('git', ['cat-file', '-t', tagObjectSha]).trim();
      if (tagObjectType !== 'tag') {
        throw new Error(
          `Refusing GitHub release: local tag ${releaseTag} must resolve to an annotated tag object, got ${tagObjectType}.`
        );
      }
      const localHeadSha = runCommand('git', ['rev-parse', `${releaseTag}^{commit}`]).trim();
      const ancestor = spawnCapture('git', ['merge-base', '--is-ancestor', localHeadSha, 'origin/main']);
      if (ancestor.error) {
        throw new Error(ancestor.error.message);
      }
      if (ancestor.status !== 0 && ancestor.status !== 1) {
        throw new Error(
          `git merge-base --is-ancestor ${localHeadSha} origin/main exited with ${ancestor.status}.`
        );
      }
      const sourceDateEpoch = runCommand('git', ['log', '-1', '--format=%ct', localHeadSha]).trim();
      return {
        localTagObjectSha: tagObjectSha,
        localHeadSha,
        commitContainedInMain: ancestor.status === 0,
        sourceDateEpoch: toNonNegativeInteger(sourceDateEpoch, 'SOURCE_DATE_EPOCH')
      };
    }
  };
}

function createGhReleaseClient({ repository = process.env.GITHUB_REPOSITORY } = {}) {
  if (!repository) {
    throw new Error('GITHUB_REPOSITORY is required to synchronize GitHub release assets.');
  }
  return {
    async getReleaseByTag(releaseTag) {
      const endpoint = `/repos/${repository}/releases/tags/${encodeURIComponent(releaseTag)}`;
      const result = spawnCapture('gh', ['api', endpoint], { encoding: 'utf8' });
      if (result.error) {
        throw new Error(result.error.message);
      }
      if (result.status !== 0) {
        const stderr = String(result.stderr ?? '').trim();
        if (/404/.test(stderr)) {
          return null;
        }
        throw new Error(stderr || `gh api ${endpoint} exited with ${result.status}.`);
      }
      return JSON.parse(result.stdout);
    },
    /// Read the annotated tag's message through the API rather than asking
    /// `gh release create --notes-from-tag` to do it. Recent `gh` refuses
    /// `--notes-from-tag` together with `--repo` ("using `--notes-from-tag`
    /// with `--repo` is not supported"), which silently became a release
    /// blocker: v0.4.1 published to npm and then failed to produce any GitHub
    /// Release. Dropping `--repo` is not an option -- this script does not
    /// assume its cwd is the target repository -- so the notes are resolved
    /// here and passed as a file instead.
    async readTagAnnotation(releaseTag) {
      const refEndpoint = `/repos/${repository}/git/ref/tags/${encodeURIComponent(releaseTag)}`;
      const ref = JSON.parse(runCommand('gh', ['api', refEndpoint]));
      if (ref?.object?.type !== 'tag') {
        throw new Error(
          `Refusing GitHub release: ${releaseTag} is not an annotated tag (object type ${ref?.object?.type ?? 'unknown'}).`
        );
      }
      const tagEndpoint = `/repos/${repository}/git/tags/${encodeURIComponent(ref.object.sha)}`;
      const message = JSON.parse(runCommand('gh', ['api', tagEndpoint]))?.message;
      if (typeof message !== 'string' || message.trim().length === 0) {
        throw new Error(`Refusing GitHub release: ${releaseTag} has an empty tag annotation.`);
      }
      return message;
    },
    async createRelease({ releaseTag, title, notesFromTag, verifyTag }) {
      const args = ['release', 'create', releaseTag, '--repo', repository, '--title', title];
      // The cleanup guard opens before the directory exists and closes after
      // gh returns, so a throw from writeFileSync leaves nothing behind either.
      let notesDir;
      try {
        if (notesFromTag) {
          const notes = await this.readTagAnnotation(releaseTag);
          notesDir = mkdtempSync(path.join(tmpdir(), 'coven-release-notes-'));
          const notesPath = path.join(notesDir, 'notes.md');
          writeFileSync(notesPath, notes.endsWith('\n') ? notes : `${notes}\n`);
          args.push('--notes-file', notesPath);
        }
        if (verifyTag) {
          args.push('--verify-tag');
        }
        runCommand('gh', args);
      } finally {
        if (notesDir) {
          rmSync(notesDir, { recursive: true, force: true });
        }
      }
      const release = await this.getReleaseByTag(releaseTag);
      if (!release) {
        throw new Error(`GitHub release ${releaseTag} was created but could not be reloaded.`);
      }
      return release;
    },
    async revalidateTag({ releaseTag, expectedTagObjectSha, expectedHeadSha }) {
      const refEndpoint = `/repos/${repository}/git/ref/tags/${encodeURIComponent(releaseTag)}`;
      const refResult = spawnCapture('gh', ['api', refEndpoint], { encoding: 'utf8' });
      if (refResult.error) {
        throw new Error(refResult.error.message);
      }
      if (refResult.status !== 0) {
        const stderr = String(refResult.stderr ?? '').trim();
        if (/404/.test(stderr)) {
          throw new Error(
            `Refusing GitHub release: ${releaseTag} no longer resolves to refs/tags/${releaseTag}.`
          );
        }
        throw new Error(stderr || `gh api ${refEndpoint} exited with ${refResult.status}.`);
      }
      const tagRef = JSON.parse(refResult.stdout);
      if (tagRef?.object?.type !== 'tag' || !isSha(tagRef?.object?.sha)) {
        assertRemoteTagMatchesVerifiedContext({
          releaseTag,
          expectedTagObjectSha,
          expectedHeadSha,
          tagRef,
          tagObject: {
            tag: releaseTag,
            object: {
              type: tagRef?.object?.type,
              sha: tagRef?.object?.sha
            },
            verification: {
              verified: false,
              reason: 'remote-ref-mismatch'
            }
          }
        });
      }
      const tagObjectEndpoint = `/repos/${repository}/git/tags/${tagRef?.object?.sha ?? ''}`;
      const tagObjectResult = spawnCapture('gh', ['api', tagObjectEndpoint], { encoding: 'utf8' });
      if (tagObjectResult.error) {
        throw new Error(tagObjectResult.error.message);
      }
      if (tagObjectResult.status !== 0) {
        const stderr = String(tagObjectResult.stderr ?? '').trim();
        if (/404/.test(stderr)) {
          throw new Error(
            `Refusing GitHub release: annotated tag object ${tagRef?.object?.sha ?? '(missing)'} for ${releaseTag} no longer exists.`
          );
        }
        throw new Error(stderr || `gh api ${tagObjectEndpoint} exited with ${tagObjectResult.status}.`);
      }
      const tagObject = JSON.parse(tagObjectResult.stdout);
      assertRemoteTagMatchesVerifiedContext({
        releaseTag,
        expectedTagObjectSha,
        expectedHeadSha,
        tagRef,
        tagObject
      });
    },
    async downloadAsset(asset, filePath) {
      const assetUrl = asset?.url;
      if (!assetUrl) {
        throw new Error(`GitHub release asset ${asset?.name ?? '(unknown)'} has no API URL.`);
      }
      const endpoint = new URL(assetUrl).pathname;
      return captureCommandOutputToFile(
        'gh',
        ['api', '-H', 'Accept: application/octet-stream', endpoint],
        { filePath }
      );
    },
    async uploadAsset({ releaseTag, filePath }) {
      runCommand('gh', ['release', 'upload', releaseTag, filePath, '--repo', repository]);
    }
  };
}

async function ghApiJson(endpoint) {
  const output = runCommand('gh', ['api', endpoint]);
  return JSON.parse(output);
}

async function fetchJsonFromNetwork(url) {
  if (!url) {
    throw new Error('Expected npm provenance URL, got empty value.');
  }
  const response = await fetch(url, {
    headers: {
      accept: 'application/json'
    }
  });
  if (!response.ok) {
    throw new Error(`GET ${url} failed with HTTP ${response.status}.`);
  }
  return response.json();
}

function decodeDssePayload(dsseEnvelope) {
  const payload = dsseEnvelope?.payload;
  if (!payload) {
    throw new Error('Refusing GitHub release: npm attestation payload is missing.');
  }
  try {
    return JSON.parse(Buffer.from(payload, 'base64').toString('utf8'));
  } catch (error) {
    throw new Error(`Refusing GitHub release: npm attestation payload is not valid JSON (${error.message}).`);
  }
}

function integrityDigestHex(integrity) {
  const match = /^sha512-([A-Za-z0-9+/]+={0,2})$/.exec(String(integrity ?? ''));
  if (!match || match[1].length % 4 !== 0) {
    throw new Error(
      `Refusing GitHub release: npm dist.integrity must be a canonical sha512 SRI string with a 64-byte digest, got ${JSON.stringify(integrity)}.`
    );
  }
  const decoded = Buffer.from(match[1], 'base64');
  if (decoded.length !== 64 || decoded.toString('base64') !== match[1]) {
    throw new Error(
      `Refusing GitHub release: npm dist.integrity must be a canonical sha512 SRI string with a 64-byte digest, got ${JSON.stringify(integrity)}.`
    );
  }
  return decoded.toString('hex');
}

function crc32ForBuffer(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc = CRC32_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function sha256Hex(buffer) {
  return createHash('sha256').update(buffer).digest('hex');
}

function writeString(buffer, offset, length, value) {
  Buffer.from(String(value), 'utf8').copy(buffer, offset, 0, length);
}

function writeOctal(buffer, offset, length, value) {
  const text = Number(value).toString(8).padStart(length - 1, '0');
  writeString(buffer, offset, length - 1, text);
  buffer[offset + length - 1] = 0;
}

function toPositiveIntegerString(value, label) {
  const normalized = String(value ?? '').trim();
  if (!/^[1-9]\d*$/.test(normalized)) {
    throw new Error(`Refusing GitHub release: ${label} must be a positive integer, got ${JSON.stringify(value)}.`);
  }
  return normalized;
}

function toPositiveInteger(value, label) {
  return Number.parseInt(toPositiveIntegerString(value, label), 10);
}

function toNonNegativeInteger(value, label) {
  const normalized = Number(value);
  if (!Number.isSafeInteger(normalized) || normalized < 0) {
    throw new Error(`Refusing GitHub release: ${label} must be a non-negative integer, got ${JSON.stringify(value)}.`);
  }
  return normalized;
}

function isSha(value) {
  return typeof value === 'string' && /^[0-9a-f]{40}$/i.test(value);
}

export function captureCommandOutputToFile(
  command,
  args,
  { filePath, cwd = repoRoot, env = process.env } = {}
) {
  const normalizedFilePathInput = String(filePath ?? '').trim();
  if (!normalizedFilePathInput) {
    throw new Error('Refusing GitHub release: capture output file path is required.');
  }
  const normalizedFilePath = path.resolve(normalizedFilePathInput);
  mkdirSync(path.dirname(normalizedFilePath), { recursive: true });

  let outputHandle;
  let captureError;
  try {
    outputHandle = openSync(normalizedFilePath, 'w');
    const result = spawnSync(command, args, {
      cwd,
      env,
      encoding: 'utf8',
      stdio: ['ignore', outputHandle, 'pipe']
    });
    if (result.error) {
      throw new Error(result.error.message);
    }
    if (result.status !== 0) {
      const printable = [command, ...args].join(' ');
      const stderr = bufferToTrimmedString(result.stderr);
      throw new Error(stderr || `${printable} exited with ${result.status}.`);
    }
    return normalizedFilePath;
  } catch (error) {
    captureError = error;
    throw error;
  } finally {
    if (outputHandle !== undefined) {
      closeSync(outputHandle);
    }
    if (captureError) {
      rmSync(normalizedFilePath, { force: true });
    }
  }
}

function runCommand(command, args, { encoding = 'utf8', cwd = repoRoot, env = process.env } = {}) {
  const result = spawnCapture(command, args, { encoding, cwd, env });
  if (result.error) {
    throw new Error(result.error.message);
  }
  if (result.status !== 0) {
    const printable = [command, ...args].join(' ');
    const stderr = bufferToTrimmedString(result.stderr);
    throw new Error(stderr || `${printable} exited with ${result.status}.`);
  }
  return typeof result.stdout === 'string' ? result.stdout : Buffer.from(result.stdout).toString('utf8');
}

function spawnCapture(command, args, { encoding = 'utf8', cwd = repoRoot, env = process.env } = {}) {
  return spawnSync(command, args, {
    cwd,
    env,
    encoding,
    stdio: ['ignore', 'pipe', 'pipe']
  });
}

function bufferToTrimmedString(value) {
  if (value == null) {
    return '';
  }
  return Buffer.isBuffer(value) ? value.toString('utf8').trim() : String(value).trim();
}
