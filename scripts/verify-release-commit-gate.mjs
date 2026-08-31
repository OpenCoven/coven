#!/usr/bin/env node

// Exact source acceptance gate for Coven releases (issue #805).
//
// Publication is an authorization decision tied to the exact source commit being
// released. A signed tag proves who cut the release; this gate proves that the
// tagged commit itself passed the repository's required quality/security policy.
// It resolves the CI workflow run for the exact release SHA and refuses to accept
// the commit unless every required check for that SHA succeeded:
//
//   - the CI run must exist on main for the exact SHA (a green ancestor commit,
//     an unrelated workflow, or a missing run is rejected);
//   - the run must be completed with conclusion success (an in-flight, stale,
//     cancelled, or failed run is rejected);
//   - evidence is bound to the *selected* run and attempt: the only accepted
//     check evidence is the job list of that exact workflow-run attempt
//     (GET /actions/runs/{id}/attempts/{attempt}/jobs). A same-named check run
//     from any other check suite — another workflow, a different attempt, or a
//     third-party integration — can never satisfy a required check because it
//     is not a job of the selected run;
//   - every job that ran in the selected run must be a declared required check
//     (strict or routed) and report an allowed conclusion, so the manifest can
//     never silently narrow what a release actually depends on.
//
// Evidence for the decision is emitted as a deterministic machine-readable
// receipt (schema coven.release-commit-gate-receipt/v1) so the release record
// can retain the required check names, conclusions, and run identity without
// re-deriving them later. The receipt contains no secret values.
//
// REST only: every GitHub interaction is a read against the REST API. The gate
// fails closed on any missing, ambiguous, or paginated-away evidence.

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import path from 'node:path';

const MANIFEST_SCHEMA = 'coven.release-required-checks/v1';
const RECEIPT_SCHEMA = 'coven.release-commit-gate-receipt/v1';
const JOB_PAGE_SIZE = 100;
const MAX_JOB_PAGES = 10;
const ACCEPTED_RUN_CONCLUSION = 'success';
const NON_COMPLETED_RUN_STATUSES = new Set([
  'queued',
  'in_progress',
  'requested',
  'waiting',
  'pending'
]);
const ROUTED_ACCEPTED_CONCLUSIONS = new Set(['success', 'skipped']);
const STABLE_TAG_REGEXP = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && moduleUrl === pathToFileURL(argv1).href;
}

function isSha(value) {
  return typeof value === 'string' && /^[0-9a-f]{40}$/i.test(value);
}

function sha256Hex(buffer) {
  return createHash('sha256').update(buffer).digest('hex');
}

function requireNonEmptyString(value, label) {
  if (typeof value !== 'string' || value.trim().length === 0) {
    throw new Error(`Refusing release commit gate: ${label} must be a non-empty string, got ${JSON.stringify(value ?? null)}.`);
  }
  return value;
}

function parseReleaseTag(releaseTag) {
  const normalized = String(releaseTag ?? '').trim();
  if (!STABLE_TAG_REGEXP.test(normalized)) {
    throw new Error(
      `Refusing release commit gate: release tag ${JSON.stringify(releaseTag)} must be a stable vX.Y.Z tag (incoherent version metadata).`
    );
  }
  return normalized;
}

// ---------------------------------------------------------------------------
// Required-checks manifest
// ---------------------------------------------------------------------------

