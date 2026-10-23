import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import {
  DEFAULT_POLICY_PATH,
  READ_CREDENTIAL_ENV,
  captureGovernanceSnapshot,
  evaluateGovernance,
  githubRequest,
  refPatternMatches,
  resolveReadCredential,
  withoutHypermedia
} from './release-governance.mjs';

const scriptsDir = path.dirname(fileURLToPath(import.meta.url));
const scriptPath = path.join(scriptsDir, 'release-governance.mjs');
const fixturePath = path.join(scriptsDir, 'fixtures', 'release-governance', 'snapshot-2026-10-19.json');
const policy = JSON.parse(readFileSync(DEFAULT_POLICY_PATH, 'utf8'));
const liveSnapshot = JSON.parse(readFileSync(fixturePath, 'utf8'));
const releaseScriptText = readFileSync(path.join(scriptsDir, 'package-github-release.mjs'), 'utf8');
const ADMIN_ROLE = { actor_type: 'RepositoryRole', actor_id: 5, bypass_mode: 'always' };
const WRITE_ROLE = { actor_type: 'RepositoryRole', actor_id: 4, bypass_mode: 'always' };

const clone = (value) => structuredClone(value);

const PROPOSED_RULESET_FILES = ['main-protection.json', 'release-tags-immutable.json', 'release-tags-creation.json'];
const rulesetsDir = path.join(scriptsDir, '..', '.github', 'rulesets');

/// The state #805 asks for: the live 2026-10-19 snapshot plus the reviewed
/// ruleset bodies in .github/rulesets/, exactly as they will be submitted to
/// the rulesets API, with the repository settings switched on.
function compliantSnapshot() {
  const snapshot = clone(liveSnapshot);
  PROPOSED_RULESET_FILES.forEach((fileName, index) => {
    const id = 30000001 + index;
    const body = { id, ...JSON.parse(readFileSync(path.join(rulesetsDir, fileName), 'utf8')), source_type: 'Repository', source: 'OpenCoven/coven' };
    snapshot.responses.rulesetPages[0].body.push({ id, name: body.name, target: body.target });
    snapshot.responses.rulesets.push({
      endpoint: `/repos/OpenCoven/coven/rulesets/${id}?includes_parents=true`,
      status: 200,
      body
    });
  });
  snapshot.responses.classicProtection.body.enforce_admins.enabled = true;
  snapshot.responses.branch.body.protection.required_status_checks.enforcement_level = 'everyone';
  snapshot.responses.immutableReleases.body.enabled = true;
  snapshot.responses.actionsPermissions.body.sha_pinning_required = true;
  return snapshot;
}

function rulesetBody(snapshot, name) {
  const found = snapshot.responses.rulesets.find((entry) => entry.body.name === name);
  assert.ok(found, `fixture has no ruleset ${name}`);
  return found.body;
}

function failedControls(snapshot) {
  return evaluateGovernance(policy, snapshot).controls
    .filter((control) => control.status !== 'pass')
    .map((control) => control.id);
}

function control(snapshot, id) {
  const found = evaluateGovernance(policy, snapshot).controls.find((entry) => entry.id === id);
  assert.ok(found, `evaluation has no control ${id}`);
  return found;
}

test('the reviewed policy names the same required source check as the release verifier', () => {
  assert.match(releaseScriptText, /const REQUIRED_SOURCE_CHECKS = \['PR gate'\];/);
  assert.deepEqual(
    policy.branch.rules.required_status_checks.checks.map((check) => check.context),
    ['PR gate']
  );
  assert.equal(policy.branch.rules.pull_request.required_approving_review_count, 0);
  assert.deepEqual(policy.releaseTags.creationBypassActors, [ADMIN_ROLE]);
});

