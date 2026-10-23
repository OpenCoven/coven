#!/usr/bin/env node

// The last gate before a release writes to any registry (#805).
//
// The verify-tag job checks the tag and the exact-source CI result before the
// builds start, but a build matrix takes long enough for that evidence to go
// stale: the tag can be moved, the CI run re-attempted, protection relaxed,
// a signer revoked, or another release published first. This step runs in the
// publish job itself, immediately before the first `npm publish`, and decides
// again from fresh reads. Every control is evaluated, so one run reports every
// gap, and the authorization record is written before a refusal exits.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { resolveReleaseSourceAcceptance } from './package-github-release.mjs';
import {
  assertPublishableSurface,
  assertSupportedReleaseLine,
  loadReleaseSupport,
  nativeTargetNamesForPackageSet,
  packageVersionPublished,
  targetPackageName,
  wrapperPackageNameList
} from './publish-npm.mjs';
import { parseReleaseTag } from './release-npm-context.mjs';
import {
  captureGovernanceSnapshot,
  evaluateGovernance,
  githubRequest,
  resolveReadCredential
} from './release-governance.mjs';
import { verifyReleaseTag } from './verify-release-tag.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const GOVERNANCE_POLICY_PATH = '.github/release-governance.json';
const NATIVE_ARTIFACTS = {
  macos: 'target/aarch64-apple-darwin/release/coven',
  'macos-x64': 'target/x86_64-apple-darwin/release/coven',
  'linux-x64': 'target/x86_64-unknown-linux-gnu/release/coven',
  windows: 'target/x86_64-pc-windows-msvc/release/coven.exe'
};
const RUNNABLE_TARGET = 'linux-x64';

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

function run(command, args, { cwd = repoRoot } = {}) {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', shell: process.platform === 'win32' && command === 'npm' });
  if (result.error) {
    throw result.error;
  }
  return result;
}

function npmDistTagLatest(packageName) {
  const result = run('npm', ['view', packageName, 'dist-tags.latest']);
  const latest = String(result.stdout ?? '').trim();
  if (result.status !== 0 || !/^\d+\.\d+\.\d+$/.test(latest)) {
    throw new Error(`could not read the npm latest dist-tag of ${packageName} (exit ${result.status}).`);
  }
  return latest;
}

function readTagAnnotation(releaseTag, cwd) {
  const read = (format) => {
    const result = run('git', ['for-each-ref', `refs/tags/${releaseTag}`, `--format=${format}`], { cwd });
    if (result.status !== 0) {
      throw new Error(`git could not read the ${releaseTag} annotation: ${result.stderr.trim()}`);
    }
    return result.stdout.replace(/\n$/, '');
  };
  return { subject: read('%(contents:subject)'), body: read('%(contents:body)') };
}

function readRepositoryFile(relativePath, cwd) {
  return readFileSync(path.join(cwd, relativePath), 'utf8');
}

function readDefaultBranchFile(relativePath, defaultBranchSha, cwd) {
  const result = run('git', ['show', `${defaultBranchSha}:${relativePath}`], { cwd });
  if (result.status !== 0) {
    throw new Error(`${relativePath} is missing on the default branch at ${defaultBranchSha}.`);
  }
  return result.stdout;
}

function runBinaryVersion(binaryPath) {
  const result = run(binaryPath, ['--version']);
  if (result.status !== 0) {
    throw new Error(`${binaryPath} --version exited ${result.status}: ${String(result.stderr).trim()}`);
  }
  return result.stdout.trim();
}

