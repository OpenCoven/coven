import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

import {
  buildGateReceipt,
  loadRequiredChecksManifest,
  parseVerifyArgs,
  selectAcceptedWorkflowRun,
  verifyCommitRequiredChecks,
  verifyExactCommitGate,
  verifyRunJobEvidence
} from './verify-release-commit-gate.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const scriptPath = path.join(repoRoot, 'scripts', 'verify-release-commit-gate.mjs');
const realManifestPath = path.join(repoRoot, 'scripts', 'release-required-checks.json');
const REAL_MANIFEST = loadRequiredChecksManifest(readFileSync(realManifestPath, 'utf8'));

const COMMIT_SHA = '4f1c9d2ab77e0c6b5a3d8e1f2a4b5c6d7e8f9012';
const ANCESTOR_SHA = '0a1b2c3d4e5f60718293a4b5c6d7e8f90123456';
const TAG_OBJECT_SHA = 'b2c3d4e5f60718293a4b5c6d7e8f9012345678ab';

function syntheticManifest(overrides = {}) {
  return {
    schema: 'coven.release-required-checks/v1',
    source_workflow: {
      name: 'CI',
      path: '.github/workflows/ci.yml',
      event: 'push',
      branch: 'main'
    },
    policy: {},
    strict_checks: [
      { name: 'Policy guard', job_id: 'policy-guard' },
      { name: 'engine contract', job_id: 'engine-contract' }
    ],
    routed_checks: [
      { name: 'Rust tests (Linux)', job_id: 'rust-test-linux' },
      { name: 'Channels package', job_id: 'channels' }
    ],
    ...overrides
  };
}

function runPayload(overrides = {}) {
  return {
    id: 501,
    run_number: 12,
    run_attempt: 1,
    name: 'CI',
    event: 'push',
    status: 'completed',
    conclusion: 'success',
    head_branch: 'main',
    head_sha: COMMIT_SHA,
    path: '.github/workflows/ci.yml',
    html_url: 'https://github.com/OpenCoven/coven/actions/runs/501',
    ...overrides
  };
}

function checkPayload(name, overrides = {}) {
  return {
    name,
    head_sha: COMMIT_SHA,
    status: 'completed',
    conclusion: 'success',
    app: { slug: 'github-actions' },
    ...overrides
  };
}

// The gate's accepted evidence is the job list of the selected run attempt.
function jobPayload(name, overrides = {}) {
  return {
    name,
    id: 9000 + name.length,
    run_id: 501,
    run_url: 'https://github.com/OpenCoven/coven/actions/runs/501',
    head_sha: COMMIT_SHA,
    status: 'completed',
    conclusion: 'success',
    workflow_name: 'CI',
    ...overrides
  };
}

function selectedRunJobs({ manifest, routed = {}, overrides = {} } = {}) {
  const jobs = manifest.strict_checks.map((entry) => jobPayload(entry.name, overrides[entry.name]));
  for (const entry of manifest.routed_checks) {
    const observation = routed[entry.name] ?? 'success';
    if (observation === 'absent') {
      continue;
    }
    jobs.push(jobPayload(entry.name, { conclusion: observation, ...(overrides[entry.name] ?? {}) }));
  }
  return jobs;
}

function gateError({ manifest = syntheticManifest(), workflowRuns, runJobs, commitSha = COMMIT_SHA } = {}) {
  let message = null;
  try {
    verifyCommitRequiredChecks({ commitSha, manifest, workflowRuns, runJobs });
  } catch (error) {
    message = error.message;
  }
  assert.ok(message, 'expected the gate to refuse the evidence');
  return message;
}

