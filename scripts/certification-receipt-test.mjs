import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import {
  ALL_OUTCOMES,
  CERTIFICATION_MATRIX,
  EVIDENCE_KINDS,
  LANES,
  OUTCOMES,
  OUTCOME_LABELS,
  RELEASE_TAG_CHANNEL,
  SOURCE_CHECKOUT_CHANNEL,
  SUPPORT_INVENTORY,
  SUPPORT_MATRIX_VERSION,
  TERMINAL_OUTCOMES,
  certificationBlockers,
  goNoGo,
  matrixSummary,
  pendingRows,
  resolveApplicability,
  resolveCandidateContext,
  validateMatrix
} from './certification-matrix.mjs';
import { buildReceipt, parseArgs, resolveCandidate } from './certification-receipt.mjs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, '..');

const CI_WORKFLOW = readFileSync(path.join(repoRoot, '.github/workflows/ci.yml'), 'utf8');

function evidenceRepoPath(entry) {
  return entry.ref.split('#')[0];
}

function ciJobExists(jobName) {
  return CI_WORKFLOW.includes(`\n  ${jobName}:\n`);
}

test('certification matrix is internally consistent', () => {
  assert.deepEqual(validateMatrix(), []);
});

test('every declared lane carries at least one certification row', () => {
  for (const lane of LANES) {
    const rows = CERTIFICATION_MATRIX.filter((row) => row.lane === lane.id);
    assert.ok(rows.length > 0, `lane ${lane.id} (${lane.title}) has no rows`);
  }
});