export function defaultDependencies({ cwd = repoRoot } = {}) {
  return {
    verifyTag: (input) => verifyReleaseTag({ ...input, cwd }),
    resolveSourceAcceptance: (input) => resolveReleaseSourceAcceptance(input),
    captureGovernance: async (repository) => {
      const credential = resolveReadCredential();
      return captureGovernanceSnapshot({
        repository,
        credentialSource: credential.source,
        request: (endpoint) => githubRequest(endpoint, credential.value)
      });
    },
    readRepositoryFile: (relativePath) => readRepositoryFile(relativePath, cwd),
    readDefaultBranchFile: (relativePath, sha) => readDefaultBranchFile(relativePath, sha, cwd),
    loadSupport: () => loadReleaseSupport(path.join(cwd, '.github', 'release-support.json')),
    packageVersionPublished: (packageName, version) => packageVersionPublished(packageName, version),
    npmDistTagLatest,
    readTagAnnotation: (releaseTag) => readTagAnnotation(releaseTag, cwd),
    readArtifact: (relativePath) => readFileSync(path.join(cwd, relativePath)),
    runBinaryVersion: (relativePath) => runBinaryVersion(path.join(cwd, relativePath)),
    environment: () => ({
      node: process.version,
      npm: String(run('npm', ['--version']).stdout ?? '').trim() || null,
      runnerImage: process.env.ImageOS ? `${process.env.ImageOS} ${process.env.ImageVersion ?? ''}`.trim() : null
    }),
    now: () => new Date()
  };
}

/// Releases publish every package in the package set plus the wrapper.
export function releasePackageNames() {
  return [...wrapperPackageNameList(), ...nativeTargetNamesForPackageSet('post-intel').map(targetPackageName)];
}