function acceptedEvidence({ manifest = syntheticManifest(), workflowRuns, runJobs, commitSha = COMMIT_SHA } = {}) {
  return verifyCommitRequiredChecks({ commitSha, manifest, workflowRuns, runJobs });
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

test('manifest: loads the real repository manifest with strict and routed checks', () => {
  assert.equal(REAL_MANIFEST.schema, 'coven.release-required-checks/v1');
  assert.equal(REAL_MANIFEST.source_workflow.path, '.github/workflows/ci.yml');
  assert.equal(REAL_MANIFEST.source_workflow.event, 'push');
  assert.equal(REAL_MANIFEST.source_workflow.branch, 'main');
  assert.ok(REAL_MANIFEST.strict_checks.length > 0);
  assert.ok(REAL_MANIFEST.routed_checks.length > 0);
});

test('manifest: rejects invalid JSON', () => {
  assert.throws(() => loadRequiredChecksManifest('{nope'), /not valid JSON/);
});

test('manifest: rejects a foreign schema version', () => {
  assert.throws(
    () => loadRequiredChecksManifest(JSON.stringify({ schema: 'coven.release-required-checks/v0', strict_checks: [], routed_checks: [] })),
    /manifest schema must be/
  );
});

test('manifest: rejects an empty strict list', () => {
  const manifest = syntheticManifest();
  const text = JSON.stringify({
    schema: 'coven.release-required-checks/v1',
    source_workflow: manifest.source_workflow,
    strict_checks: [],
    routed_checks: manifest.routed_checks
  });
  assert.throws(() => loadRequiredChecksManifest(text), /strict_checks must be a non-empty array/);
});

test('manifest: rejects a check that is both strict and routed', () => {
  const manifest = syntheticManifest();
  const text = JSON.stringify({
    schema: 'coven.release-required-checks/v1',
    source_workflow: manifest.source_workflow,
    strict_checks: [{ name: 'Policy guard', job_id: 'policy-guard' }],
    routed_checks: [{ name: 'Policy guard', job_id: 'policy-guard' }]
  });
  assert.throws(() => loadRequiredChecksManifest(text), /both strict and routed/);
});

test('manifest: rejects duplicate names in one list', () => {
  const manifest = syntheticManifest();
  const text = JSON.stringify({
    schema: 'coven.release-required-checks/v1',
    source_workflow: manifest.source_workflow,
    strict_checks: [
      { name: 'Policy guard', job_id: 'policy-guard' },
      { name: 'Policy guard', job_id: 'policy-guard' }
    ],
    routed_checks: []
  });
  assert.throws(() => loadRequiredChecksManifest(text), /duplicate check name/);
});

test('manifest: rejects a source workflow outside .github/workflows/', () => {
  const manifest = syntheticManifest();
  const text = JSON.stringify({
    schema: 'coven.release-required-checks/v1',
    source_workflow: { ...manifest.source_workflow, path: 'ci.yml' },
    strict_checks: manifest.strict_checks,
    routed_checks: manifest.routed_checks
  });
  assert.throws(() => loadRequiredChecksManifest(text), /must point inside .github\/workflows\//);
});

test('manifest: rejects entries missing a job_id binding', () => {
  const manifest = syntheticManifest();
  const text = JSON.stringify({
    schema: 'coven.release-required-checks/v1',
    source_workflow: manifest.source_workflow,
    strict_checks: [{ name: 'Policy guard' }],
    routed_checks: manifest.routed_checks
  });
  assert.throws(() => loadRequiredChecksManifest(text), /job_id must be a non-empty string/);
});

test('manifest: loads pr_only checks and the pr_gate merge policy from the real manifest', () => {
  assert.ok(REAL_MANIFEST.pr_only_checks.length > 0);
  assert.equal(REAL_MANIFEST.pr_gate.aggregate_check.name, 'PR gate');
  assert.equal(REAL_MANIFEST.pr_gate.aggregate_check.job_id, 'pr-gate');
  assert.ok(REAL_MANIFEST.pr_gate.required_checks.length > 0);
  // Job identity is disjoint across the policy dimensions even when the
  // display name repeats across event scopes (npm onboarding smoke matrix).
  const releaseJobIds = new Set(
    [...REAL_MANIFEST.strict_checks, ...REAL_MANIFEST.routed_checks].map((entry) => entry.job_id)
  );
  for (const entry of REAL_MANIFEST.pr_only_checks) {
    assert.ok(!releaseJobIds.has(entry.job_id), `pr-only job ${entry.job_id} must be separate from release jobs`);
  }
  for (const entry of [...REAL_MANIFEST.pr_gate.required_checks, REAL_MANIFEST.pr_gate.aggregate_check]) {
    assert.ok(!releaseJobIds.has(entry.job_id) || entry.job_id === 'changes' || entry.job_id === 'policy-guard');
  }
  // The PR gate aggregate itself is a pull_request-only job.
  assert.ok(!releaseJobIds.has('pr-gate'));
});

test('manifest: rejects a job_id that is both a release required check and PR-only', () => {
  const base = JSON.parse(readFileSync(realManifestPath, 'utf8'));
  const collide = JSON.stringify({
    ...base,
    pr_only_checks: [{ name: 'Policy guard', job_id: 'policy-guard' }]
  });
  assert.throws(
    () => loadRequiredChecksManifest(collide),
    /cannot be both a release required check and PR-only/
  );
});

test('manifest: accepts the same check name in push and pull_request scopes (matrix legs)', () => {
  // npm onboarding smoke runs the same display names on both events via two
  // different jobs; evidence is run-scoped, so this is not ambiguity.
  const mainLeg = REAL_MANIFEST.strict_checks.filter((entry) => entry.job_id === 'npm-onboarding-main');
  const prLeg = REAL_MANIFEST.pr_only_checks.filter((entry) => entry.job_id === 'npm-onboarding-pr');
  assert.ok(mainLeg.length > 0 && prLeg.length > 0);
  assert.ok(prLeg.every((entry) => mainLeg.some((other) => other.name === entry.name)));
});

// --- Manifest completeness: nothing can silently narrow the required sets ---

function parseCiJobs(ciText) {
  const lines = ciText.split('\n');
  const jobsIndex = lines.findIndex((line) => line === 'jobs:');
  assert.ok(jobsIndex >= 0, 'ci.yml must declare jobs');
  const jobs = [];
  let current = null;
  for (const line of lines.slice(jobsIndex + 1)) {
    const jobMatch = /^  ([a-zA-Z0-9_-]+):\s*$/.exec(line);
    if (jobMatch) {
      current = { job_id: jobMatch[1], text: [] };
      jobs.push(current);
      continue;
    }
    if (/^[a-zA-Z#]/.test(line) && current) {
      break;
    }
    if (current) {
      current.text.push(line);
    }
  }
  return jobs.map(({ job_id, text }) => ({ job_id, block: text.join('\n') }));
}

function expectedCiCheckNames(ciText) {
  const names = [];
  for (const { job_id, block } of parseCiJobs(ciText)) {
    const nameMatch = /^ {4}name: (.+)$/m.exec(block);
    assert.ok(nameMatch, `job ${job_id} must declare a display name`);
    const rawName = nameMatch[1].trim();
    const matrixMatch = /\$\{\{\s*matrix\.npm-target\s*\}\}/.exec(rawName);
    if (matrixMatch) {
      let prefix = rawName.split('${{')[0].trim();
      if (prefix.endsWith('(')) {
        prefix = prefix.slice(0, -1).trim();
      }
      const targets = [...block.matchAll(/npm-target: ([a-z0-9-]+)/g)].map((match) => match[1]);
      assert.ok(targets.length > 0, `job ${job_id} matrix targets not found`);
      for (const target of targets) {
        names.push({ name: `${prefix} (${target})`, job_id });
      }
    } else {
      names.push({ name: rawName, job_id });
    }
  }
  return names;
}

test('manifest: covers every CI job exactly once — the required sets cannot silently narrow', () => {
  const ciText = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const expected = expectedCiCheckNames(ciText);
  const expectedNamesByJobId = new Map();
  for (const entry of expected) {
    expectedNamesByJobId.set(entry.job_id, [...(expectedNamesByJobId.get(entry.job_id) ?? []), entry.name]);
  }
  const manifestEntries = [
    ...REAL_MANIFEST.strict_checks,
    ...REAL_MANIFEST.routed_checks,
    ...REAL_MANIFEST.pr_only_checks,
    REAL_MANIFEST.pr_gate.aggregate_check,
    ...REAL_MANIFEST.pr_gate.required_checks
  ].filter(Boolean);

  // (a) Every manifest entry must be bound to a real CI job by its real name.
  const manifestJobIds = new Set();
  for (const entry of manifestEntries) {
    const expectedNames = expectedNamesByJobId.get(entry.job_id);
    assert.ok(
      expectedNames?.includes(entry.name),
      `manifest entry ${JSON.stringify(entry.name)} (${entry.job_id}) does not match ci.yml`
    );
    manifestJobIds.add(entry.job_id);
  }
  // (b) Every CI job must be claimed by the manifest (strict, routed, pr-only,
  // or pr-gate) — dropping an entry can no longer silently narrow the sets.
  for (const jobId of expectedNamesByJobId.keys()) {
    assert.ok(manifestJobIds.has(jobId), `ci.yml job ${jobId} is missing from the required-checks manifest`);
  }
  // (c) Per job, the claimed names must not exceed the real expanded names.
  for (const jobId of manifestJobIds) {
    const claimed = new Set(
      manifestEntries.filter((entry) => entry.job_id === jobId).map((entry) => entry.name)
    );
    for (const name of claimed) {
      assert.ok(
        expectedNamesByJobId.get(jobId).includes(name),
        `manifest claims unexpected name ${JSON.stringify(name)} for job ${jobId}`
      );
    }
  }
});

test('manifest: pr_gate merge checks are disjoint from push-only release jobs', () => {
  const ciText = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  // Release-only jobs are conditioned on the push event and must never be
  // required for merge, or pull requests could never satisfy them.
  for (const entry of [...REAL_MANIFEST.pr_gate.required_checks, REAL_MANIFEST.pr_gate.aggregate_check]) {
    const jobBlock = parseCiJobs(ciText).find((job) => job.job_id === entry.job_id)?.block ?? '';
    assert.ok(
      !/if: \$\{\{.*github\.event_name == 'push'/.test(jobBlock),
      `pr_gate entry ${entry.name} must not be bound to a push-only job`
    );
  }
});

// ---------------------------------------------------------------------------
// Exact source acceptance — aggregate run
// ---------------------------------------------------------------------------

test('happy path: exact commit with green aggregate, strict successes, and routed mix is accepted', () => {
  const manifest = syntheticManifest();
  const evidence = acceptedEvidence({
    manifest,
    workflowRuns: [runPayload()],
    runJobs: selectedRunJobs({ manifest, routed: { 'Channels package': 'skipped' } })
  });
  assert.equal(evidence.run.id, '501');
  assert.equal(evidence.run.conclusion, 'success');
  const byName = new Map(evidence.checks.map((check) => [check.name, check]));
  assert.equal(byName.get('Policy guard').class, 'strict');
  assert.equal(byName.get('Policy guard').conclusion, 'success');
  assert.equal(byName.get('Rust tests (Linux)').class, 'routed');
  assert.equal(byName.get('Channels package').conclusion, 'skipped');
});

test('accepts the real manifest when every strict check is green and routed checks skip or succeed', () => {
  const evidence = acceptedEvidence({
    manifest: REAL_MANIFEST,
    workflowRuns: [runPayload()],
    runJobs: selectedRunJobs({ manifest: REAL_MANIFEST, routed: { 'Rust tests (Linux)': 'skipped', 'Channels package': 'absent' } })
  });
  assert.equal(evidence.checks.length, REAL_MANIFEST.strict_checks.length + REAL_MANIFEST.routed_checks.length);
});

test('refuses when no CI run exists for the exact commit (missing)', () => {
  const message = gateError({ workflowRuns: [], runJobs: [] });
  assert.match(message, /no .* run .* exists for the exact commit/);
});

test('refuses when only a green ancestor commit has a run (exact target differs)', () => {
  const message = gateError({
    workflowRuns: [runPayload({ head_sha: ANCESTOR_SHA })],
    runJobs: []
  });
  assert.match(message, /green ancestor or unrelated commit is insufficient/);
});

test('refuses a candidate run whose payload head SHA differs from the release commit', () => {
  const message = gateError({
    workflowRuns: [runPayload({ head_sha: ANCESTOR_SHA })],
    runJobs: [],
    manifest: syntheticManifest()
  });
  assert.match(message, /green ancestor or unrelated commit is insufficient/);
});

test('refuses an in-flight run for the exact commit (stale evidence)', () => {
  const message = gateError({
    workflowRuns: [runPayload({ status: 'in_progress', conclusion: null })],
    runJobs: []
  });
  assert.match(message, /stale or in flight/);
});

test('refuses a queued run for the exact commit', () => {
  const message = gateError({
    workflowRuns: [runPayload({ status: 'queued', conclusion: null })],
    runJobs: []
  });
  assert.match(message, /stale or in flight/);
});

test('refuses a green older run when a newer attempt is in flight', () => {
  const message = gateError({
    workflowRuns: [
      runPayload({ id: 501, run_number: 12, status: 'completed', conclusion: 'success' }),
      runPayload({ id: 502, run_number: 13, status: 'in_progress', conclusion: null })
    ],
    runJobs: selectedRunJobs({ manifest: syntheticManifest() })
  });
  assert.match(message, /stale or in flight/);
});

test('refuses a failed aggregate run (exact tag target has a failing CI run)', () => {
  const message = gateError({
    workflowRuns: [runPayload({ conclusion: 'failure' })],
    runJobs: selectedRunJobs({ manifest: syntheticManifest() })
  });
  assert.match(message, /concluded "failure", not "success"/);
});

test('refuses a cancelled aggregate run', () => {
  const message = gateError({
    workflowRuns: [runPayload({ conclusion: 'cancelled' })],
    runJobs: selectedRunJobs({ manifest: syntheticManifest() })
  });
  assert.match(message, /concluded "cancelled"/);
});

test('refuses a timed-out aggregate run', () => {
  const message = gateError({
    workflowRuns: [runPayload({ conclusion: 'timed_out' })],
    runJobs: []
  });
  assert.match(message, /concluded "timed_out"/);
});

test('refuses a run for the exact commit on a non-main branch', () => {
  const message = gateError({
    workflowRuns: [runPayload({ head_branch: 'feature-branch' })],
    runJobs: []
  });
  assert.match(message, /exists for the exact commit/);
});

test('refuses a pull_request CI run as release evidence', () => {
  const message = gateError({
    workflowRuns: [runPayload({ event: 'pull_request' })],
    runJobs: []
  });
  assert.match(message, /exists for the exact commit/);
});

test('accepts and selects the newest run when several green runs exist for the exact commit', () => {
  const manifest = syntheticManifest();
  const evidence = acceptedEvidence({
    manifest,
    workflowRuns: [
      runPayload({ id: 501, run_number: 11 }),
      runPayload({ id: 502, run_number: 12 })
    ],
    runJobs: selectedRunJobs({ manifest }).map((job) => ({ ...job, run_id: 502 }))
  });
  assert.equal(evidence.run.id, '502');
});

test('refuses when two completed runs for the exact commit disagree', () => {
  const message = gateError({
    workflowRuns: [
      runPayload({ id: 501, run_number: 11, conclusion: 'success' }),
      runPayload({ id: 502, run_number: 12, conclusion: 'failure' })
    ],
    runJobs: selectedRunJobs({ manifest: syntheticManifest() })
  });
  assert.match(message, /concluded "failure"/);
});

// ---------------------------------------------------------------------------
// Strict and routed checks for the exact SHA
// ---------------------------------------------------------------------------

test('refuses when a strict required check is missing for the exact commit', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).filter((run) => run.name !== 'engine contract');
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /required check "engine contract" \(strict\) is missing/);
});

test('refuses when a strict required check was skipped for the exact commit (skipped-when-required)', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).map((run) =>
    run.name === 'Policy guard' ? { ...run, conclusion: 'skipped' } : run
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /required check "Policy guard" \(strict\) was skipped/);
});