test('the 2026-10-19 live snapshot fails every control #805 still owns', () => {
  const evaluation = evaluateGovernance(policy, liveSnapshot);
  assert.equal(evaluation.compliant, false);
  assert.deepEqual(failedControls(liveSnapshot), [
    'branch.pull_request',
    'branch.required_status_checks',
    'branch.non_fast_forward',
    'branch.deletion',
    'branch.classic-protection',
    'tags.update',
    'tags.deletion',
    'tags.creation',
    'repo.immutable-releases',
    'repo.actions-sha-pinning'
  ]);
  assert.match(
    control(liveSnapshot, 'branch.classic-protection').detail,
    /exempts administrators.*non-administrators only/
  );
  assert.equal(control(liveSnapshot, 'branch.merge_queue').status, 'pass');
});

test('the proposed settings satisfy every control', () => {
  const evaluation = evaluateGovernance(policy, compliantSnapshot(), {
    policySha256: 'a'.repeat(64),
    snapshotSha256: 'b'.repeat(64)
  });
  assert.equal(evaluation.compliant, true, JSON.stringify(evaluation.controls, null, 2));
  assert.equal(evaluation.controls.length, 14);
  assert.equal(evaluation.kind, 'coven.release-governance-evaluation');
  assert.equal(evaluation.policySha256, 'a'.repeat(64));
  assert.equal(evaluation.snapshotSha256, 'b'.repeat(64));
  assert.equal(evaluation.snapshotCapturedAt, liveSnapshot.capturedAt);
});

