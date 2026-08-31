#!/usr/bin/env node
// Coven automations conformance runner (coven.automations.conformance v1).
//
//   node conformance.mjs --profile all --target reference --report conformance/automations/reports/last-run.json
//
// Standalone and dependency-free: loads the versioned manifest, schemas, and
// vectors, validates every vector against the envelope schema, and executes
// them against a target. The default target is the plane's own reference
// oracle. A daemon endpoint or packaged release can be certified the same way
// once it advertises `coven.automations.conformance.v1`; until then those
// vectors are reported as skipped, never silently passed.
//
// Output is machine-readable (conformance.report.v1), carries exact source
// revisions and artifact digests, and is redacted before writing.

import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { readdir, readFile, writeFile, mkdir } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { validateAgainstSchema } from './lib/schema.mjs';
import { evaluateVector, fuzzInvariants } from './lib/evaluate.mjs';
import { redactPublishedText, REDACTION_RULES } from './lib/redact.mjs';

export const PLANE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const PLANE_VERSION = '1.0.0';
export const PROFILES = [
  'structural',
  'scheduler-reliability',
  'runtime-authority',
  'continuity',
  'privacy',
  'interoperability',
  'full'
];
const REQUIRED_PROFILES = [
  'structural',
  'scheduler-reliability',
  'runtime-authority',
  'continuity',
  'privacy',
  'interoperability'
];

async function walkJsonFiles(root) {
  const files = [];
  let entries;
  try {
    entries = await readdir(root, { withFileTypes: true });
  } catch {
    return files;
  }
  for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const full = join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await walkJsonFiles(full)));
    } else if (entry.name.endsWith('.json')) {
      files.push(full);
    }
  }
  return files;
}

// Loads and validates every vector file. Throws with the file name when a
// vector fails the envelope schema, so a malformed vector can never count as
// a passing certification.
export async function loadVectors(planeRoot = PLANE_ROOT) {
  const envelopeSchema = JSON.parse(
    await readFile(join(planeRoot, 'schemas', 'conformance.vector.v1.schema.json'), 'utf8')
  );
  const definitionSchema = JSON.parse(
    await readFile(
      join(planeRoot, 'schemas', 'coven.automations.definition.v1.schema.json'),
      'utf8'
    )
  );
  const vectors = [];
  for (const root of ['vectors', 'scenarios']) {
    for (const file of await walkJsonFiles(join(planeRoot, root))) {
      const document = JSON.parse(await readFile(file, 'utf8'));
      const errors = validateAgainstSchema(document, envelopeSchema);
      if (errors.length > 0) {
        throw new Error(
          `vector ${relative(planeRoot, file)} failed the conformance.vector.v1 envelope:\n  ${errors.join('\n  ')}`
        );
      }
      vectors.push({ file: relative(planeRoot, file), vector: document });
    }
  }
  return { vectors, definitionSchema };
}

// Runs one vector on a target. The reference oracle executes everything;
// daemon and packaged targets must advertise the conformance capability.
// A vector whose prerequisites the target does not meet is not-applicable —
// distinct from passed and from failed: it never counts toward a profile
// result and, for vectors marked `execution: "required"`, forces the
// profile into `incomplete`. A target that advertises the capability but
// cannot evaluate a selected vector is a hard failure, never a skip.
export async function runOnTarget(vector, target) {
  const capabilities = target.capabilities ?? [];
  const unmet = (vector.prerequisites ?? []).filter(
    (prerequisite) => !capabilities.includes(prerequisite)
  );
  if (unmet.length > 0) {
    return {
      status: 'not-applicable',
      failures: [],
      reason: `prerequisites not met on this target: ${unmet.join(', ')}`
    };
  }
  if (target.kind === 'reference-oracle') {
    const { failures } = evaluateVector(vector, { definitionSchema: target.definitionSchema });
    return { status: failures.length === 0 ? 'passed' : 'failed', failures };
  }
  const capability = await target.probe?.();
  if (!capability) {
    return {
      status: 'not-applicable',
      failures: [],
      reason: 'target does not advertise coven.automations.conformance.v1'
    };
  }
  const result = await target.evaluate?.(vector);
  if (!result) {
    return {
      status: 'failed',
      failures: [
        {
          vectorId: vector.vectorId,
          profile: vector.profile,
          invariant: 'target-evaluator',
          objectIds: [],
          eventCursor: null,
          expected: 'the target evaluates every selected vector it advertises',
          observed: 'target advertised coven.automations.conformance.v1 but has no evaluator for this vector',
          reproduction: `node conformance/automations/runner/conformance.mjs --profile ${vector.profile} --vector ${vector.vectorId} --target ${target.kind}`
        }
      ],
      reason: 'target advertised the capability but has no evaluator for this vector'
    };
  }
  return result;
}