test('refuses when a strict required check failed for the exact commit', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).map((run) =>
    run.name === 'engine contract' ? { ...run, conclusion: 'failure' } : run
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /required check "engine contract" \(strict\)/);
  assert.match(message, /concluded "failure"/);
});

test('refuses when a strict required check is still running', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).map((run) =>
    run.name === 'engine contract' ? { ...run, status: 'in_progress', conclusion: null } : run
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /required check "engine contract" \(strict\)/);
  assert.match(message, /is not completed/);
});

test('refuses when a strict required check was cancelled', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).map((run) =>
    run.name === 'Policy guard' ? { ...run, conclusion: 'cancelled' } : run
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /required check "Policy guard" \(strict\)/);
  assert.match(message, /concluded "cancelled"/);
});

test('accepts an absent routed check as routed-out and records it in the evidence', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest, routed: { 'Channels package': 'absent', 'Rust tests (Linux)': 'success' } });
  const evidence = acceptedEvidence({ manifest, workflowRuns: [runPayload()], runJobs });
  const channels = evidence.checks.find((check) => check.name === 'Channels package');
  assert.equal(channels.status, 'absent');
  assert.equal(channels.conclusion, 'routed-out');
});

test('accepts a skipped routed check', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest, routed: { 'Rust tests (Linux)': 'skipped' } });
  const evidence = acceptedEvidence({ manifest, workflowRuns: [runPayload()], runJobs });
  const rust = evidence.checks.find((check) => check.name === 'Rust tests (Linux)');
  assert.equal(rust.conclusion, 'skipped');
});