const mutations = [
  ['a missing pull-request rule', 'branch.pull_request', /enforces pull_request on refs\/heads\/main/, (snapshot) => {
    const body = rulesetBody(snapshot, 'main-protection');
    body.rules = body.rules.filter((rule) => rule.type !== 'pull_request');
  }],
  ['unresolved review threads allowed', 'branch.pull_request', /required_review_thread_resolution is false/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').rules
      .find((rule) => rule.type === 'pull_request').parameters.required_review_thread_resolution = false;
  }],
  ['the required check accepted from any app', 'branch.required_status_checks', /"PR gate" from app 15368/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').rules
      .find((rule) => rule.type === 'required_status_checks').parameters.required_status_checks = [{ context: 'PR gate' }];
  }],
  ['the required check from a different app', 'branch.required_status_checks', /"PR gate" from app 15368/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').rules
      .find((rule) => rule.type === 'required_status_checks').parameters.required_status_checks = [{ context: 'PR gate', integration_id: 1 }];
  }],
  ['an administrator bypass on main', 'branch.non_fast_forward', /RepositoryRole:5\/always bypass it/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').bypass_actors = [ADMIN_ROLE];
  }],
  ['a pull-request-only administrator bypass on main', 'branch.pull_request', /RepositoryRole:5\/pull_request bypass it/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').bypass_actors = [{ ...ADMIN_ROLE, bypass_mode: 'pull_request' }];
  }],
  ['bypass actors hidden from the reader', 'branch.deletion', /not visible to this reader; capture the snapshot with RELEASE_GOVERNANCE_READ_TOKEN/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').bypass_actors = null;
  }],
  ['a ruleset in evaluate mode', 'branch.pull_request', /no active branch ruleset/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').enforcement = 'evaluate';
  }],
  ['a ruleset that excludes main', 'branch.pull_request', /no active branch ruleset/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').conditions.ref_name.exclude = ['refs/heads/main'];
  }],
  ['an unknown exclude token', 'branch.pull_request', /no active branch ruleset/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').conditions.ref_name.exclude = ['~SOMETHING_NEW'];
  }],
  ['a ruleset check the release never verifies', 'branch.required-checks-verified', /requires "Security scan", which release acceptance does not verify/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').rules
      .find((rule) => rule.type === 'required_status_checks').parameters.required_status_checks.push({ context: 'Security scan', integration_id: 15368 });
  }],
  ['a classic check the release never verifies', 'branch.required-checks-verified', /requires "Docs build"/, (snapshot) => {
    snapshot.responses.classicProtection.body.required_status_checks.contexts.push('Docs build');
  }],
  ['the merge queue removed', 'branch.merge_queue', /enforces merge_queue/, (snapshot) => {
    rulesetBody(snapshot, 'merge-queue-main').rules = [];
  }],
  ['classic protection exempting administrators', 'branch.classic-protection', /enforce_admins is not enabled/, (snapshot) => {
    snapshot.responses.classicProtection.body.enforce_admins.enabled = false;
  }],
  ['classic protection hidden from the reader', 'branch.classic-protection', /HTTP 403/, (snapshot) => {
    snapshot.responses.classicProtection = { endpoint: '/x', status: 403, body: { message: 'Resource not accessible by integration' } };
  }],
  ['an ambiguous classic-protection 404', 'branch.classic-protection', /HTTP 404 \(Not Found\)/, (snapshot) => {
    snapshot.responses.classicProtection = { endpoint: '/x', status: 404, body: { message: 'Not Found' } };
  }],
  ['status checks enforced for non-administrators only', 'branch.classic-protection', /non-administrators only/, (snapshot) => {
    snapshot.responses.branch.body.protection.required_status_checks.enforcement_level = 'non_admins';
  }],
  ['crate tags left mutable', 'tags.update', /enforces update on refs\/tags\/coven-client-v0\.1\.0$/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-immutable').conditions.ref_name.include = ['refs/tags/v*'];
  }],
  ['an administrator bypass on immutable tags', 'tags.deletion', /RepositoryRole:5\/always bypass it/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-immutable').bypass_actors = [ADMIN_ROLE];
  }],
  ['tag creation open to writers', 'tags.creation', /RepositoryRole:4\/always.*policy allows exactly \[RepositoryRole:5\/always\]/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-creation').bypass_actors = [ADMIN_ROLE, WRITE_ROLE];
  }],
  ['a silent exempt bypass on tag creation', 'tags.creation', /RepositoryRole:5\/exempt/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-creation').bypass_actors = [{ ...ADMIN_ROLE, bypass_mode: 'exempt' }];
  }],
  ['a single user allowed to bypass main', 'branch.pull_request', /User:68980965\/always bypass it/, (snapshot) => {
    rulesetBody(snapshot, 'main-protection').bypass_actors = [{ actor_type: 'User', actor_id: 68980965, bypass_mode: 'always' }];
  }],
  ['tag creation closed to everyone', 'tags.creation', /allows creation by \[\]/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-creation').bypass_actors = [];
  }],
  ['no tag-creation rule', 'tags.creation', /so any writer can create them/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-creation').rules = [];
  }],
  ['tag-creation bypass actors hidden from the reader', 'tags.creation', /not visible to this reader/, (snapshot) => {
    rulesetBody(snapshot, 'release-tags-creation').bypass_actors = null;
  }],
  ['immutable releases switched off', 'repo.immutable-releases', /enabled is false/, (snapshot) => {
    snapshot.responses.immutableReleases.body.enabled = false;
  }],
  ['immutable releases hidden from the reader', 'repo.immutable-releases', /HTTP 403/, (snapshot) => {
    snapshot.responses.immutableReleases = { endpoint: '/x', status: 403, body: { message: 'Forbidden' } };
  }],
  ['actions SHA pinning switched off', 'repo.actions-sha-pinning', /sha_pinning_required is false/, (snapshot) => {
    snapshot.responses.actionsPermissions.body.sha_pinning_required = false;
  }],
  ['write-by-default workflow tokens', 'repo.default-workflow-permissions', /"write"/, (snapshot) => {
    snapshot.responses.workflowPermissions.body.default_workflow_permissions = 'write';
  }],
  ['workflows allowed to approve pull requests', 'repo.workflow-review-approval', /can_approve_pull_request_reviews is true/, (snapshot) => {
    snapshot.responses.workflowPermissions.body.can_approve_pull_request_reviews = true;
  }]
];