function sha256File(buffer) {
  return `sha256-${createHash('sha256').update(buffer).digest('hex')}`;
}

function gitCommit() {
  const result = spawnSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' });
  if (result.status !== 0) return null;
  return result.stdout.trim() || null;
}

// Scrubs anything about to be published: sensitive structured values are
// replaced before serialization and the serialized text is scrubbed again
// (delegates to the shared redaction module).
export function redactReportValue(value, prompts) {
  return redactPublishedText(value, prompts);
}

// Profile result with fail-closed statuses:
//   passed          every selected vector executed, none failed
//   failed          at least one failure
//   incomplete      executed but a REQUIRED vector did not (unmet
//                   prerequisites on this target) — never certifiable
//   not-applicable  nothing executed (nothing selected, or only
//                   target-dependent vectors whose prerequisites are absent)
export function profileResult(entries) {
  const passed = entries.filter((entry) => entry.status === 'passed').length;
  const failed = entries.filter((entry) => entry.status === 'failed').length;
  const notApplicableEntries = entries.filter((entry) => entry.status === 'not-applicable');
  const requiredGaps = notApplicableEntries.filter(
    (entry) => entry.vector.execution !== 'target-dependent'
  ).length;
  let status;
  if (failed > 0) status = 'failed';
  else if (passed === 0) status = 'not-applicable';
  else if (requiredGaps > 0) status = 'incomplete';
  else status = 'passed';
  const artifacts = [
    ...new Set(
      entries
        .filter((entry) => entry.status === 'passed')
        .flatMap((entry) => entry.vector.artifacts ?? [])
    )
  ].sort();
  return {
    status,
    passed,
    failed,
    notApplicable: notApplicableEntries.length,
    artifacts,
    vectorIds: entries.map((entry) => entry.vector.vectorId)
  };
}

export function parseArgs(argv) {
  const options = {
    profile: 'all',
    target: 'reference-oracle',
    report: join(PLANE_ROOT, 'reports', 'last-run.json'),
    vector: null,
    slo: null,
    fuzz: 0,
    seed: 858,
    list: false,
    quiet: false
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const take = () => {
      if (arg.includes('=')) return arg.slice(arg.indexOf('=') + 1);
      index += 1;
      return argv[index];
    };
    if (arg === '--list') options.list = true;
    else if (arg === '--quiet') options.quiet = true;
    else if (arg.startsWith('--profile')) options.profile = take();
    else if (arg.startsWith('--target')) options.target = take();
    else if (arg.startsWith('--report')) options.report = take();
    else if (arg.startsWith('--vector')) options.vector = take();
    else if (arg.startsWith('--slo')) options.slo = take();
    else if (arg.startsWith('--fuzz')) options.fuzz = Number(take());
    else if (arg.startsWith('--seed')) options.seed = Number(take());
    else throw new Error(`unknown argument: ${arg}`);
  }
  if (!PROFILES.includes(options.profile) && options.profile !== 'all') {
    throw new Error(`unknown profile: ${options.profile}`);
  }
  return options;
}

// Validates a report against conformance.report.v1.schema.json. Returns the
// error list; a non-empty list means the report must not be published.
export function validateReport(report, planeRoot = PLANE_ROOT) {
  return validateAgainstSchema(report, loadReportSchema(planeRoot));
}

function loadReportSchema(planeRoot = PLANE_ROOT) {
  return JSON.parse(
    readFileSync(join(planeRoot, 'schemas', 'conformance.report.v1.schema.json'), 'utf8')
  );
}

