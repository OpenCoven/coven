#!/usr/bin/env node

// The release's final machine-readable record (#805).
//
//   build      Before the GitHub Release is published, binds every piece of
//              evidence the release produced (source, tag object, signer,
//              exact-source checks before builds, before registry writes and
//              before GitHub publication, protection snapshots, metadata
//              cleanliness, npm provenance, asset digests, the SBOM, toolchains,
//              channel state and the recovery decision) into one JSON asset.
//              It is uploaded to the draft with the other assets, so
//              publishing the immutable release locks it.
//   reconcile  After publication, reads every public channel back and checks
//              it against the receipt: the release is published and immutable,
//              its assets are byte-identical to the receipt's digests, the tag
//              still names the receipt's tag object, npm serves the version,
//              and the support line still covers it. The result is attested
//              with the receipt as its subject.
//
// Both refuse rather than record a gap: a receipt that says "unknown" is not
// a receipt.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { releaseReceiptAssetName, releaseSbomAssetName } from './package-github-release.mjs';
import { parseReleaseTag } from './release-npm-context.mjs';

const RELEASE_PACKAGES = [
  '@opencoven/cli',
  '@opencoven/cli-linux-x64',
  '@opencoven/cli-macos',
  '@opencoven/cli-macos-x64',
  '@opencoven/cli-windows'
];

/// Locale-independent ordering, so the same inputs give the same bytes on
/// every runner.
function byCodePoint(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

function parseOptions(args) {
  const options = new Map();
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg?.startsWith('--')) {
      throw new Error(`Unexpected argument ${JSON.stringify(arg)}.`);
    }
    const value = args[index + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`Missing value for ${arg}.`);
    }
    options.set(arg.slice(2), value);
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

function sha256(buffer) {
  return createHash('sha256').update(buffer).digest('hex');
}

function compareVersions(left, right) {
  const a = left.split('.').map(Number);
  const b = right.split('.').map(Number);
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) {
      return a[index] - b[index];
    }
  }
  return 0;
}

function refuse(message) {
  throw new Error(`Refusing release receipt: ${message}`);
}

function expectKind(document, kind, label) {
  if (document?.schemaVersion !== 1 || document?.kind !== kind) {
    refuse(`${label} must be ${kind} schemaVersion 1, got ${JSON.stringify(document?.kind ?? null)}.`);
  }
}

function controlSummary(controls) {
  return (controls ?? []).map((entry) => ({ id: entry.id, status: entry.status }));
}

/// Classifies npm's current `latest` for one package against this release.
/// Newer is a later release superseding this one, not an incoherence; older
/// means npm does not serve what was just published.
export function npmChannelState(latest, npmVersion) {
  if (!/^\d+\.\d+\.\d+$/.test(String(latest ?? ''))) {
    return 'unreadable';
  }
  const order = compareVersions(latest, npmVersion);
  return order === 0 ? 'current' : order > 0 ? 'superseded' : 'behind';
}