export function loadRequiredChecksManifest(manifestText) {
  let parsed;
  try {
    parsed = JSON.parse(manifestText);
  } catch (error) {
    throw new Error(`Refusing release commit gate: required checks manifest is not valid JSON (${error.message}).`);
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('Refusing release commit gate: required checks manifest must be a JSON object.');
  }
  if (parsed.schema !== MANIFEST_SCHEMA) {
    throw new Error(
      `Refusing release commit gate: manifest schema must be ${MANIFEST_SCHEMA}, got ${JSON.stringify(parsed.schema ?? null)}.`
    );
  }
  const source = parsed.source_workflow;
  if (!source || typeof source !== 'object' || Array.isArray(source)) {
    throw new Error('Refusing release commit gate: manifest source_workflow must be an object.');
  }
  const sourceWorkflow = {
    name: requireNonEmptyString(source.name, 'source_workflow.name'),
    path: requireNonEmptyString(source.path, 'source_workflow.path'),
    event: requireNonEmptyString(source.event, 'source_workflow.event'),
    branch: requireNonEmptyString(source.branch, 'source_workflow.branch')
  };
  if (!/^\.github\/workflows\/[^/]+\.ya?ml$/.test(sourceWorkflow.path)) {
    throw new Error(
      `Refusing release commit gate: source_workflow.path must point inside .github/workflows/, got ${JSON.stringify(sourceWorkflow.path)}.`
    );
  }
  const strictChecks = parseCheckEntries(parsed.strict_checks, 'strict_checks', { allowEmpty: false });
  const routedChecks = parseCheckEntries(parsed.routed_checks, 'routed_checks', { allowEmpty: true });
  const strictNames = new Set(strictChecks.map((entry) => entry.name));
  for (const entry of routedChecks) {
    if (strictNames.has(entry.name)) {
      throw new Error(
        `Refusing release commit gate: check ${JSON.stringify(entry.name)} cannot be both strict and routed.`
      );
    }
  }
  return {
    schema: parsed.schema,
    source_workflow: sourceWorkflow,
    policy: typeof parsed.policy === 'object' && parsed.policy !== null ? parsed.policy : {},
    strict_checks: strictChecks,
    routed_checks: routedChecks
  };
}

function parseCheckEntries(value, label, { allowEmpty }) {
  if (!Array.isArray(value) || (!allowEmpty && value.length === 0)) {
    throw new Error(
      `Refusing release commit gate: manifest ${label} must be a non-empty array of check entries.`
    );
  }
  const seen = new Set();
  return value.map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new Error(`Refusing release commit gate: ${label}[${index}] must be an object.`);
    }
    const name = requireNonEmptyString(entry.name, `${label}[${index}].name`);
    const jobId = requireNonEmptyString(entry.job_id, `${label}[${index}].job_id`);
    if (seen.has(name)) {
      throw new Error(`Refusing release commit gate: duplicate check name ${JSON.stringify(name)} in ${label}.`);
    }
    seen.add(name);
    return { name, job_id: jobId };
  });
}

// ---------------------------------------------------------------------------
// Pure verification: refuse anything that is not exact, complete, green evidence
// ---------------------------------------------------------------------------

export function selectAcceptedWorkflowRun({ commitSha, manifest, workflowRuns }) {
  if (!isSha(commitSha)) {
    throw new Error(`Refusing release commit gate: commit SHA ${JSON.stringify(commitSha ?? null)} must be a 40-hex git SHA.`);
  }
  const { source_workflow: sourceWorkflow } = manifest;

  const candidateRuns = (Array.isArray(workflowRuns) ? workflowRuns : []).filter(
    (run) =>
      run?.path === sourceWorkflow.path &&
      run?.event === sourceWorkflow.event &&
      run?.head_branch === sourceWorkflow.branch
  );
  if (candidateRuns.length === 0) {
    throw new Error(
      `Refusing release commit gate: no ${JSON.stringify(sourceWorkflow.path)} run for event ${JSON.stringify(sourceWorkflow.event)} on branch ${JSON.stringify(sourceWorkflow.branch)} exists for the exact commit ${commitSha}. A release may not publish without a CI run on the exact source commit.`
    );
  }
  for (const run of candidateRuns) {
    if (run?.head_sha !== commitSha) {
      throw new Error(
        `Refusing release commit gate: candidate run ${describeRunId(run)} reports head SHA ${JSON.stringify(run?.head_sha ?? null)}, not the exact commit ${commitSha}. A green ancestor or unrelated commit is insufficient.`
      );
    }
  }
  const inFlightRuns = candidateRuns.filter((run) => run?.status !== 'completed');
  if (inFlightRuns.length > 0) {
    throw new Error(
      `Refusing release commit gate: CI run ${describeRunId(inFlightRuns[0])} for the exact commit ${commitSha} is stale or in flight (status=${JSON.stringify(inFlightRuns[0]?.status ?? null)}); wait for it to finish and retry.`
    );
  }
  const rejectedRun = candidateRuns.find((run) => run?.conclusion !== ACCEPTED_RUN_CONCLUSION);
  if (rejectedRun) {
    throw new Error(
      `Refusing release commit gate: CI run ${describeRunId(rejectedRun)} for the exact commit ${commitSha} concluded ${JSON.stringify(rejectedRun?.conclusion ?? null)}, not ${JSON.stringify(ACCEPTED_RUN_CONCLUSION)}.`
    );
  }
  return [...candidateRuns].sort((a, b) => compareRuns(a, b)).at(-1);
}