test('refuses a failed routed check', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest, routed: { 'Rust tests (Linux)': 'failure' } });
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /routed check "Rust tests \(Linux\)" .* concluded "failure"/);
});

test('refuses a routed check that is still running', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest, routed: { 'Channels package': 'absent' } }).map((run) =>
    run.name === 'Rust tests (Linux)' ? { ...run, status: 'in_progress', conclusion: null } : run
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /routed check "Rust tests \(Linux\)" .* is not completed/);
});

// ---------------------------------------------------------------------------
// Evidence hygiene — evidence must come from the selected run/attempt only
// ---------------------------------------------------------------------------

test('refuses a same-named job from a different check suite (cross-suite evidence cannot satisfy a required check)', () => {
  const manifest = syntheticManifest();
  // The job list contains a green "Policy guard" job, but it belongs to another
  // run (another check suite — e.g. a different workflow or an older attempt).
  // The selected run has no such job, so the strict check is missing.
  const runJobs = selectedRunJobs({ manifest }).map((job) =>
    job.name === 'Policy guard' ? { ...job, run_id: 999 } : job
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(
    message,
    /belongs to run 999, not the selected run 501\. Check evidence from a different check suite/
  );
});

test('verifyRunJobEvidence refuses cross-suite jobs outright, even when the name would match', () => {
  const manifest = syntheticManifest();
  const run = runPayload();
  assert.throws(
    () =>
      verifyRunJobEvidence({
        commitSha: COMMIT_SHA,
        run,
        manifest,
        runJobs: [{ ...jobPayload('Policy guard'), run_id: 999 }]
      }),
    /belongs to run 999, not the selected run 501/
  );
});

test('refuses a job whose workflow name differs from the selected source workflow', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).map((job) =>
    job.name === 'Policy guard' ? { ...job, workflow_name: 'Legacy CI' } : job
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /belongs to workflow "Legacy CI", not the source workflow "CI"/);
});