export function buildReleaseReceipt({
  repository,
  releaseTag,
  trigger,
  sourceRun,
  tagVerification,
  sourceAcceptance,
  publicationAuthorization,
  publicationAuthorizationSha256,
  governanceEvaluation,
  npmProvenance,
  assets,
  sbom,
  npmLatest,
  supportedReleaseLine,
  environment,
  observedAt
}) {
  const { npmVersion } = parseReleaseTag(releaseTag);
  expectKind(tagVerification, 'coven.release-tag-verification', 'tag verification');
  expectKind(sourceAcceptance, 'coven.release-source-acceptance', 'source acceptance');
  expectKind(publicationAuthorization, 'coven.release-publication-authorization', 'publication authorization');
  expectKind(governanceEvaluation, 'coven.release-governance-evaluation', 'governance evaluation');
  expectKind(npmProvenance, 'coven.npm-provenance-verification', 'npm provenance');

  const { headSha, tagObjectSha } = tagVerification;
  if (tagVerification.releaseTag !== releaseTag || tagVerification.repository !== repository) {
    refuse(`the tag verification is for ${tagVerification.repository} ${tagVerification.releaseTag}.`);
  }
  for (const [label, document] of [
    ['publication authorization', publicationAuthorization],
    ['source acceptance', { headSha: sourceAcceptance.candidateSha, tagObjectSha: sourceAcceptance.tagObjectSha, releaseTag: sourceAcceptance.releaseTag }]
  ]) {
    if (document.releaseTag !== releaseTag || document.headSha !== headSha || document.tagObjectSha !== tagObjectSha) {
      refuse(`the ${label} names ${document.releaseTag} ${document.tagObjectSha} at ${document.headSha}, the verified tag is ${tagObjectSha} at ${headSha}.`);
    }
  }
  if (publicationAuthorization.authorized !== true) {
    refuse('the npm publication was not authorized.');
  }
  if (governanceEvaluation.compliant !== true) {
    const failed = governanceEvaluation.controls.filter((entry) => entry.status !== 'pass').map((entry) => entry.id);
    refuse(`live protection no longer matches the governance policy (${failed.join(', ')}).`);
  }
  if (sourceRun?.headSha !== headSha) {
    refuse(`the npm source run built ${sourceRun?.headSha}, the verified tag names ${headSha}.`);
  }
  const packages = npmProvenance.packages ?? [];
  if (packages.map((entry) => entry.packageName).sort().join('\n') !== [...RELEASE_PACKAGES].sort().join('\n') ||
    packages.some((entry) => entry.npmVersion !== npmVersion)) {
    refuse(`npm provenance must cover exactly ${RELEASE_PACKAGES.join(', ')} at ${npmVersion}.`);
  }
  const receiptName = releaseReceiptAssetName(npmVersion);
  const sbomName = releaseSbomAssetName(npmVersion);
  if (assets.some((entry) => entry.name === receiptName)) {
    refuse(`${receiptName} cannot list itself.`);
  }
  const sbomAsset = assets.find((entry) => entry.name === sbomName);
  if (!sbomAsset || sbom?.bomFormat !== 'CycloneDX') {
    refuse(`${sbomName} must be present and be a CycloneDX document.`);
  }
  const sbomCommit = sbom.metadata?.properties?.find((entry) => entry.name === 'coven:source-commit')?.value;
  if (sbomCommit !== headSha) {
    refuse(`the SBOM describes ${sbomCommit}, not ${headSha}.`);
  }
  const channelStates = Object.fromEntries(
    RELEASE_PACKAGES.map((name) => [name, { latest: npmLatest[name] ?? null, state: npmChannelState(npmLatest[name], npmVersion) }])
  );
  const incoherent = Object.entries(channelStates).filter(([, entry]) => !['current', 'superseded'].includes(entry.state));
  if (incoherent.length > 0) {
    refuse(`npm does not serve ${npmVersion} as latest: ${incoherent.map(([name, entry]) => `${name} latest ${entry.latest} (${entry.state})`).join(', ')}.`);
  }
  if (`${npmVersion.split('.')[0]}.${npmVersion.split('.')[1]}` !== supportedReleaseLine) {
    refuse(`${npmVersion} is not on the supported ${supportedReleaseLine}.x line.`);
  }

  const evidence = publicationAuthorization.evidence ?? {};
  const authorizationControls = Object.fromEntries((publicationAuthorization.controls ?? []).map((entry) => [entry.id, entry]));
  return {
    schemaVersion: 1,
    kind: 'coven.release-receipt',
    repository,
    releaseTag,
    npmVersion,
    source: {
      commit: headSha,
      tagObject: tagObjectSha,
      defaultBranch: tagVerification.defaultBranch,
      defaultBranchCommit: tagVerification.defaultBranchSha,
      signer: tagVerification.signer,
      githubVerification: tagVerification.github,
      signerPins: tagVerification.signerPins
    },
    checks: {
      beforeRegistryWrites: evidence.sourceAcceptance
        ? { workflow: evidence.sourceAcceptance.workflow, requiredChecks: evidence.sourceAcceptance.requiredChecks, observedAt: evidence.sourceAcceptance.observedAt }
        : null,
      beforeGitHubPublication: {
        workflow: sourceAcceptance.workflow,
        requiredChecks: sourceAcceptance.requiredChecks,
        observedAt: sourceAcceptance.observedAt
      }
    },
    protection: {
      beforeRegistryWrites: evidence.governance
        ? Object.fromEntries(Object.entries(evidence.governance).map(([label, evaluation]) => [label, {
            policySha256: evaluation.policySha256,
            snapshotSha256: evaluation.snapshotSha256,
            compliant: evaluation.compliant,
            controls: controlSummary(evaluation.controls)
          }]))
        : null,
      beforeGitHubPublication: {
        policySha256: governanceEvaluation.policySha256,
        snapshotSha256: governanceEvaluation.snapshotSha256,
        snapshotCapturedAt: governanceEvaluation.snapshotCapturedAt,
        reader: governanceEvaluation.reader,
        compliant: true,
        controls: controlSummary(governanceEvaluation.controls)
      }
    },
    cleanliness: ['metadata.placeholders', 'artifacts.source-commit', 'artifacts.reported-version', 'metadata.release-notes']
      .map((id) => ({ id, status: authorizationControls[id]?.status ?? 'missing', detail: authorizationControls[id]?.detail ?? null })),
    publicationAuthorization: {
      sha256: publicationAuthorizationSha256,
      authorized: true,
      observedAt: publicationAuthorization.observedAt,
      controls: controlSummary(publicationAuthorization.controls)
    },
    npm: {
      workflowRun: { id: sourceRun.sourceRunId, attempt: sourceRun.sourceRunAttempt },
      packages: packages.map((entry) => ({
        name: entry.packageName,
        version: entry.npmVersion,
        integrity: entry.integrity,
        shasum: entry.shasum,
        tarball: entry.tarball,
        provenance: {
          predicateType: entry.provenancePredicateType,
          trustedPublisherPredicateType: entry.trustedPublisherPredicateType,
          attestations: entry.attestationsUrl,
          invocationId: entry.invocationId
        }
      })).sort((left, right) => byCodePoint(left.name, right.name))
    },
    github: {
      assets: [...assets].sort((left, right) => byCodePoint(left.name, right.name)),
      sbom: {
        name: sbomName,
        sha256: sbomAsset.sha256,
        serialNumber: sbom.serialNumber,
        components: sbom.components?.length ?? 0
      }
    },
    toolchains: {
      registryWrites: evidence.environment ?? null,
      githubPublication: environment
    },
    channels: {
      npm: channelStates,
      supportedReleaseLine,
      releaseNotes: evidence.releaseNotes ?? null,
      githubRelease: 'draft verified; published by the reconcile step'
    },
    publication: {
      npm: 'published, provenance verified for every package',
      githubRelease: 'pending publication of the verified draft'
    },
    recovery: trigger === 'workflow_dispatch'
      ? { decision: 'github-release-recovery', detail: `GitHub Release rebuilt from npm workflow run ${sourceRun.sourceRunId} attempt ${sourceRun.sourceRunAttempt}; npm was not republished.` }
      : { decision: 'none', detail: 'first publication' },
    observedAt
  };
}