// The selected run's own job list is the only accepted evidence. Check runs are
// intentionally not consulted: the check-runs listing for a commit aggregates
// every check suite that ever reported against that SHA, so a same-named check
// run from another workflow (or another app) could satisfy a required check by
// name alone. Jobs are fetched per run attempt, carry the run id, head SHA, and
// workflow name, and are therefore bound to the exact evidence source.
export function verifyRunJobEvidence({ commitSha, run, manifest, runJobs }) {
  if (!isSha(commitSha)) {
    throw new Error(`Refusing release commit gate: commit SHA ${JSON.stringify(commitSha ?? null)} must be a 40-hex git SHA.`);
  }
  if (!run || typeof run !== 'object' || run.id === undefined || run.id === null) {
    throw new Error('Refusing release commit gate: an accepted workflow run with an id is required to bind job evidence.');
  }
  const { source_workflow: sourceWorkflow } = manifest;
  const evidence = new Map();
  for (const job of Array.isArray(runJobs) ? runJobs : []) {
    const name = typeof job?.name === 'string' ? job.name.trim() : '';
    if (!name) {
      throw new Error(
        `Refusing release commit gate: selected run ${describeRunId(run)} reported a job without a name; refusing to decide on unlabelled evidence.`
      );
    }
    if (String(job?.run_id ?? '') !== String(run.id)) {
      throw new Error(
        `Refusing release commit gate: job ${JSON.stringify(name)} belongs to run ${JSON.stringify(job?.run_id ?? null)}, not the selected run ${describeRunId(run)}. Check evidence from a different check suite (another workflow, attempt, or app) cannot satisfy a required check.`
      );
    }
    if (job?.head_sha !== commitSha) {
      throw new Error(
        `Refusing release commit gate: job ${JSON.stringify(name)} in the selected run reports head SHA ${JSON.stringify(job?.head_sha ?? null)}, not the exact commit ${commitSha}.`
      );
    }
    if (job?.workflow_name !== sourceWorkflow.name) {
      throw new Error(
        `Refusing release commit gate: job ${JSON.stringify(name)} belongs to workflow ${JSON.stringify(job?.workflow_name ?? null)}, not the source workflow ${JSON.stringify(sourceWorkflow.name)} of the selected run.`
      );
    }
    if (evidence.has(name)) {
      throw new Error(
        `Refusing release commit gate: ambiguous evidence — more than one job named ${JSON.stringify(name)} in the selected run ${describeRunId(run)} for the exact commit ${commitSha}.`
      );
    }
    evidence.set(name, { status: job.status, conclusion: job.conclusion });
  }

  // Fail closed on manifest narrowing: every job that ran in the selected CI
  // run must be a declared required check. A job missing from the manifest is
  // an unclassified required-check surface and refuses the release.
  const declaredNames = new Set(
    [...manifest.strict_checks, ...manifest.routed_checks].map((entry) => entry.name)
  );
  for (const name of evidence.keys()) {
    if (!declaredNames.has(name)) {
      throw new Error(
        `Refusing release commit gate: job ${JSON.stringify(name)} ran in the selected CI run for the exact commit ${commitSha} but is not declared in scripts/release-required-checks.json (strict, routed, or PR-only). The manifest must be updated in the same PR that adds, renames, or re-routes a CI job; refusing to decide on unclassified check surface.`
      );
    }
  }

  const checks = [];
  for (const entry of manifest.strict_checks) {
    const observed = evidence.get(entry.name);
    if (!observed) {
      throw new Error(
        `Refusing release commit gate: required check ${JSON.stringify(entry.name)} (strict) is missing for the exact commit ${commitSha}. Required-check names must stay stable; see scripts/release-required-checks.json.`
      );
    }
    if (observed.status !== 'completed') {
      throw new Error(
        `Refusing release commit gate: required check ${JSON.stringify(entry.name)} (strict) for the exact commit ${commitSha} is not completed (status=${JSON.stringify(observed.status ?? null)}).`
      );
    }
    if (observed.conclusion === 'skipped') {
      throw new Error(
        `Refusing release commit gate: required check ${JSON.stringify(entry.name)} (strict) was skipped for the exact commit ${commitSha}. A check that runs on every push to main may not be skipped for a release.`
      );
    }
    if (observed.conclusion !== ACCEPTED_RUN_CONCLUSION) {
      throw new Error(
        `Refusing release commit gate: required check ${JSON.stringify(entry.name)} (strict) for the exact commit ${commitSha} concluded ${JSON.stringify(observed.conclusion ?? null)}, not ${JSON.stringify(ACCEPTED_RUN_CONCLUSION)}.`
      );
    }
    checks.push({ name: entry.name, job_id: entry.job_id, class: 'strict', status: observed.status, conclusion: observed.conclusion });
  }

  for (const entry of manifest.routed_checks) {
    const observed = evidence.get(entry.name);
    if (!observed) {
      // Routed jobs are classified out by scripts/classify-ci-changes.py when
      // the commit does not touch their surface; the aggregate run conclusion
      // already covers any job that actually ran.
      checks.push({ name: entry.name, job_id: entry.job_id, class: 'routed', status: 'absent', conclusion: 'routed-out' });
      continue;
    }
    if (observed.status !== 'completed') {
      throw new Error(
        `Refusing release commit gate: routed check ${JSON.stringify(entry.name)} for the exact commit ${commitSha} is not completed (status=${JSON.stringify(observed.status ?? null)}).`
      );
    }
    if (!ROUTED_ACCEPTED_CONCLUSIONS.has(observed.conclusion)) {
      throw new Error(
        `Refusing release commit gate: routed check ${JSON.stringify(entry.name)} for the exact commit ${commitSha} concluded ${JSON.stringify(observed.conclusion ?? null)}; routed checks accept success or skipped only.`
      );
    }
    checks.push({ name: entry.name, job_id: entry.job_id, class: 'routed', status: observed.status, conclusion: observed.conclusion });
  }

  return {
    run: {
      id: String(run.id),
      run_number: run.run_number,
      run_attempt: run.run_attempt,
      head_sha: run.head_sha,
      conclusion: run.conclusion,
      url: typeof run.html_url === 'string' ? run.html_url : null
    },
    checks
  };
}