export async function runConformance(options, planeRoot = PLANE_ROOT) {
  const { vectors, definitionSchema } = await loadVectors(planeRoot);
  const selected = options.vector
    ? vectors.filter((entry) => entry.vector.vectorId === options.vector)
    : vectors.filter(
        (entry) =>
          options.profile === 'all' ||
          entry.vector.profile === options.profile ||
          options.profile === 'full'
      );
  if (selected.length === 0) {
    throw new Error(`no vectors selected (profile=${options.profile}, vector=${options.vector})`);
  }

  const target = {
    kind: options.target,
    definitionSchema,
    capabilities: options.target === 'reference-oracle' ? ['reference-oracle'] : []
  };
  const results = [];
  for (const entry of selected) {
    const outcome = await runOnTarget(entry.vector, target);
    results.push({
      vector: entry.vector,
      file: entry.file,
      status: outcome.status,
      failures: outcome.failures ?? [],
      reason: outcome.reason
    });
  }

  const failures = results.flatMap((entry) =>
    entry.failures.map((item) => ({ ...item, vectorId: item.vectorId ?? entry.vector.vectorId }))
  );

  // Randomized property testing folds into the scheduler profile.
  let fuzz = null;
  if (options.fuzz > 0) {
    fuzz = fuzzInvariants({ operations: options.fuzz, seed: options.seed });
    for (const violation of fuzz.violations) {
      failures.push({
        vectorId: `fuzz-seed-${options.seed}`,
        profile: 'scheduler-reliability',
        invariant: violation.invariant ?? 'randomized-invariant',
        objectIds: [],
        eventCursor: null,
        expected: 'invariant holds under randomized operations',
        observed: `step ${violation.step} (${violation.op}): ${violation.observed}`,
        reproduction: `node conformance/automations/runner/conformance.mjs --fuzz ${options.fuzz} --seed ${options.seed}`
      });
    }
  }

  // A single-vector or single-profile run asserts exactly what it names;
  // only a full-plane run (all/full) certifies the full profile.
  const scopedRun = options.vector || (options.profile !== 'all' && options.profile !== 'full');

  const profiles = {};
  for (const profile of PROFILES) {
    // The full profile is the immutable v1 compatibility set: it only means
    // something on a full-plane run. A scoped run reports it not-applicable
    // instead of silently counting its subset as `full` passed.
    const entries =
      profile === 'full'
        ? scopedRun
          ? []
          : results
        : results.filter((entry) => entry.vector.profile === profile);
    profiles[profile] = profileResult(entries);
  }

  // Collect every definition prompt — valid and invalid fixtures alike — so
  // report text can be scrubbed. Prompts of any length are collected; the
  // redaction module handles short ones.
  const prompts = new Set();
  const collectPrompts = (document) => {
    if (typeof document?.prompt === 'string' && document.prompt.trim() !== '') {
      prompts.add(document.prompt);
    }
  };
  for (const entry of vectors) {
    for (const document of entry.vector.input?.definitions ?? []) collectPrompts(document);
    for (const document of entry.vector.input?.invalidDefinitions ?? []) collectPrompts(document);
  }

  const artifactDigests = {};
  for (const file of await walkJsonFiles(planeRoot)) {
    artifactDigests[relative(planeRoot, file)] = sha256File(await readFile(file));
  }
  for (const file of ['manifest.json']) {
    try {
      artifactDigests[file] = sha256File(await readFile(join(planeRoot, file)));
    } catch {
      // manifest is optional for report assembly
    }
  }

  const sloResult = { profile: null, status: 'not-run' };
  if (options.slo) {
    const gate = await evaluateSloGate(options.slo, planeRoot);
    sloResult.profile = gate.profile;
    sloResult.status = gate.status;
  }

  // A single-vector or single-profile run asserts exactly what it names;
  // only a full-plane run (all/full) must cover every required profile.
  // Fail-closed certification gate (finding 2 of the review):
  //   - a single-vector or single-profile run asserts exactly what it names,
  //     so every selected vector must have executed: any not-applicable
  //     outcome fails the gate;
  //   - only a full-plane run (all/full) certifies the full profile, and it
  //     requires every required profile to be `passed` — not incomplete, not
  //     not-applicable — plus zero failures. Target-dependent canaries that
  //     this target cannot run are reported separately and never upgrade a
  //     profile to passed-by-skip.
  const profilesFullStatus = scopedRun
    ? profileResult([]).status
    : profiles.full.status;
  const missingProfiles = scopedRun
    ? []
    : REQUIRED_PROFILES.filter((profile) => profiles[profile].status !== 'passed');
  const selectedNotApplicable = results.filter((entry) => entry.status === 'not-applicable');
  const gateStatus =
    failures.length === 0 &&
    missingProfiles.length === 0 &&
    (!scopedRun || selectedNotApplicable.length === 0) &&
    (sloResult.status === 'not-run' || sloResult.status === 'passed')
      ? 'passed'
      : 'failed';

  const gateNotes =
    missingProfiles.length > 0
      ? `profiles not fully certified on this target: ${missingProfiles.join(', ')}`
      : scopedRun && selectedNotApplicable.length > 0
        ? `selected vectors that did not execute: ${selectedNotApplicable.map((entry) => entry.vector.vectorId).join(', ')}`
        : fuzz
          ? `randomized property testing: ${fuzz.steps} steps, seed ${options.seed}`
          : sloResult.status === 'not-run'
            ? 'SLO evidence not provided: vector conformance only, no SLO certification'
            : null;
  const gate = {
    status: gateStatus,
    requiredProfiles: REQUIRED_PROFILES,
    fullProfileStatus: profilesFullStatus
  };
  if (gateNotes !== null) gate.notes = gateNotes;

  const report = {
    reportVersion: 1,
    plane: 'coven.automations.conformance',
    planeVersion: PLANE_VERSION,
    generatedAt: new Date().toISOString(),
    target: {
      kind: options.target,
      name: options.target === 'reference-oracle' ? 'reference-oracle' : options.target,
      version: PLANE_VERSION,
      revisions: { sourceCommit: gitCommit(), runnerVersion: PLANE_VERSION },
      endpoint: null
    },
    environment: {
      node: process.version,
      os: process.platform,
      arch: process.arch,
      hostTimezone: Intl.DateTimeFormat().resolvedOptions().timeZone ?? null
    },
    artifactDigests,
    profiles,
    failures,
    notApplicable: selectedNotApplicable.map((entry) => ({
      vectorId: entry.vector.vectorId,
      required: entry.vector.execution !== 'target-dependent',
      reason: entry.reason ?? 'not applicable on this target'
    })),
    slo: sloResult,
    redaction: {
      applied: true,
      rules: REDACTION_RULES
    },
    gate
  };

  // The report must satisfy conformance.report.v1 before it is published:
  // a report that violates its own schema can never back a certification,
  // so validation failures abort the run (fail closed). The published form
  // is the redacted text, so that exact artifact is what gets validated.
  const publishErrors = validateReport(report, planeRoot);
  if (options.report) {
    const redacted = redactReportValue(report, [...prompts]) + '\n';
    publishErrors.push(...validateReport(JSON.parse(redacted), planeRoot));
    if (publishErrors.length > 0) {
      throw new Error(
        `conformance report violates conformance.report.v1; refusing to publish:\n  ${publishErrors.join('\n  ')}`
      );
    }
    await mkdir(dirname(options.report), { recursive: true });
    await writeFile(options.report, redacted);
  } else if (publishErrors.length > 0) {
    throw new Error(
      `conformance report violates conformance.report.v1; refusing to certify:\n  ${publishErrors.join('\n  ')}`
    );
  }

  return { report, prompts, selectedCount: selected.length };
}