for (const [label, controlId, detail, mutate] of mutations) {
  test(`fails closed on ${label}`, () => {
    const snapshot = compliantSnapshot();
    mutate(snapshot);
    const found = control(snapshot, controlId);
    assert.equal(found.status, 'fail');
    assert.match(found.detail, detail);
    assert.equal(evaluateGovernance(policy, snapshot).compliant, false);
  });
}

test('an unreadable ruleset list fails every ruleset control', () => {
  const snapshot = compliantSnapshot();
  snapshot.responses.rulesetPages = [{ endpoint: '/x', status: 403, body: { message: 'Forbidden' } }];
  const failed = failedControls(snapshot);
  assert.deepEqual(failed, [
    'branch.pull_request',
    'branch.required_status_checks',
    'branch.merge_queue',
    'branch.non_fast_forward',
    'branch.deletion',
    'branch.required-checks-verified',
    'tags.update',
    'tags.deletion',
    'tags.creation'
  ]);
  assert.match(control(snapshot, 'tags.creation').detail, /ruleset list was unreadable: HTTP 403/);
});

test('ruleset details that disagree with the list fail closed', () => {
  const dropped = compliantSnapshot();
  dropped.responses.rulesets.pop();
  assert.match(control(dropped, 'branch.pull_request').detail, /do not match the ruleset list/);

  const swapped = compliantSnapshot();
  swapped.responses.rulesets.reverse();
  assert.match(control(swapped, 'branch.pull_request').detail, /names #/);

  const hidden = compliantSnapshot();
  hidden.responses.rulesets[1].status = 404;
  assert.match(control(hidden, 'branch.pull_request').detail, /was unreadable: HTTP 404/);
});

test('an organization ruleset can carry the main-branch rules', () => {
  const snapshot = compliantSnapshot();
  const body = rulesetBody(snapshot, 'main-protection');
  body.source_type = 'Organization';
  body.source = 'OpenCoven';
  body.conditions = {
    ref_name: { include: ['~ALL'], exclude: [] },
    repository_name: { include: ['coven'], exclude: [], protected: true }
  };
  assert.equal(evaluateGovernance(policy, snapshot).compliant, true);
});

test('the evaluation refuses a snapshot or policy it cannot interpret', () => {
  assert.throws(
    () => evaluateGovernance(policy, { ...liveSnapshot, repository: 'OpenCoven/other' }),
    /snapshot is for "OpenCoven\/other"/
  );
  assert.throws(() => evaluateGovernance(policy, { ...liveSnapshot, schemaVersion: 2 }), /schemaVersion 1/);
  assert.throws(() => evaluateGovernance({ ...policy, kind: 'other' }, liveSnapshot), /schemaVersion 1/);
  const withoutSetting = clone(policy);
  delete withoutSetting.repositorySettings.immutableReleases;
  assert.throws(() => evaluateGovernance(withoutSetting, liveSnapshot), /repositorySettings\.immutableReleases/);
  const unprobed = clone(policy);
  unprobed.releaseTags.patterns.push('refs/tags/coven-sdk-v*');
  assert.throws(() => evaluateGovernance(unprobed, liveSnapshot), /no probe ref for release tag pattern refs\/tags\/coven-sdk-v\*/);
});

test('ref patterns follow ruleset fnmatch semantics and refuse unknown tokens', () => {
  const options = { defaultBranch: 'main' };
  assert.equal(refPatternMatches('refs/tags/v*', 'refs/tags/v0.4.11', options), true);
  assert.equal(refPatternMatches('refs/tags/v*', 'refs/tags/wip/v1', options), false);
  assert.equal(refPatternMatches('refs/tags/**', 'refs/tags/wip/v1', options), true);
  assert.equal(refPatternMatches('refs/tags/v?.*', 'refs/tags/v1.2', options), true);
  assert.equal(refPatternMatches('refs/tags/v0.4.1', 'refs/tags/v0x4x1', options), false);
  assert.equal(refPatternMatches('~DEFAULT_BRANCH', 'refs/heads/main', options), true);
  assert.equal(refPatternMatches('~DEFAULT_BRANCH', 'refs/heads/dev', options), false);
  assert.equal(refPatternMatches('~ALL', 'refs/tags/anything', options), true);
  assert.equal(refPatternMatches('~SOMETHING_NEW', 'refs/heads/main', options), null);
  assert.equal(refPatternMatches('', 'refs/heads/main', options), null);
});

test('a snapshot reads every protection surface and records no credential', async () => {
  const requested = [];
  const responses = new Map([
    ['/repos/OpenCoven/coven/rulesets?includes_parents=true&per_page=100&page=1', {
      status: 200,
      body: Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }))
    }],
    ['/repos/OpenCoven/coven/rulesets?includes_parents=true&per_page=100&page=2', {
      status: 200,
      body: [{ id: 101, node_id: 'RRS_example', _links: { self: { href: 'https://api.github.com/x' } } }]
    }],
    ['/repos/OpenCoven/coven/branches/main', {
      status: 200,
      body: { name: 'main', protected: true, protection: { enabled: true }, commit: { sha: 'f'.repeat(40) } }
    }]
  ]);
  const snapshot = await captureGovernanceSnapshot({
    repository: 'OpenCoven/coven',
    credentialSource: READ_CREDENTIAL_ENV,
    now: () => new Date('2026-10-19T00:00:00Z'),
    request: async (endpoint) => {
      requested.push(endpoint);
      if (responses.has(endpoint)) {
        return responses.get(endpoint);
      }
      const id = /\/rulesets\/(\d+)\?/.exec(endpoint)?.[1];
      return id ? { status: 200, body: { id: Number(id) } } : { status: 404, body: { message: 'Branch not protected' } };
    }
  });
  assert.equal(snapshot.responses.rulesetPages.length, 2);
  assert.equal(snapshot.responses.rulesets.length, 101);
  assert.equal(snapshot.responses.rulesets[100].endpoint, '/repos/OpenCoven/coven/rulesets/101?includes_parents=true');
  assert.deepEqual(snapshot.responses.rulesetPages[1].body, [{ id: 101 }]);
  assert.deepEqual(snapshot.responses.branch.body, { name: 'main', protected: true, protection: { enabled: true } });
  assert.deepEqual(snapshot.reader, { credentialSource: READ_CREDENTIAL_ENV });
  assert.equal(snapshot.capturedAt, '2026-10-19T00:00:00.000Z');
  assert.deepEqual(requested.slice(-5), [
    '/repos/OpenCoven/coven/branches/main',
    '/repos/OpenCoven/coven/branches/main/protection',
    '/repos/OpenCoven/coven/immutable-releases',
    '/repos/OpenCoven/coven/actions/permissions',
    '/repos/OpenCoven/coven/actions/permissions/workflow'
  ]);
  await assert.rejects(
    captureGovernanceSnapshot({ repository: 'not a repo', request: async () => ({}) }),
    /OWNER\/REPO/
  );
});