export function verifyCommitRequiredChecks({ commitSha, manifest, workflowRuns, runJobs }) {
  const run = selectAcceptedWorkflowRun({ commitSha, manifest, workflowRuns });
  return verifyRunJobEvidence({ commitSha, run, manifest, runJobs });
}

function compareRuns(a, b) {
  return compareNumbers(a?.run_number, b?.run_number) || compareNumbers(a?.run_attempt, b?.run_attempt) || compareNumbers(a?.id, b?.id);
}

function compareNumbers(a, b) {
  const left = Number(a ?? 0);
  const right = Number(b ?? 0);
  if (!Number.isSafeInteger(left) || !Number.isSafeInteger(right) || left === right) {
    return 0;
  }
  return left - right;
}

function describeRunId(run) {
  const id = run?.id === undefined || run?.id === null ? '(unknown)' : String(run.id);
  return id;
}

// ---------------------------------------------------------------------------
// Receipt
// ---------------------------------------------------------------------------

export function buildGateReceipt({
  repository,
  commitSha,
  manifestDigest,
  manifest,
  evidence,
  releaseTag = null,
  tagObjectSha = null,
  decision = 'accepted'
}) {
  requireNonEmptyString(repository, 'repository');
  if (!isSha(commitSha)) {
    throw new Error(`Refusing release commit gate: receipt commit SHA ${JSON.stringify(commitSha ?? null)} must be a 40-hex git SHA.`);
  }
  requireNonEmptyString(manifestDigest, 'manifest digest');
  if (releaseTag !== null) {
    parseReleaseTag(releaseTag);
  }
  if (tagObjectSha !== null && !isSha(tagObjectSha)) {
    throw new Error(`Refusing release commit gate: receipt tag object SHA ${JSON.stringify(tagObjectSha)} must be a 40-hex git SHA or null.`);
  }
  if (!evidence || typeof evidence !== 'object') {
    throw new Error('Refusing release commit gate: receipt requires accepted-commit evidence.');
  }
  return {
    schema: RECEIPT_SCHEMA,
    decision,
    repository,
    commit_sha: commitSha,
    release_tag: releaseTag,
    tag_object_sha: tagObjectSha,
    required_checks_manifest: {
      schema: manifest.schema,
      sha256: manifestDigest,
      strict_count: manifest.strict_checks.length,
      routed_count: manifest.routed_checks.length
    },
    source_workflow: { ...manifest.source_workflow },
    workflow_run: { ...evidence.run },
    checks: evidence.checks.map((check) => ({ ...check })),
    generated_from: 'scripts/verify-release-commit-gate.mjs'
  };
}