export function reconcileRelease({ receipt, receiptSha256, release, tagRef, npmLatest, supportedReleaseLine, latestReleaseTag, observedAt }) {
  expectKind(receipt, 'coven.release-receipt', 'receipt');
  const { releaseTag, npmVersion } = receipt;
  const controls = [];
  const record = (id, gaps, detail = 'matches the receipt') => {
    controls.push({ id, status: gaps.length === 0 ? 'pass' : 'fail', detail: gaps.length === 0 ? detail : gaps.join('; ') });
  };

  record('github.published', [
    ...(release?.tag_name !== releaseTag ? [`release names ${release?.tag_name}`] : []),
    ...(release?.draft !== false ? ['release is still a draft'] : []),
    ...(release?.immutable !== true ? ['release is not immutable'] : [])
  ], `published ${release?.published_at ?? ''} and immutable`.trim());

  const served = new Map((release?.assets ?? []).map((asset) => [asset.name, asset]));
  const expected = [...receipt.github.assets, { name: releaseReceiptAssetName(npmVersion), sha256: receiptSha256 }];
  const assetGaps = [];
  for (const entry of expected) {
    const asset = served.get(entry.name);
    if (!asset) {
      assetGaps.push(`${entry.name} is not served`);
    } else if (asset.digest !== `sha256:${entry.sha256}`) {
      assetGaps.push(`${entry.name} is served as ${asset.digest}, the receipt says sha256:${entry.sha256}`);
    }
  }
  for (const name of served.keys()) {
    if (!expected.some((entry) => entry.name === name)) {
      assetGaps.push(`${name} is served but not in the receipt`);
    }
  }
  record('github.assets', assetGaps, `${expected.length} assets, receipt included, byte-identical`);

  record('tag.unchanged', tagRef?.object?.type === 'tag' && tagRef?.object?.sha === receipt.source.tagObject
    ? []
    : [`${releaseTag} now names ${tagRef?.object?.type} ${tagRef?.object?.sha}, the receipt says tag ${receipt.source.tagObject}`]);

  const npmGaps = RELEASE_PACKAGES
    .map((name) => [name, npmChannelState(npmLatest[name], npmVersion)])
    .filter(([, state]) => !['current', 'superseded'].includes(state))
    .map(([name, state]) => `${name} latest ${npmLatest[name]} (${state})`);
  record('npm.latest', npmGaps, `every package serves ${npmVersion} or a later release`);

  const body = String(release?.body ?? '');
  const notesGaps = [];
  if (/-----BEGIN (?:SSH|PGP) SIGNATURE-----/.test(body)) {
    notesGaps.push('the release notes contain a raw signature block');
  }
  const [subject, ...rest] = body.replace(/\r\n/g, '\n').split('\n');
  if (subject !== `Coven ${releaseTag}`) {
    notesGaps.push(`the release notes are titled ${JSON.stringify(subject)}, expected "Coven ${releaseTag}"`);
  }
  const expectedNotes = receipt.channels?.releaseNotes?.bodySha256;
  const servedNotes = createHash('sha256').update(rest.join('\n').trim()).digest('hex');
  if (!expectedNotes) {
    notesGaps.push('the receipt carries no release-notes digest');
  } else if (servedNotes !== expectedNotes) {
    notesGaps.push(`the published notes hash to ${servedNotes}, the authorized annotation to ${expectedNotes}`);
  }
  record('github.release-notes', notesGaps, 'the published notes are the signed annotation, without its signature');

  const line = `${npmVersion.split('.')[0]}.${npmVersion.split('.')[1]}`;
  record('support.line', line === supportedReleaseLine ? [] : [`${npmVersion} is outside the supported ${supportedReleaseLine}.x line`]);

  const githubLatest = latestReleaseTag === releaseTag
    ? 'current'
    : latestReleaseTag && compareVersions(parseReleaseTag(latestReleaseTag).npmVersion, npmVersion) > 0 ? 'superseded' : 'behind';
  record('github.latest', githubLatest === 'behind' ? [`GitHub marks ${latestReleaseTag} as latest, older than ${releaseTag}`] : [], `latest release is ${latestReleaseTag}`);

  return {
    schemaVersion: 1,
    kind: 'coven.release-reconciliation',
    repository: receipt.repository,
    releaseTag,
    receipt: { name: releaseReceiptAssetName(npmVersion), sha256: receiptSha256 },
    release: {
      id: release?.id ?? null,
      htmlUrl: release?.html_url ?? null,
      immutable: release?.immutable === true,
      publishedAt: release?.published_at ?? null
    },
    channels: {
      npm: Object.fromEntries(RELEASE_PACKAGES.map((name) => [name, npmLatest[name] ?? null])),
      githubLatest: latestReleaseTag,
      supportedReleaseLine
    },
    reconciled: controls.every((entry) => entry.status === 'pass'),
    controls,
    observedAt
  };
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8' });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed: ${String(result.stderr).trim()}`);
  }
  return result.stdout;
}

