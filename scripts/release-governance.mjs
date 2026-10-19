#!/usr/bin/env node

// Compares live GitHub protection for this repository with the reviewed policy
// in .github/release-governance.json (#805). Release publication runs this
// before any registry or GitHub Release effect. Every gap fails closed,
// including evidence the reading credential cannot see: a ruleset whose bypass
// actors are hidden from the reader is not proof that nobody can bypass it.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
export const DEFAULT_POLICY_PATH = path.join(repoRoot, '.github', 'release-governance.json');
const GITHUB_API_URL = 'https://api.github.com';
const GITHUB_API_VERSION = '2022-11-28';
const REQUEST_TIMEOUT_MS = 30_000;
const RULESET_PAGE_SIZE = 100;
const RULESET_PAGE_LIMIT = 10;
export const READ_CREDENTIAL_ENV = 'RELEASE_GOVERNANCE_READ_TOKEN';
const CREDENTIAL_ENV_ORDER = [READ_CREDENTIAL_ENV, 'GH_TOKEN', 'GITHUB_TOKEN'];
const POLICY_KIND = 'coven.release-governance-policy';
const SNAPSHOT_KIND = 'coven.release-governance-snapshot';
const EVALUATION_KIND = 'coven.release-governance-evaluation';
const REPOSITORY_SETTING_FIELDS = [
  'immutableReleases',
  'actionsShaPinningRequired',
  'defaultWorkflowPermissions',
  'workflowsCanApprovePullRequestReviews'
];
const UNREADABLE_BYPASS_HINT =
  `capture the snapshot with ${READ_CREDENTIAL_ENV} (a fine-grained credential with Administration: read on this repository)`;

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  const options = parseOptions(args);
  switch (command) {
    case 'snapshot': {
      const snapshot = await captureGovernanceSnapshot({
        repository: requiredOption(options, 'repository'),
        branch: options.get('branch') ?? 'main',
        ...githubReader()
      });
      writeJson(requiredOption(options, 'output'), snapshot);
      return;
    }
    case 'evaluate': {
      const policyPath = options.get('policy') ?? DEFAULT_POLICY_PATH;
      const snapshotPath = requiredOption(options, 'snapshot');
      const evaluation = evaluateGovernance(readJson(policyPath), readJson(snapshotPath), {
        policySha256: sha256File(policyPath),
        snapshotSha256: sha256File(snapshotPath)
      });
      finish(evaluation, requiredOption(options, 'output'));
      return;
    }
    case 'check': {
      const policyPath = options.get('policy') ?? DEFAULT_POLICY_PATH;
      const policy = readJson(policyPath);
      const snapshotPath = requiredOption(options, 'snapshot-output');
      const snapshot = await captureGovernanceSnapshot({
        repository: requiredOption(options, 'repository'),
        branch: policyBranchName(policy),
        ...githubReader()
      });
      writeJson(snapshotPath, snapshot);
      const evaluation = evaluateGovernance(policy, snapshot, {
        policySha256: sha256File(policyPath),
        snapshotSha256: sha256File(snapshotPath)
      });
      finish(evaluation, requiredOption(options, 'output'));
      return;
    }
    default:
      throw new Error(
        'Usage: release-governance.mjs <snapshot|evaluate|check> [--option value ...]\n' +
          '  snapshot --repository OWNER/REPO [--branch main] --output FILE\n' +
          '  evaluate --snapshot FILE [--policy FILE] --output FILE\n' +
          '  check    --repository OWNER/REPO --snapshot-output FILE --output FILE [--policy FILE]'
      );
  }
}

function finish(evaluation, outputPath) {
  writeJson(outputPath, evaluation);
  for (const control of evaluation.controls) {
    const line = `${control.status === 'pass' ? 'pass' : 'FAIL'} ${control.id}: ${control.detail}`;
    if (control.status === 'pass') {
      console.log(line);
    } else if (process.env.GITHUB_ACTIONS === 'true') {
      console.log(`::error::Release governance ${control.id}: ${control.detail}`);
    } else {
      console.error(line);
    }
  }
  if (!evaluation.compliant) {
    const failed = evaluation.controls.filter((control) => control.status !== 'pass').length;
    console.error(
      `Refusing release: ${failed} of ${evaluation.controls.length} release governance controls do not match .github/release-governance.json.`
    );
    process.exitCode = 1;
  }
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

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, value) {
  const resolved = path.resolve(filePath);
  mkdirSync(path.dirname(resolved), { recursive: true });
  writeFileSync(resolved, `${JSON.stringify(value, null, 2)}\n`);
}