test('refuses a job in the selected run reporting a different head SHA', () => {
  const manifest = syntheticManifest();
  const runJobs = selectedRunJobs({ manifest }).map((job) =>
    job.name === 'Policy guard' ? { ...job, head_sha: ANCESTOR_SHA } : job
  );
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /job "Policy guard" in the selected run reports head SHA .* not the exact commit/);
});

test('refuses a job that ran in the selected CI run but is not declared in the manifest', () => {
  const manifest = syntheticManifest();
  const runJobs = [...selectedRunJobs({ manifest }), jobPayload('Brand new job')];
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /not declared in scripts\/release-required-checks\.json/);
});

test('refuses duplicate job names inside the selected run (ambiguous evidence)', () => {
  const manifest = syntheticManifest();
  const base = selectedRunJobs({ manifest });
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs: [...base, jobPayload('Policy guard')] });
  assert.match(message, /more than one job named "Policy guard"/);
});

test('refuses a selected-run job without a name', () => {
  const manifest = syntheticManifest();
  const runJobs = [...selectedRunJobs({ manifest }), jobPayload('')];
  const message = gateError({ manifest, workflowRuns: [runPayload()], runJobs });
  assert.match(message, /reported a job without a name/);
});

test('selectAcceptedWorkflowRun is exported and selects the newest green run', () => {
  const run = selectAcceptedWorkflowRun({
    commitSha: COMMIT_SHA,
    manifest: syntheticManifest(),
    workflowRuns: [runPayload({ id: 501, run_number: 11 }), runPayload({ id: 502, run_number: 12 })]
  });
  assert.equal(run.id, 502);
});

