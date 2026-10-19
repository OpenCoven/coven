#!/usr/bin/env node

// Verifies a pushed release tag before any release job trusts it (#805).
//
// The tag must be an annotated tag object that GitHub reports as verified, it
// must target a commit contained in the default branch, and its SSH signature
// must come from a trusted signer. Trusted signers are pinned in
// .github/release-signers.json by GitHub user id and SSH signing-key id. The
// public keys are read from GitHub at verification time, so no key material
// is committed, and a deleted key or a reassigned login fails closed.
//
// The signature must verify against the pins at the tagged commit and at the
// current tip of the default branch. Removing a signer on main therefore
// revokes it for every tag not yet released.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const RELEASE_SIGNERS_PATH = '.github/release-signers.json';
const SIGNERS_KIND = 'coven.release-signers';
const VERIFICATION_KIND = 'coven.release-tag-verification';
const SUPPORTED_KEY_TYPES = new Set(['ssh-ed25519', 'sk-ssh-ed25519@openssh.com']);
const SAFE_REF_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
const SAFE_LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;
const SIGNING_KEYS_PAGE_SIZE = 100;
const GOOD_SIGNATURE = /^Good "git" signature for (\S+) with (\S+) key (SHA256:[A-Za-z0-9+/]+=*)$/m;

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