function sha256File(filePath) {
  return createHash('sha256').update(readFileSync(filePath)).digest('hex');
}

/// Which credential reads protection, without ever recording its value.
///
/// The release workflow passes RELEASE_GOVERNANCE_READ_TOKEN: the default
/// GITHUB_TOKEN cannot see ruleset bypass actors or classic protection, so a
/// snapshot taken with it can never pass. Locally, `gh auth token` is enough
/// for a repository administrator.
export function resolveReadCredential(env = process.env, ghAuth = ghAuthCredential) {
  for (const name of CREDENTIAL_ENV_ORDER) {
    const value = env[name]?.trim();
    if (value) {
      return { source: name, value };
    }
  }
  const value = ghAuth();
  if (value) {
    return { source: 'gh auth token', value };
  }
  throw new Error(
    `No GitHub credential available to read repository protection. Set ${CREDENTIAL_ENV_ORDER.join(', ')}, or sign in with gh.`
  );
}

function ghAuthCredential() {
  const result = spawnSync('gh', ['auth', 'token'], { encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : '';
}

function githubReader() {
  const credential = resolveReadCredential();
  return {
    credentialSource: credential.source,
    request: (endpoint) => githubRequest(endpoint, credential.value)
  };
}

/// One REST read. Returns the HTTP status with the parsed body rather than
/// throwing on 4xx: a 404 "Branch not protected" is evidence, not a failure.
export async function githubRequest(endpoint, credential, fetchImpl = fetch) {
  const response = await fetchImpl(`${GITHUB_API_URL}${endpoint}`, {
    headers: {
      accept: 'application/vnd.github+json',
      authorization: `Bearer ${credential}`,
      'x-github-api-version': GITHUB_API_VERSION
    },
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS)
  });
  const text = await response.text();
  let body = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      body = { unparsedBody: text.slice(0, 500) };
    }
  }
  return { status: response.status, body };
}

function policyBranchName(policy) {
  const ref = policy?.branch?.ref;
  if (typeof ref !== 'string' || !ref.startsWith('refs/heads/')) {
    throw new Error(`Release governance policy branch.ref must be a refs/heads/ ref, got ${JSON.stringify(ref)}.`);
  }
  return ref.slice('refs/heads/'.length);
}

/// Drops GraphQL node ids and hypermedia links. They carry no protection
/// evidence, and their high-entropy ids would make every retained snapshot
/// look like a credential to the repository secret scan.
export function withoutHypermedia(value) {
  if (Array.isArray(value)) {
    return value.map(withoutHypermedia);
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([key]) => key !== 'node_id' && key !== '_links')
        .map(([key, entry]) => [key, withoutHypermedia(entry)])
    );
  }
  return value;
}