test('every row only cites evidence kinds that resolve', () => {
  for (const row of CERTIFICATION_MATRIX) {
    assert.ok(row.evidence.length > 0, `row ${row.id} has no evidence`);
    for (const entry of row.evidence) {
      assert.ok(EVIDENCE_KINDS[entry.kind], `row ${row.id} cites unknown evidence kind ${entry.kind}`);
      if (entry.kind === 'issue') {
        assert.match(entry.ref, /^OpenCoven\/coven#\d+$/, `row ${row.id} cites a malformed issue ref`);
      } else if (entry.kind === 'ci-job') {
        const [workflow, jobName] = entry.ref.split('#');
        assert.equal(workflow, 'ci.yml', `row ${row.id} cites a ci-job outside ci.yml`);
        assert.ok(ciJobExists(jobName), `row ${row.id} cites ci job ${jobName} which is not defined in ci.yml`);
      } else {
        const repoPath = evidenceRepoPath(entry);
        assert.ok(
          existsSync(path.join(repoRoot, repoPath)),
          `row ${row.id} cites evidence ${repoPath} which does not exist in the repo`
        );
      }
    }
  }
});

test('non-terminal rows are accountable: owner issue and justification', () => {
  for (const row of CERTIFICATION_MATRIX) {
    if (!TERMINAL_OUTCOMES.has(row.outcome)) {
      assert.ok(row.ownerIssue, `row ${row.id} is non-terminal without an owner issue`);
      assert.ok(row.justification, `row ${row.id} is non-terminal without a justification`);
    }
    if (row.outcome === OUTCOMES.DEFERRED) {
      assert.ok(row.ownerIssue, `deferred row ${row.id} must name its owner issue`);
    }
  }
});

test('the certification rule is executable: unknown and failed required rows are blockers', () => {
  const unknownRows = CERTIFICATION_MATRIX.filter((row) => row.outcome === OUTCOMES.REQUIRED_UNKNOWN);
  const failedRows = CERTIFICATION_MATRIX.filter((row) => row.outcome === OUTCOMES.REQUIRED_FAILED);
  const blockers = certificationBlockers();
  assert.equal(
    blockers.length,
    unknownRows.length + failedRows.length,
    'blockers must be exactly the unknown/failed required rows'
  );
  for (const row of unknownRows) {
    assert.ok(
      blockers.some((blocker) => blocker.id === row.id && blocker.reason.includes('unknown disposition')),
      `row ${row.id} should be an explicit unknown blocker`
    );
  }
});

test('outcome vocabulary never includes skipped', () => {
  for (const outcome of ALL_OUTCOMES) {
    assert.ok(!/skip/i.test(outcome), `'${outcome}' must not be a skipped-shaped outcome`);
  }
  assert.ok(!OUTCOME_LABELS.deferred.match(/skip/i));
});

test('docs/reference/certification.md stays in lockstep with the matrix', () => {
  const docsPath = path.join(repoRoot, 'docs/reference/certification.md');
  assert.ok(existsSync(docsPath), 'docs/reference/certification.md must exist');
  const docsText = readFileSync(docsPath, 'utf8');

  const labelPattern = Object.values(OUTCOME_LABELS)
    .map((label) => label.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('|');
  const rowPattern = new RegExp(`^\\| ([A-K]\\d+) \\|.*\\| (${labelPattern}) \\|`, 'gm');

  const docRows = new Map();
  for (const match of docsText.matchAll(rowPattern)) {
    assert.ok(!docRows.has(match[1]), `docs table repeats row ${match[1]}`);
    docRows.set(match[1], match[2]);
  }

  const matrixRows = new Map(
    CERTIFICATION_MATRIX.map((row) => [row.id, OUTCOME_LABELS[row.outcome]])
  );
  assert.equal(
    docRows.size,
    matrixRows.size,
    `docs table has ${docRows.size} rows but the matrix has ${matrixRows.size}`
  );
  for (const [id, label] of matrixRows) {
    assert.ok(docRows.has(id), `docs table is missing row ${id}`);
    assert.equal(docRows.get(id), label, `docs table outcome for ${id} drifted from the matrix`);
  }

  assert.ok(
    docsText.includes(String(SUPPORT_MATRIX_VERSION)),
    'docs must state the support-matrix version'
  );
  assert.ok(
    docsText.includes('scripts/certification-receipt.mjs'),
    'docs must point at the receipt generator'
  );
});

test('ci.yml runs the certification receipt test suite in the policy guard', () => {
  assert.ok(
    CI_WORKFLOW.includes('node --test scripts/certification-receipt-test.mjs'),
    'policy-guard must run scripts/certification-receipt-test.mjs so the matrix cannot drift silently'
  );
});

test('receipt is deterministic, keyless, and structurally complete', () => {
  const pinnedCandidate = {
    sourceCommit: '0'.repeat(40),
    sourceTreeDigest: '1'.repeat(40),
    tag: null,
    channel: 'source-checkout'
  };
  const first = buildReceipt({ candidate: pinnedCandidate });
  const second = buildReceipt({ candidate: pinnedCandidate });
  assert.equal(JSON.stringify(first), JSON.stringify(second), 'receipt must be deterministic');

  assert.equal(first.receiptVersion, 1);
  assert.equal(typeof first.supportMatrixVersion, 'string');
  assert.equal(first.reviewerDecision, null, 'the receipt never self-certifies');

  const laneIds = first.lanes.map((lane) => lane.id);
  assert.deepEqual(laneIds, LANES.map((lane) => lane.id));

  const receiptRows = first.lanes.flatMap((lane) => lane.rows);
  assert.equal(receiptRows.length, CERTIFICATION_MATRIX.length);
  for (const row of receiptRows) {
    assert.ok(ALL_OUTCOMES.has(row.outcome), `receipt row ${row.id} has an out-of-vocabulary outcome`);
    assert.ok('evidence' in row && 'claim' in row);
  }

  assert.equal(first.summary.total, CERTIFICATION_MATRIX.length);
  assert.equal(
    first.summary.requiredPassed +
      first.summary.requiredFailed +
      first.summary.requiredUnknown +
      first.summary.notApplicable +
      first.summary.experimentalDisabled +
      first.summary.deferred,
    first.summary.total,
    'summary must partition every row exactly once'
  );

  const serialized = JSON.stringify(first);
  assert.doesNotMatch(serialized, /"token"|"password"|"secret"|"credential"/i);
  assert.match(first.candidate.sourceCommit, /^[0-9a-f]{40}$/);
  assert.match(first.candidate.sourceTreeDigest, /^[0-9a-f]{40}$/);
});

test('receipt fail-closed: unknown and failed required rows become blockers', () => {
  const base = {
    id: 'A1',
    lane: 'A',
    claim: 'synthetic row used to exercise blocker accounting',
    evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#807' }]
  };
  const syntheticUnknown = buildReceipt({
    matrix: [{ ...base, outcome: OUTCOMES.REQUIRED_UNKNOWN, ownerIssue: 807, justification: 'unknown' }],
    candidate: { sourceCommit: '0'.repeat(40), sourceTreeDigest: '1'.repeat(40), tag: null, channel: 'source-checkout' }
  });
  assert.equal(syntheticUnknown.releaseBlockers.length, 1);
  assert.equal(syntheticUnknown.summary.requiredUnknown, 1);

  const syntheticFailed = buildReceipt({
    matrix: [{ ...base, outcome: OUTCOMES.REQUIRED_FAILED, justification: 'proven broken' }],
    candidate: { sourceCommit: '0'.repeat(40), sourceTreeDigest: '1'.repeat(40), tag: null, channel: 'source-checkout' }
  });
  assert.equal(syntheticFailed.releaseBlockers.length, 1);
  assert.match(syntheticFailed.releaseBlockers[0].reason, /proven failed/);
});

test('receipt generation refuses an inconsistent matrix', () => {
  assert.throws(
    () =>
      buildReceipt({
        matrix: [
          {
            id: 'nope',
            lane: 'A',
            claim: 'this id is malformed on purpose',
            outcome: OUTCOMES.REQUIRED_PASSED,
            evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#807' }]
          }
        ]
      }),
    /internally inconsistent/
  );
});

test('resolveCandidate trims git output and survives a missing repo', () => {
  const trimmed = resolveCandidate({ execFile: () => 'abc123def\n' });
  assert.equal(trimmed.sourceCommit, 'abc123def');
  assert.equal(trimmed.sourceTreeDigest, 'abc123def');

  const missing = resolveCandidate({
    execFile: () => {
      throw new Error('not a repository');
    }
  });
  assert.equal(missing.sourceCommit, null);
  assert.equal(missing.sourceTreeDigest, null);

  const pinned = resolveCandidate({
    sourceCommit: 'f'.repeat(40),
    execFile: () => '0'.repeat(40)
  });
  assert.equal(pinned.sourceCommit, 'f'.repeat(40));
});

test('resolveCandidate reads the real checkout when unpinned', () => {
  const candidate = resolveCandidate();
  assert.match(candidate.sourceCommit ?? '', /^[0-9a-f]{40}$/);
  assert.match(candidate.sourceTreeDigest ?? '', /^[0-9a-f]{40}$/);
});

test('cli flags parse; unknown flags are rejected', () => {
  const parsed = parseArgs(['--out', 'receipt.json', '--strict', '--force', '--tag', 'v1.2.3', '--channel', 'npm']);
  assert.deepEqual(parsed, {
    out: 'receipt.json',
    strict: true,
    force: true,
    channel: 'npm',
    sourceCommit: null,
    sourceTreeDigest: null,
    tag: 'v1.2.3'
  });
  assert.equal(parseArgs(['--bogus']), null);
});

test('strict cli exits nonzero while the matrix carries open blockers', () => {
  const script = pathToFileURL(path.join(repoRoot, 'scripts/certification-receipt.mjs'));
  const result = spawnSync(process.execPath, [script.pathname, '--strict'], {
    encoding: 'utf8',
    cwd: repoRoot
  });
  const blockers = certificationBlockers();
  if (blockers.length > 0) {
    assert.equal(result.status, 1, 'strict mode must fail closed on open blockers');
    assert.match(result.stderr, /open blocker/);
  } else {
    assert.equal(result.status, 0);
  }
});

// ---------------------------------------------------------------------------
// Applicability, the support inventory, and the go/no-go verdict.

test('candidate contexts are validated against the support inventory', () => {
  assert.deepEqual(resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL }), {
    channel: SOURCE_CHECKOUT_CHANNEL,
    tag: null
  });
  const tagged = resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL, tag: 'v1.2.3' });
  assert.equal(tagged.channel, RELEASE_TAG_CHANNEL);
  assert.equal(tagged.tag, 'v1.2.3');

  assert.throws(() => resolveCandidateContext({ channel: 'vibes' }), /unknown certification channel/);
  assert.throws(
    () => resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL, tag: 'not-a-version' }),
    /release-tag candidates require a tag/
  );
  assert.throws(
    () => resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL }),
    /release-tag candidates require a tag/
  );
  assert.throws(
    () => resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL, tag: 'v1.2.3' }),
    /must not carry a release tag/
  );
});