export async function authorizeReleasePublication({
  repository,
  releaseTag,
  headSha,
  tagObjectSha,
  deps = defaultDependencies()
}) {
  const { npmVersion } = parseReleaseTag(releaseTag);
  const controls = [];
  const evidence = {};
  const check = async (id, requirement, evaluate) => {
    try {
      const detail = await evaluate();
      controls.push({ id, requirement, status: 'pass', detail: detail ?? 'matches' });
    } catch (error) {
      controls.push({ id, requirement, status: 'fail', detail: error?.message ?? String(error) });
    }
  };

  let tagVerification = null;
  await check('tag.reverified', 'the tag is still signed by a pinned signer, verified by GitHub, and on main', async () => {
    tagVerification = await deps.verifyTag({ repository, releaseTag });
    evidence.tagVerification = tagVerification;
    return `signed by ${tagVerification.signer.githubLogin} key ${tagVerification.signer.fingerprint}`;
  });
  await check('tag.unchanged', 'the tag names the same tag object and commit the builds started from', async () => {
    if (!tagVerification) {
      throw new Error('the tag could not be re-verified, so it cannot be compared with the preflight.');
    }
    if (tagVerification.tagObjectSha !== tagObjectSha || tagVerification.headSha !== headSha) {
      throw new Error(
        `${releaseTag} now names tag object ${tagVerification.tagObjectSha} at ${tagVerification.headSha}; the builds started from ${tagObjectSha} at ${headSha}.`
      );
    }
  });
  await check('source.exact-ci', 'the canonical CI push run for the exact commit still passed PR gate', async () => {
    const acceptance = await deps.resolveSourceAcceptance({ repository, releaseTag, headSha, tagObjectSha });
    evidence.sourceAcceptance = acceptance;
    return `CI run ${acceptance.workflow.runId} attempt ${acceptance.workflow.runAttempt}: ${acceptance.requiredChecks.map((entry) => `${entry.name}=${entry.conclusion}`).join(', ')}`;
  });
  await check('governance.protection', 'live protection satisfies the governance policy at the tagged commit and on main', async () => {
    const snapshot = await deps.captureGovernance(repository);
    const snapshotText = `${JSON.stringify(snapshot, null, 2)}\n`;
    evidence.governanceSnapshot = snapshot;
    const policies = [['tagged commit', deps.readRepositoryFile(GOVERNANCE_POLICY_PATH)]];
    if (tagVerification?.defaultBranchSha) {
      policies.push(['main', deps.readDefaultBranchFile(GOVERNANCE_POLICY_PATH, tagVerification.defaultBranchSha)]);
    } else {
      throw new Error('the default-branch policy could not be read because the tag was not re-verified.');
    }
    const failures = [];
    evidence.governance = {};
    for (const [label, policyText] of policies) {
      const evaluation = evaluateGovernance(JSON.parse(policyText), snapshot, {
        policySha256: sha256(policyText),
        snapshotSha256: sha256(snapshotText)
      });
      evidence.governance[label === 'main' ? 'defaultBranch' : 'taggedCommit'] = evaluation;
      for (const entry of evaluation.controls.filter((control) => control.status !== 'pass')) {
        failures.push(`${label} ${entry.id}: ${entry.detail}`);
      }
    }
    if (failures.length > 0) {
      throw new Error(failures.join('; '));
    }
    return `${evidence.governance.taggedCommit.controls.length} controls pass under both policies`;
  });

  let support = null;
  await check('support.release-line', 'the version is on the supported release line', async () => {
    support = deps.loadSupport();
    assertSupportedReleaseLine(support, npmVersion);
    return `${npmVersion} is on the ${support.supportedReleaseLine}.x line`;
  });
  await check('support.surfaces', 'every package this release publishes is a supported surface', async () => {
    support ??= deps.loadSupport();
    const refused = [];
    for (const packageName of releasePackageNames()) {
      try {
        assertPublishableSurface(support, packageName);
      } catch (error) {
        refused.push(error.message);
      }
    }
    if (refused.length > 0) {
      throw new Error(refused.join(' '));
    }
  });

  await check('version.unpublished', 'no package already has this version', async () => {
    const published = [];
    for (const packageName of releasePackageNames()) {
      if (deps.packageVersionPublished(packageName, npmVersion)) {
        published.push(`${packageName}@${npmVersion}`);
      }
    }
    if (published.length > 0) {
      throw new Error(`already on npm: ${published.join(', ')}. npm versions are immutable; bump forward.`);
    }
  });
  await check('version.monotonic', 'the version is newer than every package\'s current latest', async () => {
    const latest = {};
    for (const packageName of releasePackageNames()) {
      latest[packageName] = deps.npmDistTagLatest(packageName);
    }
    evidence.registryLatest = latest;
    const notNewer = Object.entries(latest).filter(([, version]) => compareVersions(npmVersion, version) <= 0);
    if (notNewer.length > 0) {
      throw new Error(
        `${npmVersion} is not newer than ${notNewer.map(([name, version]) => `${name} latest ${version}`).join(', ')}; publishing it would move latest backwards.`
      );
    }
  });

  await check('metadata.placeholders', 'source manifests still carry placeholders, so only the tag sets the version', async () => {
    const problems = [];
    const wrapper = JSON.parse(deps.readRepositoryFile('npm/coven/package.json'));
    if (wrapper.version !== '0.0.0') {
      problems.push(`npm/coven/package.json version is ${wrapper.version}, not the 0.0.0 placeholder`);
    }
    for (const [name, spec] of Object.entries(wrapper.optionalDependencies ?? {})) {
      if (name.startsWith('@opencoven/cli-') && spec !== '0.0.0') {
        problems.push(`npm/coven/package.json optionalDependencies ${name} is ${spec}, not 0.0.0`);
      }
    }
    if (!/"version":\s*"__VERSION__"/.test(deps.readRepositoryFile('npm/coven-platform-template/package.json.tpl'))) {
      problems.push('the platform package template no longer takes its version from __VERSION__');
    }
    if (!/^version = "0\.0\.0"$/m.test(deps.readRepositoryFile('crates/coven-cli/Cargo.toml'))) {
      problems.push('crates/coven-cli/Cargo.toml is no longer the 0.0.0 placeholder; the release build stamps the version');
    }
    if (problems.length > 0) {
      throw new Error(problems.join('; '));
    }
  });
  await check('artifacts.source-commit', 'every native binary embeds the exact tagged commit and version', async () => {
    const problems = [];
    evidence.artifacts = {};
    for (const [target, relativePath] of Object.entries(NATIVE_ARTIFACTS)) {
      let bytes;
      try {
        bytes = deps.readArtifact(relativePath);
      } catch (error) {
        problems.push(`${relativePath} is missing (${error.message})`);
        continue;
      }
      evidence.artifacts[target] = { path: relativePath, sha256: sha256(bytes), size: bytes.length };
      if (!bytes.includes(Buffer.from(headSha))) {
        problems.push(`${relativePath} does not embed the tagged commit ${headSha}`);
      }
      if (!bytes.includes(Buffer.from(`v${npmVersion}`))) {
        problems.push(`${relativePath} does not embed v${npmVersion}`);
      }
    }
    if (problems.length > 0) {
      throw new Error(problems.join('; '));
    }
  });
  await check('artifacts.reported-version', 'the runnable native binary reports the release version', async () => {
    const line = deps.runBinaryVersion(NATIVE_ARTIFACTS[RUNNABLE_TARGET]);
    evidence.reportedVersion = line;
    if (!new RegExp(`^coven v${npmVersion.replaceAll('.', '\\.')}(\\s|$)`).test(line)) {
      throw new Error(`${NATIVE_ARTIFACTS[RUNNABLE_TARGET]} --version printed ${JSON.stringify(line)}, expected coven v${npmVersion}.`);
    }
    return line;
  });
  await check('metadata.release-notes', 'the tag annotation is the release record: titled for this version, with notes', async () => {
    const { subject, body } = deps.readTagAnnotation(releaseTag);
    const notes = body.replace(/-----BEGIN SSH SIGNATURE-----[\s\S]*?-----END SSH SIGNATURE-----\s*/g, '').trim();
    evidence.releaseNotes = { subject, bodySha256: sha256(notes), bodyLength: notes.length };
    if (subject !== `Coven ${releaseTag}`) {
      throw new Error(`the ${releaseTag} annotation is titled ${JSON.stringify(subject)}, expected "Coven ${releaseTag}".`);
    }
    if (notes.length === 0) {
      throw new Error(`the ${releaseTag} annotation has no release notes; it becomes the GitHub Release body, the changelog of record.`);
    }
  });

  evidence.environment = deps.environment();
  return {
    schemaVersion: 1,
    kind: 'coven.release-publication-authorization',
    repository,
    releaseTag,
    npmVersion,
    headSha,
    tagObjectSha,
    authorized: controls.every((entry) => entry.status === 'pass'),
    controls,
    evidence,
    observedAt: deps.now().toISOString()
  };
}