export async function captureGovernanceSnapshot({
  repository,
  branch = 'main',
  request,
  credentialSource,
  now = () => new Date()
}) {
  const normalizedRepository = String(repository ?? '').trim();
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(normalizedRepository)) {
    throw new Error(`Repository must be OWNER/REPO, got ${JSON.stringify(repository)}.`);
  }
  const base = `/repos/${normalizedRepository}`;
  const read = async (endpoint) => {
    const { status, body } = await request(endpoint);
    return { endpoint, status, body: withoutHypermedia(body) };
  };

  const rulesetPages = [];
  const rulesetSummaries = [];
  for (let page = 1; ; page += 1) {
    if (page > RULESET_PAGE_LIMIT) {
      throw new Error(`Refusing to snapshot more than ${RULESET_PAGE_LIMIT * RULESET_PAGE_SIZE} rulesets.`);
    }
    const response = await read(`${base}/rulesets?includes_parents=true&per_page=${RULESET_PAGE_SIZE}&page=${page}`);
    rulesetPages.push(response);
    if (response.status !== 200 || !Array.isArray(response.body)) {
      break;
    }
    rulesetSummaries.push(...response.body);
    if (response.body.length < RULESET_PAGE_SIZE) {
      break;
    }
  }
  const rulesets = [];
  for (const summary of rulesetSummaries) {
    rulesets.push(await read(`${base}/rulesets/${encodeURIComponent(summary?.id)}?includes_parents=true`));
  }

  const branchSummary = await read(`${base}/branches/${encodeURIComponent(branch)}`);
  const capturedAt = now();
  return {
    schemaVersion: 1,
    kind: SNAPSHOT_KIND,
    repository: normalizedRepository,
    branch,
    capturedAt: capturedAt.toISOString(),
    reader: { credentialSource: credentialSource ?? 'unknown' },
    responses: {
      rulesetPages,
      rulesets,
      classicProtection: await read(`${base}/branches/${encodeURIComponent(branch)}/protection`),
      // The branch payload also embeds the tip commit; only protection is evidence.
      branch: {
        endpoint: branchSummary.endpoint,
        status: branchSummary.status,
        body:
          branchSummary.status === 200
            ? {
                name: branchSummary.body?.name,
                protected: branchSummary.body?.protected,
                protection: branchSummary.body?.protection
              }
            : branchSummary.body
      },
      immutableReleases: await read(`${base}/immutable-releases`),
      actionsPermissions: await read(`${base}/actions/permissions`),
      workflowPermissions: await read(`${base}/actions/permissions/workflow`)
    }
  };
}

/// Matches one ruleset ref_name condition entry against a full ref.
///
/// Returns null for a token this verifier does not understand, so callers can
/// fail closed in whichever direction is safe: an unknown include never counts
/// as coverage, and an unknown exclude always counts as excluding.
export function refPatternMatches(pattern, ref, { defaultBranch }) {
  if (typeof pattern !== 'string' || pattern.length === 0) {
    return null;
  }
  if (pattern === '~ALL') {
    return true;
  }
  if (pattern === '~DEFAULT_BRANCH') {
    return ref === `refs/heads/${defaultBranch}`;
  }
  if (pattern.startsWith('~')) {
    return null;
  }
  let source = '';
  for (let index = 0; index < pattern.length; index += 1) {
    const char = pattern[index];
    if (char === '*' && pattern[index + 1] === '*') {
      source += '.*';
      index += 1;
    } else if (char === '*') {
      source += '[^/]*';
    } else if (char === '?') {
      source += '[^/]';
    } else if ('[]{}()+.^$|\\'.includes(char)) {
      source += `\\${char}`;
    } else {
      source += char;
    }
  }
  return new RegExp(`^${source}$`).test(ref);
}

function rulesetCoversRef(ruleset, ref, defaultBranch) {
  const include = ruleset?.conditions?.ref_name?.include;
  const exclude = ruleset?.conditions?.ref_name?.exclude ?? [];
  if (!Array.isArray(include) || !Array.isArray(exclude)) {
    return false;
  }
  if (!include.some((pattern) => refPatternMatches(pattern, ref, { defaultBranch }) === true)) {
    return false;
  }
  return !exclude.some((pattern) => refPatternMatches(pattern, ref, { defaultBranch }) !== false);
}

function describeRuleset(ruleset) {
  return `${JSON.stringify(ruleset?.name ?? 'unnamed')} (#${ruleset?.id ?? '?'})`;
}

function describeActors(actors) {
  return actors
    .map((actor) => `${actor?.actor_type ?? '?'}:${actor?.actor_id ?? '?'}/${actor?.bypass_mode ?? '?'}`)
    .join(', ');
}

function bypassState(ruleset) {
  if (!Array.isArray(ruleset?.bypass_actors)) {
    return 'unreadable';
  }
  return ruleset.bypass_actors.length === 0 ? 'none' : 'present';
}

function sameActorSet(actual, expected) {
  const key = (actor) => `${actor?.actor_type}|${actor?.actor_id ?? 'null'}|${actor?.bypass_mode}`;
  const actualKeys = new Set(actual.map(key));
  const expectedKeys = new Set(expected.map(key));
  return actualKeys.size === actual.length &&
    actualKeys.size === expectedKeys.size &&
    [...expectedKeys].every((entry) => actualKeys.has(entry));
}