test('refuses a malformed release commit SHA', () => {
  const message = gateError({
    commitSha: 'not-a-sha',
    workflowRuns: [runPayload()],
    runJobs: selectedRunJobs({ manifest: syntheticManifest() })
  });
  assert.match(message, /must be a 40-hex git SHA/);
});

// ---------------------------------------------------------------------------
// Receipt
// ---------------------------------------------------------------------------

function acceptedReceipt() {
  const manifest = syntheticManifest();
  const evidence = acceptedEvidence({
    manifest,
    workflowRuns: [runPayload()],
    runJobs: selectedRunJobs({ manifest })
  });
  return buildGateReceipt({
    repository: 'OpenCoven/coven',
    commitSha: COMMIT_SHA,
    manifestDigest: 'f'.repeat(64),
    manifest,
    evidence,
    releaseTag: 'v0.4.2',
    tagObjectSha: TAG_OBJECT_SHA
  });
}

test('receipt: records the release SHA, run identity, check names/conclusions, and manifest digest', () => {
  const receipt = acceptedReceipt();
  assert.equal(receipt.schema, 'coven.release-commit-gate-receipt/v1');
  assert.equal(receipt.decision, 'accepted');
  assert.equal(receipt.repository, 'OpenCoven/coven');
  assert.equal(receipt.commit_sha, COMMIT_SHA);
  assert.equal(receipt.release_tag, 'v0.4.2');
  assert.equal(receipt.tag_object_sha, TAG_OBJECT_SHA);
  assert.equal(receipt.workflow_run.id, '501');
  assert.equal(receipt.workflow_run.head_sha, COMMIT_SHA);
  assert.equal(receipt.required_checks_manifest.sha256, 'f'.repeat(64));
  assert.equal(receipt.required_checks_manifest.strict_count, 2);
  assert.equal(receipt.required_checks_manifest.routed_count, 2);
  assert.ok(receipt.checks.some((check) => check.name === 'Policy guard' && check.conclusion === 'success'));
});

test('receipt: is deterministic and contains no timestamps or secret-bearing fields', () => {
  const first = JSON.parse(JSON.stringify(acceptedReceipt()));
  const second = JSON.parse(JSON.stringify(acceptedReceipt()));
  assert.deepEqual(first, second);
  const keys = Object.keys(first).sort();
  assert.deepEqual(keys, [
    'checks',
    'commit_sha',
    'decision',
    'generated_from',
    'release_tag',
    'repository',
    'required_checks_manifest',
    'schema',
    'source_workflow',
    'tag_object_sha',
    'workflow_run'
  ]);
  assert.match(JSON.stringify(first), /coven\.release-commit-gate-receipt\/v1/);
  assert.doesNotMatch(JSON.stringify(first), /created_at|updated_at|generated_at|token|password/);
});