// SLO gate: validates a measured report against slo/slo.v1.json. Gates that
// need the real binary stay 'provisional' until measured; the hard invariants
// (no duplicate dispatch, no silent loss, no false success) come from the
// conformance run itself.
export async function evaluateSloGate(measuredPath, planeRoot = PLANE_ROOT) {
  const slo = JSON.parse(await readFile(join(planeRoot, 'slo', 'slo.v1.json'), 'utf8'));
  const measured = JSON.parse(await readFile(measuredPath, 'utf8'));
  const values = new Map(
    (measured.measures ?? []).map((measure) => [measure.id, measure.value])
  );
  let failed = false;
  let measuredCount = 0;
  for (const measure of slo.measures ?? []) {
    const value = values.get(measure.id);
    if (value === undefined) continue;
    measuredCount += 1;
    if (measure.direction === 'lower-is-better' && value > measure.gate) failed = true;
    if (measure.direction === 'higher-is-better' && value < measure.gate) failed = true;
  }
  return {
    profile: slo.profile ?? 'coven.automations.slo.v1',
    status: measuredCount === 0 ? 'provisional' : failed ? 'failed' : 'passed'
  };
}

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.list) {
    const { vectors } = await loadVectors();
    for (const entry of vectors) {
      console.log(`${entry.vector.profile}\t${entry.vector.category}\t${entry.vector.vectorId}`);
    }
    return 0;
  }
  const { report, selectedCount } = await runConformance(options);
  if (!options.quiet) {
    const summary = {
      plane: report.plane,
      planeVersion: report.planeVersion,
      target: report.target.kind,
      vectors: selectedCount,
      profiles: Object.fromEntries(
        Object.entries(report.profiles).map(([name, result]) => [
          name,
          `${result.passed} passed / ${result.failed} failed / ${result.notApplicable} not-applicable (${result.status})`
        ])
      ),
      failures: report.failures.length,
      slo: report.slo.status,
      gate: report.gate.status,
      report: options.report
    };
    console.log(JSON.stringify(summary, null, 2));
  }
  return report.gate.status === 'passed' ? 0 : 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main()
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error) => {
      console.error(`conformance runner failed: ${error.message}`);
      process.exitCode = 1;
    });
}