async function main() {
  const options = parseOptions(process.argv.slice(2));
  const result = await authorizeReleasePublication({
    repository: requiredOption(options, 'repository'),
    releaseTag: requiredOption(options, 'release-tag'),
    headSha: requiredOption(options, 'head-sha'),
    tagObjectSha: requiredOption(options, 'tag-object-sha')
  });
  const outputDir = path.resolve(requiredOption(options, 'output-dir'));
  mkdirSync(outputDir, { recursive: true });
  writeFileSync(path.join(outputDir, 'release-publication-authorization.json'), `${JSON.stringify(result, null, 2)}\n`);
  for (const entry of result.controls) {
    const line = `${entry.status === 'pass' ? 'pass' : 'FAIL'} ${entry.id}: ${entry.detail}`;
    if (entry.status === 'pass') {
      console.log(line);
    } else {
      console.log(process.env.GITHUB_ACTIONS === 'true' ? `::error::Publication ${entry.id}: ${entry.detail}` : line);
    }
  }
  if (!result.authorized) {
    const failed = result.controls.filter((entry) => entry.status !== 'pass').length;
    console.error(`Refusing to publish ${result.releaseTag}: ${failed} of ${result.controls.length} publication controls failed.`);
    process.exit(1);
  }
  console.log(`Authorized ${result.releaseTag} for publication.`);
}

if (isMainModule()) {
  main().catch((error) => {
    console.error(error?.message ?? String(error));
    process.exit(1);
  });
}