test('receipt: rejects incoherent version metadata (non-stable tag)', () => {
  const manifest = syntheticManifest();
  const evidence = acceptedEvidence({ manifest, workflowRuns: [runPayload()], runJobs: selectedRunJobs({ manifest }) });
  assert.throws(
    () =>
      buildGateReceipt({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifestDigest: 'f'.repeat(64),
        manifest,
        evidence,
        releaseTag: 'v0.4'
      }),
    /must be a stable vX\.Y\.Z tag/
  );
});

test('receipt: rejects a malformed tag object SHA', () => {
  const manifest = syntheticManifest();
  const evidence = acceptedEvidence({ manifest, workflowRuns: [runPayload()], runJobs: selectedRunJobs({ manifest }) });
  assert.throws(
    () =>
      buildGateReceipt({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifestDigest: 'f'.repeat(64),
        manifest,
        evidence,
        tagObjectSha: 'deadbeef'
      }),
    /must be a 40-hex git SHA or null/
  );
});

// ---------------------------------------------------------------------------
// REST wrapper
// ---------------------------------------------------------------------------

test('REST wrapper: accepts a green exact commit and returns evidence plus receipt', async () => {
  const manifest = syntheticManifest();
  const endpoints = [];
  const ghApi = async (endpoint) => {
    endpoints.push(endpoint);
    if (endpoint.includes('/actions/runs?')) {
      return { total_count: 1, workflow_runs: [runPayload()] };
    }
    return { total_count: 4, jobs: selectedRunJobs({ manifest }) };
  };
  const { evidence, receipt } = await verifyExactCommitGate({
    repository: 'OpenCoven/coven',
    commitSha: COMMIT_SHA,
    manifest,
    manifestDigest: 'f'.repeat(64),
    releaseTag: 'v0.4.2',
    ghApi
  });
  assert.equal(endpoints.length, 2);
  assert.match(endpoints[0], /\/actions\/runs\?head_sha=/);
  // Evidence is bound to the selected run and attempt.
  assert.match(endpoints[1], /\/actions\/runs\/501\/attempts\/1\/jobs\?per_page=100&page=1/);
  assert.equal(evidence.run.conclusion, 'success');
  assert.equal(evidence.run.id, '501');
  assert.equal(evidence.run.run_attempt, 1);
  assert.equal(receipt.decision, 'accepted');
});

test('REST wrapper: fails closed when run results are paginated away (incomplete evidence)', async () => {
  const manifest = syntheticManifest();
  const ghApi = async () => ({ total_count: 2, workflow_runs: [runPayload()] });
  await assert.rejects(
    () =>
      verifyExactCommitGate({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifest,
        manifestDigest: 'f'.repeat(64),
        ghApi
      }),
    /refusing to decide on incomplete evidence/
  );
});

test('REST wrapper: fetches job pages for the selected run attempt until the reported total is collected', async () => {
  const manifest = syntheticManifest();
  const allJobs = selectedRunJobs({ manifest });
  const pageOne = allJobs.slice(0, 3);
  const pageTwo = allJobs.slice(3);
  const requestedEndpoints = [];
  const ghApi = async (endpoint) => {
    if (endpoint.includes('/actions/runs?')) {
      return { total_count: 1, workflow_runs: [runPayload()] };
    }
    const page = Number(/[?&]page=(\d+)/.exec(endpoint)?.[1] ?? 1);
    requestedEndpoints.push(endpoint);
    return {
      total_count: pageOne.length + pageTwo.length,
      jobs: page === 1 ? pageOne : pageTwo
    };
  };
  const { evidence } = await verifyExactCommitGate({
    repository: 'OpenCoven/coven',
    commitSha: COMMIT_SHA,
    manifest,
    manifestDigest: 'f'.repeat(64),
    ghApi
  });
  assert.equal(requestedEndpoints.length, 2);
  assert.match(requestedEndpoints[0], /\/actions\/runs\/501\/attempts\/1\/jobs\?per_page=100&page=1/);
  assert.match(requestedEndpoints[1], /\/actions\/runs\/501\/attempts\/1\/jobs\?per_page=100&page=2/);
  assert.ok(evidence.checks.length > 0);
});

test('REST wrapper: fails closed when job pages never reach the reported total', async () => {
  const manifest = syntheticManifest();
  const ghApi = async (endpoint) => {
    if (endpoint.includes('/actions/runs?')) {
      return { total_count: 1, workflow_runs: [runPayload()] };
    }
    return { total_count: 5000, jobs: selectedRunJobs({ manifest }) };
  };
  await assert.rejects(
    () =>
      verifyExactCommitGate({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifest,
        manifestDigest: 'f'.repeat(64),
        ghApi
      }),
    /truncated evidence/
  );
});

