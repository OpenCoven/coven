#!/usr/bin/env node
// Build the structured certification receipt for the Coven end-to-end
// certification matrix (issue OpenCoven/coven#779).
//
//   node scripts/certification-receipt.mjs                     # print receipt
//   node scripts/certification-receipt.mjs --out receipt.json  # write to file
//   node scripts/certification-receipt.mjs --strict            # fail closed
//
// The receipt is keyed by exact source digests (commit + tree) and carries one
// entry per certification row with its outcome, evidence references, and — for
// non-terminal rows — the named owner issue and justification. It contains no
// prompts, credentials, or unrestricted terminal output, so it is safe to
// attach to a release record.
//
// Fail-closed behavior: `--strict` exits nonzero when any required row is
// proven failed or carries an explicit unknown disposition. That is the
// certification rule from the issue, executable: `unknown` is an explicit
// receipt disposition, never a terminal outcome for a required row.

import { execFileSync } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import process from 'node:process';

import {
  CERTIFICATION_MATRIX,
  RECEIPT_VERSION,
  SUPPORT_MATRIX_VERSION,
  certificationBlockers,
  matrixSummary,
  receiptLanes,
  validateMatrix
} from './certification-matrix.mjs';

// Deterministic by construction: no wall-clock timestamp. The receipt is keyed
// by the candidate digests, so the same candidate always yields the same bytes
// and an independent reviewer can regenerate and diff it. `reviewerDecision`
// stays null until release authorization (#805) sets it after human review —
// the receipt never self-certifies.
export function buildReceipt({
  matrix = CERTIFICATION_MATRIX,
  candidate = { sourceCommit: null, sourceTreeDigest: null, tag: null, channel: 'source-checkout' },
  platform = { os: process.platform, arch: process.arch },
  tooling = { node: process.versions.node }
} = {}) {
  const integrityErrors = validateMatrix(matrix);
  if (integrityErrors.length > 0) {
    throw new Error(
      `certification matrix is internally inconsistent:\n${formatIntegrityErrors(integrityErrors)}`
    );
  }
  return {
    receiptVersion: RECEIPT_VERSION,
    supportMatrixVersion: SUPPORT_MATRIX_VERSION,
    candidate,
    platform,
    tooling,
    lanes: receiptLanes(matrix),
    summary: matrixSummary(matrix),
    releaseBlockers: certificationBlockers(matrix),
    // Set by release authorization (#805) after human review; the receipt
    // never self-certifies.
    reviewerDecision: null
  };
}

// Resolve the candidate identity from the surrounding checkout. Every input
// can be pinned explicitly: artifact/tag receipts key off the published
// artifact, not whatever checkout happens to be present.
export function resolveCandidate({
  sourceCommit = null,
  sourceTreeDigest = null,
  tag = null,
  channel = 'source-checkout',
  execFile = (args) => execFileSync('git', args, { encoding: 'utf8' })
} = {}) {
  const resolve = (args) => {
    try {
      const value = execFile(args);
      return typeof value === 'string' ? value.trim() : null;
    } catch {
      return null;
    }
  };
  return {
    sourceCommit: sourceCommit ?? resolve(['rev-parse', 'HEAD']),
    sourceTreeDigest: sourceTreeDigest ?? resolve(['rev-parse', 'HEAD^{tree}']),
    tag,
    channel
  };
}

function formatBlockers(blockers) {
  return blockers
    .map((blocker) => `  - ${blocker.id} (${blocker.lane}): ${blocker.reason}`)
    .join('\n');
}

function formatIntegrityErrors(errors) {
  return errors.map((line) => `  - ${line}`).join('\n');
}

function writeReceipt(outPath, json, { force }) {
  if (existsSync(outPath) && !force) {
    // Mirrors the coven report publication contract: fail-if-exists, never a
    // silent overwrite of an evidence artifact.
    throw new Error(`refusing to overwrite ${outPath}: move it aside or pass --force`);
  }
  writeFileSync(outPath, json, 'utf8');
  process.stderr.write(`receipt written to ${outPath}\n`);
}

export function parseArgs(argv) {
  const options = {
    out: null,
    strict: false,
    force: false,
    channel: 'source-checkout',
    sourceCommit: null,
    sourceTreeDigest: null,
    tag: null
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--out') options.out = argv[(index += 1)];
    else if (arg === '--strict') options.strict = true;
    else if (arg === '--force') options.force = true;
    else if (arg === '--source-commit') options.sourceCommit = argv[(index += 1)];
    else if (arg === '--source-tree-digest') options.sourceTreeDigest = argv[(index += 1)];
    else if (arg === '--tag') options.tag = argv[(index += 1)];
    else if (arg === '--channel') options.channel = argv[(index += 1)];
    else return null;
  }
  return options;
}

function runCli(argv) {
  const options = parseArgs(argv);
  if (!options) {
    process.stderr.write(
      'usage: node scripts/certification-receipt.mjs [--out FILE] [--strict] [--force]\n' +
        '       [--channel NAME] [--source-commit SHA] [--source-tree-digest DIGEST] [--tag TAG]\n'
    );
    process.exitCode = 2;
    return;
  }

  const integrityErrors = validateMatrix();
  if (integrityErrors.length > 0) {
    process.stderr.write(
      `certification matrix is internally inconsistent:\n${formatIntegrityErrors(integrityErrors)}\n`
    );
    process.exitCode = 1;
    return;
  }

  const receipt = buildReceipt({ candidate: resolveCandidate(options) });
  const json = `${JSON.stringify(receipt, null, 2)}\n`;

  if (options.out) {
    writeReceipt(options.out, json, { force: options.force });
  } else {
    process.stdout.write(json);
  }

  const blockers = certificationBlockers();
  if (blockers.length > 0) {
    process.stderr.write(`certification is not complete: ${blockers.length} open blocker(s)\n`);
    for (const blocker of blockers) {
      process.stderr.write(`  - ${blocker.id} (${blocker.lane}): ${blocker.reason}\n`);
    }
    if (options.strict) {
      process.exitCode = 1;
    }
  }
}

const isMainModule = (argv1 = process.argv[1], moduleUrl = import.meta.url) =>
  Boolean(argv1) && moduleUrl === pathToFileURL(argv1).href;

if (isMainModule()) {
  try {
    runCli(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