test('release-only rows are derived applicable from the candidate channel/tag, not statically frozen', () => {
  const sourceContext = resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL });
  const releaseContext = resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL, tag: 'v9.9.9' });

  for (const rowId of ['J1', 'J3', 'J6']) {
    const row = CERTIFICATION_MATRIX.find((candidate) => candidate.id === rowId);
    assert.deepEqual(
      resolveApplicability(row, sourceContext),
      { applicable: false, basis: 'channel-not-in-applicableWhen' },
      `${rowId} must not apply to a source checkout`
    );
    assert.equal(
      resolveApplicability(row, releaseContext).applicable,
      true,
      `${rowId} must apply to a tagged release candidate`
    );
  }
});

test('deferred rows cannot vanish from the release go/no-go', () => {
  const sourceContext = resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL });
  const releaseContext = resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL, tag: 'v9.9.9' });

  const deferredIds = CERTIFICATION_MATRIX.filter((row) => row.outcome === OUTCOMES.DEFERRED).map(
    (row) => row.id
  );
  assert.ok(deferredIds.length > 0, 'the matrix must carry deferred rows for this test to mean anything');

  // Source channel: deferred rows are pending, owned, and not blockers.
  const sourceBlockers = certificationBlockers(CERTIFICATION_MATRIX, sourceContext);
  const sourcePending = pendingRows(CERTIFICATION_MATRIX, sourceContext);
  for (const id of deferredIds) {
    assert.ok(!sourceBlockers.some((blocker) => blocker.id === id), `${id} must not block a source checkout`);
    assert.ok(sourcePending.some((row) => row.id === id), `${id} must stay visible as pending`);
    assert.ok(sourcePending.find((row) => row.id === id).ownerIssue, `${id} must name its owner issue`);
  }

  // Release channel: every deferred row becomes an explicit blocker.
  const releaseBlockers = certificationBlockers(CERTIFICATION_MATRIX, releaseContext);
  for (const id of deferredIds) {
    const blocker = releaseBlockers.find((entry) => entry.id === id);
    assert.ok(blocker, `${id} must block a tagged release`);
    assert.match(blocker.reason, /deferred requirement/);
    assert.ok(blocker.ownerIssue, `${id} blocker must carry its owner issue`);
  }
  assert.equal(pendingRows(CERTIFICATION_MATRIX, releaseContext).length, 0);
});