function npmLatestNow() {
  return Object.fromEntries(RELEASE_PACKAGES.map((name) => [name, run('npm', ['view', name, 'dist-tags.latest']).trim()]));
}

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, value) {
  const resolved = path.resolve(filePath);
  mkdirSync(path.dirname(resolved), { recursive: true });
  writeFileSync(resolved, `${JSON.stringify(value, null, 2)}\n`);
}

function environmentNow() {
  return {
    node: process.version,
    npm: run('npm', ['--version']).trim(),
    runnerImage: process.env.ImageOS ? `${process.env.ImageOS} ${process.env.ImageVersion ?? ''}`.trim() : null
  };
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  const options = parseOptions(args);
  const repository = requiredOption(options, 'repository');
  const releaseTag = requiredOption(options, 'release-tag');
  const { npmVersion } = parseReleaseTag(releaseTag);
  const supportedReleaseLine = readJson(requiredOption(options, 'support-policy')).supportedReleaseLine;
  if (command === 'build') {
    const assetsDir = path.resolve(requiredOption(options, 'assets-dir'));
    const assets = readdirSync(assetsDir)
      .filter((name) => statSync(path.join(assetsDir, name)).isFile())
      .map((name) => {
        const bytes = readFileSync(path.join(assetsDir, name));
        return { name, sha256: sha256(bytes), size: bytes.length };
      });
    const authorizationPath = requiredOption(options, 'publication-authorization');
    const receipt = buildReleaseReceipt({
      repository,
      releaseTag,
      trigger: requiredOption(options, 'trigger'),
      sourceRun: {
        sourceRunId: requiredOption(options, 'source-run-id'),
        sourceRunAttempt: Number(requiredOption(options, 'source-run-attempt')),
        headSha: requiredOption(options, 'source-run-head-sha')
      },
      tagVerification: readJson(requiredOption(options, 'tag-verification')),
      sourceAcceptance: readJson(requiredOption(options, 'source-acceptance')),
      publicationAuthorization: readJson(authorizationPath),
      publicationAuthorizationSha256: sha256(readFileSync(authorizationPath)),
      governanceEvaluation: readJson(requiredOption(options, 'governance-evaluation')),
      npmProvenance: readJson(requiredOption(options, 'npm-provenance')),
      assets,
      sbom: readJson(path.join(assetsDir, releaseSbomAssetName(npmVersion))),
      npmLatest: npmLatestNow(),
      supportedReleaseLine,
      environment: environmentNow(),
      observedAt: new Date().toISOString()
    });
    writeJson(path.join(assetsDir, releaseReceiptAssetName(npmVersion)), receipt);
    console.log(`Wrote ${releaseReceiptAssetName(npmVersion)} binding ${assets.length} assets.`);
    return;
  }
  if (command === 'reconcile') {
    const receiptPath = requiredOption(options, 'receipt');
    const ghJson = (endpoint) => JSON.parse(run('gh', ['api', endpoint]));
    const latest = spawnSync('gh', ['api', `/repos/${repository}/releases/latest`, '--jq', '.tag_name'], { encoding: 'utf8' });
    const reconciliation = reconcileRelease({
      receipt: readJson(receiptPath),
      receiptSha256: sha256(readFileSync(receiptPath)),
      release: ghJson(`/repos/${repository}/releases/tags/${encodeURIComponent(releaseTag)}`),
      tagRef: ghJson(`/repos/${repository}/git/ref/tags/${encodeURIComponent(releaseTag)}`),
      npmLatest: npmLatestNow(),
      supportedReleaseLine,
      latestReleaseTag: latest.status === 0 ? latest.stdout.trim() : null,
      observedAt: new Date().toISOString()
    });
    writeJson(requiredOption(options, 'output'), reconciliation);
    for (const entry of reconciliation.controls) {
      const line = `${entry.status === 'pass' ? 'pass' : 'FAIL'} ${entry.id}: ${entry.detail}`;
      console.log(entry.status === 'pass' || process.env.GITHUB_ACTIONS !== 'true' ? line : `::error::Reconciliation ${entry.id}: ${entry.detail}`);
    }
    if (!reconciliation.reconciled) {
      throw new Error(`Release ${releaseTag} does not reconcile with its receipt.`);
    }
    return;
  }
  throw new Error('Usage: release-receipt.mjs <build|reconcile> --repository OWNER/REPO --release-tag vX.Y.Z --support-policy FILE ...');
}

if (isMainModule()) {
  try {
    main();
  } catch (error) {
    console.error(error?.message ?? String(error));
    process.exit(1);
  }
}