test('the retained fixture is exactly what a capture writes', () => {
  assert.deepEqual(withoutHypermedia(liveSnapshot), liveSnapshot);
  assert.deepEqual(
    withoutHypermedia({ node_id: 'x', a: [{ _links: {}, b: 1 }], c: null }),
    { a: [{ b: 1 }], c: null }
  );
});

test('the read credential prefers the dedicated governance secret', () => {
  const noGh = () => '';
  assert.deepEqual(
    resolveReadCredential({ [READ_CREDENTIAL_ENV]: ' scoped ', GH_TOKEN: 'workflow' }, noGh),
    { source: READ_CREDENTIAL_ENV, value: 'scoped' }
  );
  assert.deepEqual(resolveReadCredential({ GH_TOKEN: 'workflow' }, noGh), { source: 'GH_TOKEN', value: 'workflow' });
  assert.deepEqual(resolveReadCredential({ [READ_CREDENTIAL_ENV]: ' ' }, () => 'local'), { source: 'gh auth token', value: 'local' });
  assert.throws(() => resolveReadCredential({}, noGh), /No GitHub credential/);
});

test('a REST read returns 4xx evidence instead of throwing', async () => {
  let seen;
  const result = await githubRequest('/repos/OpenCoven/coven/branches/main/protection', 'scoped', async (url, init) => {
    seen = { url, init };
    return new Response(JSON.stringify({ message: 'Branch not protected' }), { status: 404 });
  });
  assert.deepEqual(result, { status: 404, body: { message: 'Branch not protected' } });
  assert.equal(seen.url, 'https://api.github.com/repos/OpenCoven/coven/branches/main/protection');
  assert.equal(seen.init.headers.authorization, 'Bearer scoped');
  assert.equal(seen.init.headers['x-github-api-version'], '2022-11-28');
  const unparsed = await githubRequest('/x', 'scoped', async () => new Response('<html>', { status: 502 }));
  assert.deepEqual(unparsed, { status: 502, body: { unparsedBody: '<html>' } });
});