test('statically not-applicable release rows become blockers once a tag is in flight', () => {
  const releaseContext = resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL, tag: 'v9.9.9' });
  const blockers = certificationBlockers(CERTIFICATION_MATRIX, releaseContext);
  for (const rowId of ['J1', 'J3', 'J6']) {
    const blocker = blockers.find((entry) => entry.id === rowId);
    assert.ok(blocker, `${rowId} must block a tagged release while it carries no evidence binding`);
    assert.match(blocker.reason, /became applicable/);
  }
  const sourceBlockers = certificationBlockers(CERTIFICATION_MATRIX);
  for (const rowId of ['J1', 'J3', 'J6']) {
    assert.ok(!sourceBlockers.some((entry) => entry.id === rowId));
  }
});

test('go/no-go verdict requires zero blockers, no pending release rows, and an approved reviewer decision', () => {
  const sourceContext = resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL });
  assert.equal(goNoGo(CERTIFICATION_MATRIX, sourceContext).verdict, 'no-go');
  assert.ok(goNoGo(CERTIFICATION_MATRIX, sourceContext).blockers.length > 0);

  // A synthetic fully-passed matrix still cannot self-certify a release: the
  // reviewer decision is missing.
  const cleanMatrix = [
    {
      id: 'A1',
      lane: 'A',
      claim: 'synthetic clean row used to exercise the go/no-go verdict',
      outcome: OUTCOMES.REQUIRED_PASSED,
      evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#805' }]
    }
  ];
  const releaseContext = resolveCandidateContext({ channel: RELEASE_TAG_CHANNEL, tag: 'v9.9.9' });
  assert.equal(
    goNoGo(cleanMatrix, releaseContext, { reviewerDecision: null }).verdict,
    'no-go',
    'a release without an approved reviewer decision must be no-go'
  );
  assert.equal(
    goNoGo(cleanMatrix, releaseContext, { reviewerDecision: 'approved' }).verdict,
    'go'
  );
  assert.equal(goNoGo(cleanMatrix, sourceContext, { reviewerDecision: null }).verdict, 'go');
});