/// Returns a problem string when a rule's parameters are weaker than policy.
///
/// A numeric policy value is a minimum, `true` must be matched exactly, and
/// `false` imposes nothing. Required checks must name the policy's app, since a
/// context with no integration id can be satisfied by any status sender.
function ruleParameterProblem(ruleType, required, parameters) {
  if (ruleType === 'required_status_checks') {
    const actual = Array.isArray(parameters?.required_status_checks) ? parameters.required_status_checks : [];
    const missing = (required?.checks ?? []).filter(
      (check) => !actual.some(
        (entry) => entry?.context === check.context && entry?.integration_id === check.integration_id
      )
    );
    return missing.length === 0
      ? null
      : `does not require ${missing.map((check) => `${JSON.stringify(check.context)} from app ${check.integration_id}`).join(', ')}`;
  }
  for (const [key, expected] of Object.entries(required ?? {})) {
    const actual = parameters?.[key];
    if (typeof expected === 'number' && !(Number.isFinite(actual) && actual >= expected)) {
      return `${key} is ${JSON.stringify(actual)}, policy requires at least ${expected}`;
    }
    if (expected === true && actual !== true) {
      return `${key} is ${JSON.stringify(actual)}, policy requires true`;
    }
  }
  return null;
}

/// A rule passes for a ref when some active ruleset covering that ref enforces
/// it with no bypass actors and parameters at least as strict as policy.
function evaluateNoBypassRule({ rulesets, target, refs, ruleType, required, defaultBranch }) {
  const gaps = [];
  const uncovered = [];
  for (const ref of refs) {
    const candidates = rulesets.filter(
      (ruleset) =>
        ruleset?.target === target &&
        ruleset?.enforcement === 'active' &&
        rulesetCoversRef(ruleset, ref, defaultBranch) &&
        (ruleset?.rules ?? []).some((rule) => rule?.type === ruleType)
    );
    if (candidates.length === 0) {
      uncovered.push(ref);
      continue;
    }
    const problems = [];
    let satisfied = false;
    for (const ruleset of candidates) {
      const rule = ruleset.rules.find((entry) => entry?.type === ruleType);
      const parameterProblem = ruleParameterProblem(ruleType, required, rule?.parameters);
      const bypass = bypassState(ruleset);
      if (bypass === 'none' && !parameterProblem) {
        satisfied = true;
        break;
      }
      if (parameterProblem) {
        problems.push(`${describeRuleset(ruleset)} ${parameterProblem}`);
      }
      if (bypass === 'present') {
        problems.push(`${describeRuleset(ruleset)} lets ${describeActors(ruleset.bypass_actors)} bypass it`);
      }
      if (bypass === 'unreadable') {
        problems.push(`${describeRuleset(ruleset)} bypass actors are not visible to this reader; ${UNREADABLE_BYPASS_HINT}`);
      }
    }
    if (!satisfied) {
      gaps.push(`${ref}: ${problems.join('; ')}`);
    }
  }
  if (uncovered.length > 0) {
    gaps.unshift(`no active ${target} ruleset enforces ${ruleType} on ${uncovered.join(', ')}`);
  }
  return gaps;
}

/// Creation is the one release-tag rule with an intended bypass: someone has
/// to be able to push a release tag. Every creation ruleset covering a release
/// ref must allow exactly the policy's creators, no more and no fewer.
function evaluateCreationRule({ rulesets, refs, expectedActors, defaultBranch }) {
  const gaps = [];
  const uncovered = [];
  for (const ref of refs) {
    const candidates = rulesets.filter(
      (ruleset) =>
        ruleset?.target === 'tag' &&
        ruleset?.enforcement === 'active' &&
        rulesetCoversRef(ruleset, ref, defaultBranch) &&
        (ruleset?.rules ?? []).some((rule) => rule?.type === 'creation')
    );
    if (candidates.length === 0) {
      uncovered.push(ref);
      continue;
    }
    for (const ruleset of candidates) {
      if (bypassState(ruleset) === 'unreadable') {
        gaps.push(`${ref}: ${describeRuleset(ruleset)} bypass actors are not visible to this reader; ${UNREADABLE_BYPASS_HINT}`);
      } else if (!sameActorSet(ruleset.bypass_actors, expectedActors)) {
        gaps.push(
          `${ref}: ${describeRuleset(ruleset)} allows creation by [${describeActors(ruleset.bypass_actors)}], policy allows exactly [${describeActors(expectedActors)}]`
        );
      }
    }
  }
  if (uncovered.length > 0) {
    gaps.unshift(`no active tag ruleset restricts creation of ${uncovered.join(', ')}, so any writer can create them`);
  }
  return gaps;
}