function renderReceipt(receipt) {
  return `${JSON.stringify(receipt, null, 2)}\n`;
}

// ---------------------------------------------------------------------------
// REST access
// ---------------------------------------------------------------------------

function ghApiJson(endpoint) {
  const result = spawnSync('gh', ['api', endpoint], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe']
  });
  if (result.error) {
    throw new Error(`gh api ${endpoint} failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`gh api ${endpoint} exited with ${result.status}: ${String(result.stderr ?? '').trim()}`);
  }
  return JSON.parse(result.stdout);
}

export async function verifyExactCommitGate({
  repository,
  commitSha,
  manifest,
  manifestDigest,
  releaseTag = null,
  tagObjectSha = null,
  ghApi = ghApiJson
}) {
  requireNonEmptyString(repository, 'repository');
  const runsEndpoint = `/repos/${repository}/actions/runs?head_sha=${encodeURIComponent(commitSha)}&per_page=${JOB_PAGE_SIZE}`;
  const runsPayload = await ghApi(runsEndpoint);
  const workflowRuns = Array.isArray(runsPayload?.workflow_runs) ? runsPayload.workflow_runs : [];
  if (Number.isSafeInteger(Number(runsPayload?.total_count)) && Number(runsPayload.total_count) > workflowRuns.length) {
    throw new Error(
      `Refusing release commit gate: ${runsPayload.total_count} workflow runs reference the exact commit ${commitSha} but only ${workflowRuns.length} were returned; refusing to decide on incomplete evidence.`
    );
  }

  const run = selectAcceptedWorkflowRun({ commitSha, manifest, workflowRuns });

  // Evidence is fetched for the selected run and attempt only, so no check run
  // from another check suite can enter the decision.
  const runJobs = [];
  let totalJobCount = null;
  let page = 1;
  while (true) {
    const jobsEndpoint =
      `/repos/${repository}/actions/runs/${encodeURIComponent(String(run.id))}` +
      `/attempts/${encodeURIComponent(String(run.run_attempt))}/jobs?per_page=${JOB_PAGE_SIZE}&page=${page}`;
    const payload = await ghApi(jobsEndpoint);
    if (totalJobCount === null) {
      totalJobCount = Number(payload?.total_count);
      if (!Number.isSafeInteger(totalJobCount) || totalJobCount < 0) {
        throw new Error(
          `Refusing release commit gate: job list payload for the selected run ${describeRunId(run)} has no usable total_count; refusing to decide on unverifiable evidence.`
        );
      }
    }
    const batch = Array.isArray(payload?.jobs) ? payload.jobs : [];
    runJobs.push(...batch);
    if (runJobs.length >= totalJobCount) {
      break;
    }
    page += 1;
    if (page > MAX_JOB_PAGES) {
      throw new Error(
        `Refusing release commit gate: jobs for the selected run ${describeRunId(run)} exceed ${MAX_JOB_PAGES * JOB_PAGE_SIZE} entries; refusing to decide on truncated evidence.`
      );
    }
  }

  const evidence = verifyRunJobEvidence({ commitSha, run, manifest, runJobs });
  const receipt = buildGateReceipt({
    repository,
    commitSha,
    manifestDigest,
    manifest,
    evidence,
    releaseTag,
    tagObjectSha
  });
  return { evidence, receipt };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

export function parseVerifyArgs(args) {
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
  const required = ['repository', 'commit-sha', 'manifest'];
  for (const key of required) {
    if (!options.get(key)) {
      throw new Error(`Missing required option --${key}.`);
    }
  }
  return options;
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command !== 'verify') {
    throw new Error('Usage: verify-release-commit-gate.mjs verify --repository OWNER/REPO --commit-sha <sha> --manifest <path> [--release-tag vX.Y.Z] [--tag-object-sha <sha>] [--receipt-output <path>]');
  }
  const options = parseVerifyArgs(args);
  const repository = options.get('repository');
  const commitSha = options.get('commit-sha');
  const manifestPath = options.get('manifest');
  const releaseTag = options.get('release-tag') ?? null;
  const tagObjectSha = options.get('tag-object-sha') ?? null;
  const receiptOutput = options.get('receipt-output') ?? null;

  let manifestText;
  try {
    manifestText = readFileSync(manifestPath, 'utf8');
  } catch (error) {
    throw new Error(`Refusing release commit gate: required checks manifest ${manifestPath} is unreadable (${error.message}).`);
  }
  const manifest = loadRequiredChecksManifest(manifestText);
  const { receipt } = await verifyExactCommitGate({
    repository,
    commitSha,
    manifest,
    manifestDigest: sha256Hex(Buffer.from(manifestText, 'utf8')),
    releaseTag,
    tagObjectSha
  });
  const receiptText = renderReceipt(receipt);
  if (receiptOutput) {
    const normalizedReceiptPath = path.resolve(receiptOutput);
    mkdirSync(path.dirname(normalizedReceiptPath), { recursive: true });
    writeFileSync(normalizedReceiptPath, receiptText, { flag: 'wx' });
  }
  process.stdout.write(
    [
      `release-commit-gate: accepted commit ${receipt.commit_sha}`,
      `workflow_run=${receipt.workflow_run.id} conclusion=${receipt.workflow_run.conclusion}`,
      `strict_checks=${receipt.required_checks_manifest.strict_count} routed_checks=${receipt.required_checks_manifest.routed_count}`,
      ''
    ].join('\n')
  );
}

if (isMainModule()) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}