test('support inventory and row platform coverage form a bijection', () => {
  const platformIds = SUPPORT_INVENTORY.platforms.map((platform) => platform.id);
  assert.equal(new Set(platformIds).size, platformIds.length, 'platform ids must be unique');
  assert.ok(SUPPORT_INVENTORY.platforms.length > 0);

  const covered = new Set();
  for (const row of CERTIFICATION_MATRIX) {
    if (row.platforms === undefined) {
      continue;
    }
    for (const platform of row.platforms) {
      assert.ok(
        platformIds.includes(platform),
        `row ${row.id} claims platform ${platform} which the support inventory does not declare`
      );
      covered.add(platform);
    }
  }
  for (const platform of platformIds) {
    assert.ok(
      covered.has(platform),
      `support inventory platform ${platform} is not covered by any certification row`
    );
  }
});

test('rows never claim release coverage a runner does not provide', () => {
  const platformById = new Map(SUPPORT_INVENTORY.platforms.map((platform) => [platform.id, platform]));
  for (const row of CERTIFICATION_MATRIX) {
    if (row.platforms === undefined) {
      continue;
    }
    const claimsReleaseCoverage = /at release tags?|release-time coverage|tag-built/i.test(row.claim);
    const claimedUnknown = row.outcome === OUTCOMES.REQUIRED_UNKNOWN;
    for (const platformId of row.platforms) {
      const platform = platformById.get(platformId);
      assert.ok(platform.releaseBuildRunner, `row ${row.id}: ${platformId} has no release build runner`);
      if (platform.releaseOnboardingRunner === null && claimsReleaseCoverage && !claimedUnknown) {
        assert.fail(
          `row ${row.id} claims release coverage for ${platformId} but the inventory has no release onboarding leg for it (${platform.releaseBuildRunner} builds, nobody verifies)`
        );
      }
      if (platform.releaseOnboardingRunner === null && claimedUnknown) {
        assert.ok(
          /releaseOnboardingRunner: null|dry-runs them on ubuntu-latest|dry-run/.test(row.justification),
          `row ${row.id} must explain that ${platformId} has no release onboarding leg`
        );
      }
    }
  }

  // The macOS rows are the review finding: they must be explicit unknowns
  // owned by release authorization until matching-runner release legs exist.
  for (const rowId of ['B3', 'B4']) {
    const row = CERTIFICATION_MATRIX.find((candidate) => candidate.id === rowId);
    assert.equal(row.outcome, OUTCOMES.REQUIRED_UNKNOWN, `${rowId} must stay an explicit unknown`);
    assert.equal(row.ownerIssue, 805, `${rowId} must be owned by release authorization`);
    assert.match(row.justification, /dry-runs them on ubuntu-latest|dry-runs the tag-built/);
  }
});