function responseProblem(response, label) {
  if (!response || typeof response !== 'object') {
    return `${label} is missing from the snapshot`;
  }
  if (response.status !== 200) {
    const message = response.body?.message ? ` (${response.body.message})` : '';
    return `${label} was unreadable: HTTP ${response.status}${message}`;
  }
  return null;
}

function readRulesets(snapshot) {
  const pages = snapshot?.responses?.rulesetPages;
  if (!Array.isArray(pages) || pages.length === 0) {
    return { problem: 'ruleset list is missing from the snapshot' };
  }
  for (const page of pages) {
    const problem = responseProblem(page, 'ruleset list');
    if (problem) {
      return { problem };
    }
    if (!Array.isArray(page.body)) {
      return { problem: 'ruleset list response is not an array' };
    }
  }
  const listedIds = pages.flatMap((page) => page.body.map((summary) => summary?.id));
  const details = snapshot.responses.rulesets;
  if (!Array.isArray(details) || details.length !== listedIds.length) {
    return { problem: 'ruleset details do not match the ruleset list' };
  }
  const rulesets = [];
  for (const [index, detail] of details.entries()) {
    const problem = responseProblem(detail, `ruleset #${listedIds[index]}`);
    if (problem) {
      return { problem };
    }
    if (detail.body?.id !== listedIds[index]) {
      return { problem: `ruleset detail ${index} names #${detail.body?.id}, the list names #${listedIds[index]}` };
    }
    rulesets.push(detail.body);
  }
  return { rulesets };
}

function classicProtectionGaps(snapshot) {
  const gaps = [];
  const classic = snapshot?.responses?.classicProtection;
  if (classic?.status === 404 && classic?.body?.message === 'Branch not protected') {
    // Absent classic protection leaves rulesets as the only protection layer.
  } else if (classic?.status === 200) {
    if (classic.body?.enforce_admins?.enabled !== true) {
      gaps.push('classic branch protection exempts administrators (enforce_admins is not enabled)');
    }
  } else {
    gaps.push(`${responseProblem(classic, 'classic branch protection') ?? 'classic branch protection is unreadable'}; ${UNREADABLE_BYPASS_HINT}`);
  }
  const branch = snapshot?.responses?.branch;
  const branchProblem = responseProblem(branch, 'branch summary');
  if (branchProblem) {
    gaps.push(branchProblem);
  } else if (branch.body?.protection?.required_status_checks?.enforcement_level === 'non_admins') {
    gaps.push('branch summary reports required status checks enforced for non-administrators only');
  }
  return gaps;
}

function settingGap(response, label, field, expected) {
  const problem = responseProblem(response, label);
  if (problem) {
    return `${problem}; ${UNREADABLE_BYPASS_HINT}`;
  }
  const actual = response.body?.[field];
  return actual === expected ? null : `${label} ${field} is ${JSON.stringify(actual)}, policy requires ${JSON.stringify(expected)}`;
}

function assertPolicy(policy) {
  if (policy?.schemaVersion !== 1 || policy?.kind !== POLICY_KIND) {
    throw new Error(`Release governance policy must be ${POLICY_KIND} schemaVersion 1.`);
  }
  policyBranchName(policy);
  const tags = policy.releaseTags;
  if (!Array.isArray(tags?.probeRefs) || tags.probeRefs.length === 0 ||
    !tags.probeRefs.every((ref) => typeof ref === 'string' && ref.startsWith('refs/tags/'))) {
    throw new Error('Release governance policy releaseTags.probeRefs must list refs/tags/ refs.');
  }
  for (const pattern of tags.patterns ?? []) {
    if (!tags.probeRefs.some((ref) => refPatternMatches(pattern, ref, { defaultBranch: '' }) === true)) {
      throw new Error(`Release governance policy has no probe ref for release tag pattern ${pattern}.`);
    }
  }
  if (!Array.isArray(tags.immutableRules) || !Array.isArray(tags.creationBypassActors)) {
    throw new Error('Release governance policy must list releaseTags.immutableRules and creationBypassActors.');
  }
  for (const field of REPOSITORY_SETTING_FIELDS) {
    if (policy.repositorySettings?.[field] === undefined) {
      throw new Error(`Release governance policy must set repositorySettings.${field}.`);
    }
  }
}