test('the evaluate command writes the evaluation and exits non-zero on any gap', () => {
  const workdir = mkdtempSync(path.join(tmpdir(), 'release-governance-'));
  try {
    const compliantPath = path.join(workdir, 'compliant.json');
    writeFileSync(compliantPath, `${JSON.stringify(compliantSnapshot(), null, 2)}\n`);
    for (const [snapshotPath, expectedStatus] of [[fixturePath, 1], [compliantPath, 0]]) {
      const outputPath = path.join(workdir, `evaluation-${expectedStatus}.json`);
      const result = spawnSync(process.execPath, [scriptPath, 'evaluate', '--snapshot', snapshotPath, '--output', outputPath], {
        encoding: 'utf8',
        env: { ...process.env, GITHUB_ACTIONS: '' }
      });
      assert.equal(result.status, expectedStatus, result.stderr);
      const evaluation = JSON.parse(readFileSync(outputPath, 'utf8'));
      assert.equal(evaluation.compliant, expectedStatus === 0);
      assert.match(evaluation.policySha256, /^[0-9a-f]{64}$/);
      assert.match(evaluation.snapshotSha256, /^[0-9a-f]{64}$/);
      if (expectedStatus === 1) {
        assert.match(result.stderr, /FAIL tags\.creation/);
        assert.match(result.stderr, /Refusing release: 10 of 14 release governance controls/);
      }
    }
  } finally {
    rmSync(workdir, { recursive: true, force: true });
  }
});

function docTableRows(markdown, heading) {
  const section = markdown.split(`\n## ${heading}\n`)[1]?.split('\n## ')[0];
  assert.ok(section, `release-governance.md has no "## ${heading}" section`);
  return section.split('\n')
    .map((line) => /^\| `([a-z.\-_]+)` \|(.*)\|$/.exec(line))
    .filter(Boolean)
    .map((match) => ({ id: match[1], cells: match[2].split('|').map((cell) => cell.trim()) }));
}

test('the governance page lists every control and its 2026-10-19 status', () => {
  const markdown = readFileSync(path.join(scriptsDir, '..', 'docs', 'reference', 'release-governance.md'), 'utf8');
  const evaluation = evaluateGovernance(policy, liveSnapshot);
  const ids = evaluation.controls.map((entry) => entry.id);
  assert.deepEqual(docTableRows(markdown, 'Required state').map((row) => row.id), ids);
  const observed = docTableRows(markdown, 'Current versus required');
  assert.deepEqual(observed.map((row) => row.id), ids);
  for (const row of observed) {
    const expected = evaluation.controls.find((entry) => entry.id === row.id).status === 'pass' ? 'Pass' : 'Fail';
    assert.equal(row.cells.at(-1), expected, `${row.id} status in release-governance.md`);
  }
});