test('REST wrapper: fails closed when the job list payload has no usable total_count', async () => {
  const manifest = syntheticManifest();
  const ghApi = async (endpoint) => {
    if (endpoint.includes('/actions/runs?')) {
      return { total_count: 1, workflow_runs: [runPayload()] };
    }
    return { jobs: selectedRunJobs({ manifest }) };
  };
  await assert.rejects(
    () =>
      verifyExactCommitGate({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifest,
        manifestDigest: 'f'.repeat(64),
        ghApi
      }),
    /no usable total_count/
  );
});

test('REST wrapper: cross-suite negative — a same-named green check run from another check suite cannot satisfy the gate', async () => {
  const manifest = syntheticManifest();
  // The selected run's own job list lacks "Policy guard"; a green same-named
  // check run exists on the commit from a different workflow/check suite. The
  // gate never reads the check-runs listing, so it must refuse.
  const ghApi = async (endpoint) => {
    if (endpoint.includes('/actions/runs?')) {
      return { total_count: 1, workflow_runs: [runPayload()] };
    }
    if (endpoint.includes('/attempts/1/jobs')) {
      return {
        total_count: 3,
        jobs: selectedRunJobs({ manifest }).filter((job) => job.name !== 'Policy guard')
      };
    }
    throw new Error(`Unexpected endpoint ${endpoint}`);
  };
  await assert.rejects(
    () =>
      verifyExactCommitGate({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifest,
        manifestDigest: 'f'.repeat(64),
        ghApi
      }),
    /required check "Policy guard" \(strict\) is missing/
  );
});

test('REST wrapper: fails closed when the REST call errors', async () => {
  const manifest = syntheticManifest();
  const ghApi = async () => {
    throw new Error('HTTP 502');
  };
  await assert.rejects(
    () =>
      verifyExactCommitGate({
        repository: 'OpenCoven/coven',
        commitSha: COMMIT_SHA,
        manifest,
        manifestDigest: 'f'.repeat(64),
        ghApi
      }),
    /HTTP 502/
  );
});

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

test('CLI args: parses the required options and rejects incomplete invocations', () => {
  const options = parseVerifyArgs(['--repository', 'OpenCoven/coven', '--commit-sha', COMMIT_SHA, '--manifest', 'm.json']);
  assert.equal(options.get('repository'), 'OpenCoven/coven');
  assert.equal(options.get('commit-sha'), COMMIT_SHA);
  assert.equal(options.get('manifest'), 'm.json');
  assert.throws(() => parseVerifyArgs(['--repository', 'OpenCoven/coven']), /Missing required option --commit-sha/);
  assert.throws(() => parseVerifyArgs(['--repository']), /Missing value for --repository/);
});

test('CLI: exits non-zero for an unknown command without touching the network', () => {
  const result = spawnSync(process.execPath, [scriptPath, 'bless', '--repository', 'OpenCoven/coven'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Usage: verify-release-commit-gate\.mjs verify/);
});

test('CLI: exits non-zero when required options are missing', () => {
  const result = spawnSync(process.execPath, [scriptPath, 'verify', '--repository', 'OpenCoven/coven'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Missing required option --commit-sha/);
});

test('CLI: exits non-zero when the manifest file is unreadable', () => {
  const result = spawnSync(
    process.execPath,
    [scriptPath, 'verify', '--repository', 'OpenCoven/coven', '--commit-sha', COMMIT_SHA, '--manifest', 'scripts/does-not-exist.json'],
    { encoding: 'utf8', cwd: repoRoot }
  );
  assert.equal(result.status, 1);
  assert.match(result.stderr, /unreadable/);
});

test('CLI: exits non-zero when the manifest on disk does not satisfy the schema', () => {
  const scratch = path.join(repoRoot, 'npm', 'dist', '.verify-release-commit-gate-tests');
  mkdirSync(scratch, { recursive: true });
  const badManifestPath = path.join(scratch, 'bad-manifest.json');
  writeFileSync(badManifestPath, JSON.stringify({ schema: 'something/else' }));
  try {
    const result = spawnSync(
      process.execPath,
      [scriptPath, 'verify', '--repository', 'OpenCoven/coven', '--commit-sha', COMMIT_SHA, '--manifest', badManifestPath],
      { encoding: 'utf8', cwd: repoRoot }
    );
    assert.equal(result.status, 1);
    assert.match(result.stderr, /manifest schema must be/);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});