export function evaluateGovernance(policy, snapshot, { policySha256 = null, snapshotSha256 = null } = {}) {
  assertPolicy(policy);
  if (snapshot?.schemaVersion !== 1 || snapshot?.kind !== SNAPSHOT_KIND) {
    throw new Error(`Release governance snapshot must be ${SNAPSHOT_KIND} schemaVersion 1.`);
  }
  if (snapshot.repository !== policy.repository) {
    throw new Error(
      `Release governance snapshot is for ${JSON.stringify(snapshot.repository)}, policy is for ${JSON.stringify(policy.repository)}.`
    );
  }
  const defaultBranch = policyBranchName(policy);
  const controls = [];
  const record = (id, requirement, gaps) => {
    controls.push({
      id,
      requirement,
      status: gaps.length === 0 ? 'pass' : 'fail',
      detail: gaps.length === 0 ? 'matches policy' : gaps.join('; ')
    });
  };

  const { rulesets, problem: rulesetProblem } = readRulesets(snapshot);
  const rulesetGaps = (evaluate) => (rulesetProblem ? [`${rulesetProblem}; ${UNREADABLE_BYPASS_HINT}`] : evaluate());

  for (const [ruleType, required] of Object.entries(policy.branch.rules ?? {})) {
    record(
      `branch.${ruleType}`,
      `${policy.branch.ref} enforces ${ruleType} for everyone, administrators included`,
      rulesetGaps(() => evaluateNoBypassRule({
        rulesets,
        target: 'branch',
        refs: [policy.branch.ref],
        ruleType,
        required,
        defaultBranch
      }))
    );
  }
  record(
    'branch.classic-protection',
    'classic branch protection is absent or binds administrators',
    classicProtectionGaps(snapshot)
  );
  for (const ruleType of policy.releaseTags.immutableRules) {
    record(
      `tags.${ruleType}`,
      `release tags reject ${ruleType} by everyone; changing that is a recorded ruleset edit`,
      rulesetGaps(() => evaluateNoBypassRule({
        rulesets,
        target: 'tag',
        refs: policy.releaseTags.probeRefs,
        ruleType,
        required: {},
        defaultBranch
      }))
    );
  }
  record(
    'tags.creation',
    'only the policy creators can create release tags',
    rulesetGaps(() => evaluateCreationRule({
      rulesets,
      refs: policy.releaseTags.probeRefs,
      expectedActors: policy.releaseTags.creationBypassActors,
      defaultBranch
    }))
  );

  const settings = policy.repositorySettings;
  const responses = snapshot.responses ?? {};
  const settingControls = [
    ['repo.immutable-releases', 'published releases and their tags are immutable',
      () => settingGap(responses.immutableReleases, 'immutable releases', 'enabled', settings.immutableReleases)],
    ['repo.actions-sha-pinning', 'every workflow action is pinned to a full commit SHA',
      () => settingGap(responses.actionsPermissions, 'Actions permissions', 'sha_pinning_required', settings.actionsShaPinningRequired)],
    ['repo.default-workflow-permissions', 'workflow tokens default to read-only',
      () => settingGap(responses.workflowPermissions, 'workflow permissions', 'default_workflow_permissions', settings.defaultWorkflowPermissions)],
    ['repo.workflow-review-approval', 'workflows cannot approve pull requests',
      () => settingGap(responses.workflowPermissions, 'workflow permissions', 'can_approve_pull_request_reviews', settings.workflowsCanApprovePullRequestReviews)]
  ];
  for (const [id, requirement, evaluate] of settingControls) {
    const gap = evaluate();
    record(id, requirement, gap ? [gap] : []);
  }

  return {
    schemaVersion: 1,
    kind: EVALUATION_KIND,
    repository: policy.repository,
    policySha256,
    snapshotSha256,
    snapshotCapturedAt: snapshot.capturedAt ?? null,
    reader: snapshot.reader ?? null,
    compliant: controls.every((control) => control.status === 'pass'),
    controls
  };
}

if (isMainModule()) {
  main().catch((error) => {
    console.error(error?.message ?? error);
    process.exit(1);
  });
}