async function main() {
  const options = parseOptions(process.argv.slice(2));
  const result = await verifyReleaseTag({
    repository: requiredOption(options, 'repository'),
    releaseTag: requiredOption(options, 'release-tag'),
    defaultBranch: options.get('default-branch') ?? 'main'
  });
  const outputPath = path.resolve(requiredOption(options, 'output'));
  mkdirSync(path.dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`);
  console.error(
    `tag=${result.releaseTag} verified=${result.github.verified} reason=${result.github.reason} ` +
      `target=${result.headSha} signer=${result.signer.principal} key=${result.signer.fingerprint}`
  );
  process.stdout.write(`head_sha=${result.headSha}\ntag_object_sha=${result.tagObjectSha}\n`);
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

function isSha(value) {
  return typeof value === 'string' && /^[0-9a-f]{40}$/.test(value);
}

function isPositiveInteger(value) {
  return Number.isSafeInteger(value) && value > 0;
}

/// The allowed-signers principal for a pin: the account's GitHub noreply
/// address. It is only a label inside the verification, derived rather than
/// pinned so the reviewed file holds identifiers, not addresses.
export function signerPrincipal(signer) {
  return `${signer.githubUserId}+${signer.githubLogin}@users.noreply.github.com`;
}

/// Parses and validates the reviewed signer pins. Anything unexpected is an
/// error rather than ignored: this file decides who can publish.
export function parseReleaseSigners(text, label = RELEASE_SIGNERS_PATH) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    throw new Error(`Refusing release: ${label} is not valid JSON (${error.message}).`);
  }
  if (parsed?.schemaVersion !== 1 || parsed?.kind !== SIGNERS_KIND) {
    throw new Error(`Refusing release: ${label} must be ${SIGNERS_KIND} schemaVersion 1.`);
  }
  const signers = parsed.signers;
  if (!Array.isArray(signers) || signers.length === 0) {
    throw new Error(`Refusing release: ${label} must pin at least one trusted signer.`);
  }
  const principals = new Set();
  const keyIds = new Set();
  return signers.map((signer, index) => {
    const where = `${label} signers[${index}]`;
    const fields = Object.keys(signer ?? {}).sort().join(',');
    if (fields !== 'githubLogin,githubUserId,keyType,sshSigningKeyId') {
      throw new Error(
        `Refusing release: ${where} must have exactly githubLogin, githubUserId, sshSigningKeyId and keyType.`
      );
    }
    if (!SAFE_LOGIN.test(signer.githubLogin)) {
      throw new Error(`Refusing release: ${where} githubLogin ${JSON.stringify(signer.githubLogin)} is not a GitHub login.`);
    }
    if (!isPositiveInteger(signer.githubUserId) || !isPositiveInteger(signer.sshSigningKeyId)) {
      throw new Error(`Refusing release: ${where} githubUserId and sshSigningKeyId must be positive integers.`);
    }
    if (!SUPPORTED_KEY_TYPES.has(signer.keyType)) {
      throw new Error(
        `Refusing release: ${where} keyType ${JSON.stringify(signer.keyType)} is not one of ${[...SUPPORTED_KEY_TYPES].join(', ')}.`
      );
    }
    const principal = signerPrincipal(signer);
    if (principals.has(principal) || keyIds.has(signer.sshSigningKeyId)) {
      throw new Error(`Refusing release: ${where} repeats a signer or signing key already pinned.`);
    }
    principals.add(principal);
    keyIds.add(signer.sshSigningKeyId);
    return { ...signer, principal };
  });
}

/// Resolves each pin to its public key through GitHub's public user APIs.
export async function resolveSignerKeys(signers, { ghApi }) {
  const resolved = [];
  for (const signer of signers) {
    const user = await ghApi(`/users/${encodeURIComponent(signer.githubLogin)}`);
    if (user?.id !== signer.githubUserId) {
      throw new Error(
        `Refusing release: GitHub login ${signer.githubLogin} now belongs to user ${JSON.stringify(user?.id)}, not pinned user ${signer.githubUserId}.`
      );
    }
    const keys = await ghApi(
      `/users/${encodeURIComponent(signer.githubLogin)}/ssh_signing_keys?per_page=${SIGNING_KEYS_PAGE_SIZE}`
    );
    if (!Array.isArray(keys) || keys.length >= SIGNING_KEYS_PAGE_SIZE) {
      throw new Error(`Refusing release: could not read the complete SSH signing-key list for ${signer.githubLogin}.`);
    }
    const matches = keys.filter((key) => key?.id === signer.sshSigningKeyId);
    if (matches.length !== 1) {
      throw new Error(
        `Refusing release: ${signer.githubLogin} has no SSH signing key ${signer.sshSigningKeyId} on GitHub; the pinned key was removed or never existed.`
      );
    }
    // Anything after the key blob is a comment and is dropped.
    const [keyType, keyBlob] = String(matches[0].key ?? '').trim().split(/\s+/);
    if (keyType !== signer.keyType || !/^[A-Za-z0-9+/]+=*$/.test(keyBlob ?? '')) {
      throw new Error(
        `Refusing release: SSH signing key ${signer.sshSigningKeyId} for ${signer.githubLogin} is not a ${signer.keyType} public key.`
      );
    }
    resolved.push({ ...signer, publicKey: `${keyType} ${keyBlob}` });
  }
  return resolved;
}

/// One `principal key-type key` line per signer, the allowed_signers subset
/// that has no options, wildcards, or comments to misread.
export function allowedSignersText(resolvedSigners) {
  return resolvedSigners.map((signer) => `${signer.principal} ${signer.publicKey}\n`).join('');
}

function git(args, { cwd, allowFailure = false } = {}) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8' });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0 && !allowFailure) {
    throw new Error(`git ${args.join(' ')} failed: ${String(result.stderr || result.stdout).trim()}`);
  }
  return result;
}

/// Verifies the tag signature against one allowed-signers set. A key outside
/// the set still prints `Good "git" signature with ...` and exits non-zero, so
/// both the exit status and the principal-bearing line are required.
export function verifyTagSignature({ cwd, releaseTag, allowedSigners }) {
  const workdir = mkdtempSync(path.join(tmpdir(), 'coven-release-signers-'));
  try {
    const signersFile = path.join(workdir, 'allowed_signers');
    writeFileSync(signersFile, allowedSigners);
    const result = git(
      [
        '-c', 'gpg.format=ssh',
        '-c', 'gpg.ssh.program=ssh-keygen',
        '-c', `gpg.ssh.allowedSignersFile=${signersFile}`,
        'verify-tag', `refs/tags/${releaseTag}`
      ],
      { cwd, allowFailure: true }
    );
    const output = `${result.stdout}\n${result.stderr}`;
    const good = GOOD_SIGNATURE.exec(output);
    if (result.status !== 0 || !good) {
      throw new Error(
        `Refusing release: ${releaseTag} is not signed by a trusted signer in ${RELEASE_SIGNERS_PATH}. git verify-tag said: ${output.trim().replace(/\s+/g, ' ')}`
      );
    }
    return { principal: good[1], keyAlgorithm: good[2], fingerprint: good[3] };
  } finally {
    rmSync(workdir, { recursive: true, force: true });
  }
}

async function ghApiJson(endpoint) {
  const result = spawnSync('gh', ['api', endpoint], { encoding: 'utf8' });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`gh api ${endpoint} failed: ${String(result.stderr).trim()}`);
  }
  return JSON.parse(result.stdout);
}

function sha256(text) {
  return createHash('sha256').update(text).digest('hex');
}

export async function verifyReleaseTag({
  repository,
  releaseTag,
  defaultBranch = 'main',
  cwd = process.cwd(),
  ghApi = ghApiJson,
  now = () => new Date()
}) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(String(repository ?? ''))) {
    throw new Error(`Refusing release: repository must be OWNER/REPO, got ${JSON.stringify(repository)}.`);
  }
  if (!SAFE_REF_NAME.test(String(releaseTag ?? '')) || !SAFE_REF_NAME.test(String(defaultBranch ?? ''))) {
    throw new Error(`Refusing release: unsupported tag or branch name ${JSON.stringify(releaseTag)} / ${JSON.stringify(defaultBranch)}.`);
  }

  const tagObjectSha = git(['rev-parse', '--verify', `refs/tags/${releaseTag}`], { cwd }).stdout.trim();
  const objectType = git(['cat-file', '-t', tagObjectSha], { cwd }).stdout.trim();
  if (objectType !== 'tag') {
    throw new Error(
      `Refusing release: ${releaseTag} is a lightweight tag, not an annotated signed tag. Push a signed tag for the next version with \`git tag -s\`.`
    );
  }

  const tagRef = await ghApi(`/repos/${repository}/git/ref/tags/${encodeURIComponent(releaseTag)}`);
  if (tagRef?.ref !== `refs/tags/${releaseTag}`) {
    throw new Error(`Refusing release: GitHub tag ref payload named ${JSON.stringify(tagRef?.ref)} instead of refs/tags/${releaseTag}.`);
  }
  if (tagRef?.object?.type !== 'tag' || !isSha(tagRef?.object?.sha)) {
    throw new Error(`Refusing release: GitHub tag ref for ${releaseTag} must resolve to an annotated tag object.`);
  }
  if (tagRef.object.sha !== tagObjectSha) {
    throw new Error(
      `Refusing release: local tag object ${tagObjectSha} does not match the GitHub tag object ${tagRef.object.sha} for ${releaseTag}.`
    );
  }

  const tagObject = await ghApi(`/repos/${repository}/git/tags/${tagObjectSha}`);
  if (tagObject?.tag !== releaseTag) {
    throw new Error(`Refusing release: GitHub tag payload named ${JSON.stringify(tagObject?.tag)} instead of ${releaseTag}.`);
  }
  if (tagObject?.verification?.verified !== true) {
    throw new Error(
      `Refusing release: ${releaseTag} does not have a GitHub-verified signature (reason=${tagObject?.verification?.reason ?? 'unknown'}). Register the signing key on the tagger's GitHub account.`
    );
  }
  if (tagObject?.object?.type !== 'commit' || !isSha(tagObject?.object?.sha)) {
    throw new Error(`Refusing release: annotated tag ${releaseTag} targets ${JSON.stringify(tagObject?.object?.type)}, not a commit.`);
  }
  const headSha = tagObject.object.sha;
  const localHeadSha = git(['rev-parse', '--verify', `refs/tags/${releaseTag}^{commit}`], { cwd }).stdout.trim();
  if (localHeadSha !== headSha) {
    throw new Error(
      `Refusing release: local tag ${releaseTag} resolves to ${localHeadSha}, not the GitHub-verified commit ${headSha}.`
    );
  }

  const trackingRef = `refs/remotes/origin/${defaultBranch}`;
  git(['fetch', '--no-tags', 'origin', `+refs/heads/${defaultBranch}:${trackingRef}`], { cwd });
  const defaultBranchSha = git(['rev-parse', '--verify', trackingRef], { cwd }).stdout.trim();
  const ancestry = git(['merge-base', '--is-ancestor', headSha, defaultBranchSha], { cwd, allowFailure: true });
  if (ancestry.status === 1) {
    throw new Error(`Refusing release: tagged commit ${headSha} is not contained in origin/${defaultBranch}.`);
  }
  if (ancestry.status !== 0) {
    throw new Error(`Refusing release: could not check that ${headSha} is on origin/${defaultBranch}: ${ancestry.stderr.trim()}`);
  }

  const pinSources = [];
  for (const [label, commit] of [['tagged commit', headSha], [`origin/${defaultBranch}`, defaultBranchSha]]) {
    const shown = git(['show', `${commit}:${RELEASE_SIGNERS_PATH}`], { cwd, allowFailure: true });
    if (shown.status !== 0) {
      throw new Error(`Refusing release: ${RELEASE_SIGNERS_PATH} is missing at the ${label} ${commit}.`);
    }
    const signers = parseReleaseSigners(shown.stdout, `${RELEASE_SIGNERS_PATH} at the ${label}`);
    const resolved = await resolveSignerKeys(signers, { ghApi });
    const signature = verifyTagSignature({ cwd, releaseTag, allowedSigners: allowedSignersText(resolved) });
    pinSources.push({ label, commit, sha256: sha256(shown.stdout), signature, signer: resolved.find((entry) => entry.principal === signature.principal) });
  }
  const [atTag, atBranch] = pinSources;
  if (
    atTag.signature.fingerprint !== atBranch.signature.fingerprint ||
    atTag.signer.sshSigningKeyId !== atBranch.signer.sshSigningKeyId
  ) {
    throw new Error(
      `Refusing release: ${releaseTag} verifies as different signers at the tagged commit and on origin/${defaultBranch}.`
    );
  }

  return {
    schemaVersion: 1,
    kind: VERIFICATION_KIND,
    repository,
    releaseTag,
    tagObjectSha,
    headSha,
    defaultBranch,
    defaultBranchSha,
    github: { verified: true, reason: tagObject.verification.reason ?? null },
    signer: {
      principal: atBranch.signer.principal,
      githubLogin: atBranch.signer.githubLogin,
      githubUserId: atBranch.signer.githubUserId,
      sshSigningKeyId: atBranch.signer.sshSigningKeyId,
      keyAlgorithm: atBranch.signature.keyAlgorithm,
      fingerprint: atBranch.signature.fingerprint
    },
    signerPins: {
      path: RELEASE_SIGNERS_PATH,
      taggedCommitSha256: atTag.sha256,
      defaultBranchSha256: atBranch.sha256
    },
    observedAt: now().toISOString()
  };
}

if (isMainModule()) {
  main().catch((error) => {
    const message = error?.message ?? String(error);
    console.error(process.env.GITHUB_ACTIONS === 'true' ? `::error::${message}` : message);
    process.exit(1);
  });
}
