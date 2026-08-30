// Operator diagnostics evaluator for the conformance reference oracle.
//
// Derives the coven automations doctor report (diagnostics.doctor.v1) from an
// oracle state: one stable finding code per supported unhealthy state, each
// with subject ids, a redacted observation, and exact safe next steps. No
// diagnostic ever recommends deleting rows or blindly rerunning ambiguous
// mutating work.

import { latestDueSlot } from './clock.mjs';

// Safe next steps per finding code. Mutating commands always carry an
// expected-state guard or a --dry-run review first.
const SAFE_NEXT_STEPS = {
  'scheduler.no-leader': [
    'start the daemon or restore scheduler leadership; do not mutate the store by hand',
    're-run: coven automations doctor --json after one cadence (60s)'
  ],
  'scheduler.stale-pass': [
    'check daemon liveness: coven daemon status --json',
    'the next healthy pass fences the latest due slot (misfire latest); no manual backfill is safe'
  ],
  'scheduler.planning-lag': [
    'verify scheduler leadership; the next pass fences the latest due slot',
    'run: coven automations status --json'
  ],
  'occurrences.due-backlog': [
    'ensure the daemon is running; eligible work drains on the next passes',
    'inspect: coven automations status --json'
  ],
  'occurrences.stale-lease': [
    'the next scheduler pass expires the lease automatically; review with: coven automations occurrence <occurrence-id>',
    'if urgent: coven automations reconcile <occurrence-id> --dry-run'
  ],
  'occurrences.recovery-required': [
    'review with: coven automations reconcile <run-id> --dry-run',
    'retry only with a guard: coven automations retry <run-id> --expected-state <state>'
  ],
  'occurrences.missed-occurrence': [
    'inspect: coven automations occurrence <occurrence-id>',
    'misfire policy latest replans only the latest missed slot on the next pass'
  ],
  'runs.repeated-failures': [
    'read the bounded failure reasons: coven automations explain <automation-id>',
    'fix the cause before retrying; retries re-dispatch real work'
  ],
  'runs.quarantined': [
    'inspect: coven automations explain <automation-id>',
    'after fixing the cause: coven automations retry <run-id> --expected-state quarantined'
  ],
  'runs.ambiguous-outcome': [
    'do NOT rerun blindly; the runtime may have applied side effects',
    'review first: coven automations reconcile <run-id> --dry-run'
  ],
  'runs.cancelled': [
    'read the cancellation reason: coven automations run <run-id>',
    'a cancelled run is terminal; schedule new work instead of rerunning it'
  ],
  'authority.unresolved-refusal': [
    'fix the authority condition (identity, capability, or approval), then retry with --expected-state',
    'dispatch stays fail-closed until the binding verifies at dispatch time'
  ],
  'authority.stale-identity': [
    're-pair the familiar or restore its revision, then retry with --expected-state',
    'do not loosen capability requirements to force a dispatch'
  ],
  'delivery.commit-failures': [
    'inspect the output target permissions; the runtime outcome is preserved on the receipt',
    'delivery retries only through an explicit guarded retry'
  ],
  'receipts.unverified': [
    'do not trust the receipt; re-verify: coven automations run <run-id> --verify-receipt'
  ],
  'events.cursor-lag': [
    'resume the subscriber from the reported cursor; replay is idempotent'
  ],
  'disk.retention-pressure': [
    'review growth: coven automations status --json',
    'retention applies automatically on the next pass; no manual deletion is safe here'
  ]
};

function ageMsSafe(nowMs, thenMs) {
  return Math.max(0, nowMs - thenMs);
}

function doctorReport(findings, model) {
  const errorFindings = findings.filter((finding) => finding.severity === 'error');
  const safeNextSteps = [];
  for (const finding of findings) {
    for (const step of finding.safeNextSteps) {
      if (!safeNextSteps.includes(step)) safeNextSteps.push(step);
    }
  }
  const counts = {
    due: 0,
    eligible: 0,
    claimed: 0,
    running: 0,
    recoveryRequired: 0,
    quarantined: 0
  };
  for (const occurrence of model.occurrences.values()) {
    if (occurrence.state === 'planned') {
      counts.due += occurrence.scheduledForMs <= model.now ? 1 : 0;
      counts.eligible += occurrence.scheduledForMs <= model.now ? 1 : 0;
    }
    if (occurrence.state === 'claimed') counts.claimed += 1;
    if (occurrence.state === 'running') counts.running += 1;
    if (occurrence.state === 'quarantined') counts.quarantined += 1;
  }
  for (const run of model.runs.values()) {
    if (run.status === 'running') {
      const occurrence = run.occurrenceId ? model.occurrences.get(run.occurrenceId) : null;
      if (occurrence && ['succeeded', 'failed', 'quarantined'].includes(occurrence.state)) {
        counts.recoveryRequired += 1;
      }
    }
  }
  return {
    doctorVersion: 1,
    generatedAt: new Date(model.now).toISOString(),
    ok: errorFindings.length === 0,
    leadership: {
      isLeader: model.leader.isLeader,
      leaderId: model.leader.isLeader ? 'daemon' : null,
      lastSuccessfulPassAt:
        model.leader.lastSuccessfulPassAtMs === null
          ? null
          : new Date(model.leader.lastSuccessfulPassAtMs).toISOString(),
      lastPassDurationMs: model.leader.lastPassDurationMs
    },
    store: {
      schemaVersion: 1,
      integrity: 'ok',
      migrationState: 'current'
    },
    counts,
    findings,
    safeNextSteps,
    surface: {
      doctor: 'coven automations doctor',
      status: 'coven automations status',
      explain: 'coven automations explain <automation-id>',
      occurrence: 'coven automations occurrence <occurrence-id>',
      run: 'coven automations run <run-id>',
      attempts: 'coven automations attempts <run-id>',
      leases: 'coven automations leases',
      events: 'coven automations events --after <cursor>',
      schedule: 'coven automations schedule --from <iso> --to <iso> --dry-run',
      reconcile: 'coven automations reconcile <run-id> --dry-run',
      retry: 'coven automations retry <run-id> --expected-state <state>',
      cancel: 'coven automations cancel <run-id> --reason <text>'
    }
  };
}

export function doctorFindings(model) {
  const findings = [];
  const push = (code, severity, subjectIds, observed) => {
    findings.push({
      code,
      severity,
      subjectIds,
      observed,
      safeNextSteps: SAFE_NEXT_STEPS[code] ?? []
    });
  };

  if (!model.leader.isLeader) {
    push('scheduler.no-leader', 'error', [], 'no scheduler leader is recorded');
  } else if (
    model.leader.lastSuccessfulPassAtMs !== null &&
    ageMsSafe(model.now, model.leader.lastSuccessfulPassAtMs) > 2 * 60e3
  ) {
    const seconds = Math.round(ageMsSafe(model.now, model.leader.lastSuccessfulPassAtMs) / 1000);
    push('scheduler.stale-pass', 'warning', [], `last successful scheduler pass was ${seconds}s ago`);
  }

  // Planning lag: an active routine whose latest due slot is not fenced.
  for (const record of model.definitions.values()) {
    if (record.definition.status !== 'ACTIVE') continue;
    const latest = latestDueSlot(
      record.definition.rrule,
      record.definition.timezone,
      record.createdAtMs,
      model.now,
      model.hostTimezone
    );
    if (latest === null) continue;
    const fenced = [...model.occurrences.values()].some(
      (occurrence) =>
        occurrence.automationId === record.definition.id &&
        occurrence.scheduledForMs === latest
    );
    if (fenced) continue;
    const hadAChance = model.passes.some(
      (pass) =>
        pass.at >= Math.max(latest, record.createdAtMs) &&
        pass.activeIds.includes(record.definition.id)
    );
    if (!hadAChance) continue;
    push(
      'scheduler.planning-lag',
      'warning',
      [record.definition.id],
      'latest due slot is not fenced yet; the next successful pass fences it (misfire latest)'
    );
  }

  // Due backlog: planned occurrences overdue by more than two cadences.
  const backlog = [...model.occurrences.values()].filter(
    (occurrence) => occurrence.state === 'planned' && model.now - occurrence.scheduledForMs > 2 * 60e3
  );
  if (backlog.length > 0) {
    push(
      'occurrences.due-backlog',
      'warning',
      backlog.map((occurrence) => occurrence.id),
      `${backlog.length} planned occurrence(s) past due with no claim`
    );
  }

  // Stale leases: still held but already expired.
  const staleLeases = [...model.occurrences.values()].filter(
    (occurrence) =>
      ['claimed', 'running'].includes(occurrence.state) &&
      occurrence.leaseExpiresAtMs !== null &&
      occurrence.leaseExpiresAtMs < model.now
  );
  if (staleLeases.length > 0) {
    push(
      'occurrences.stale-lease',
      'warning',
      staleLeases.map((occurrence) => occurrence.id),
      `${staleLeases.length} occurrence(s) hold an expired lease`
    );
  }

  // Settlement interrupted: a running run whose occurrence already settled.
  for (const run of model.runs.values()) {
    if (run.status !== 'running') continue;
    const occurrence = run.occurrenceId ? model.occurrences.get(run.occurrenceId) : null;
    if (occurrence && ['succeeded', 'failed', 'quarantined'].includes(occurrence.state)) {
      push(
        'occurrences.recovery-required',
        'warning',
        [run.id, occurrence.id],
        `run is ${run.status} while its occurrence is ${occurrence.state}; recovery pending`
      );
    }
  }

  // Missed occurrences: leases that expired and recovered to failed.
  const missed = [...model.occurrences.values()].filter(
    (occurrence) => occurrence.failureReason === 'lease expired'
  );
  if (missed.length > 0) {
    push(
      'occurrences.missed-occurrence',
      'warning',
      missed.map((occurrence) => occurrence.id),
      `${missed.length} occurrence(s) missed (expired lease) and recovered to failed`
    );
  }

  // Repeated failures per automation (consecutive, latest-first).
  const runsByAutomation = new Map();
  const sortedRuns = [...model.runs.values()].sort((a, b) => a.startedAtMs - b.startedAtMs);
  for (const run of sortedRuns) {
    const list = runsByAutomation.get(run.automationId) ?? [];
    list.push(run);
    runsByAutomation.set(run.automationId, list);
  }
  for (const [automationId, runs] of runsByAutomation) {
    let consecutive = 0;
    for (const run of runs) {
      if (run.status === 'failed') {
        consecutive += 1;
      } else if (run.status === 'succeeded') {
        consecutive = 0;
      }
    }
    if (consecutive >= 3) {
      push('runs.repeated-failures', 'warning', [automationId], `${consecutive} consecutive failed runs`);
    }
  }

  const quarantined = [...model.occurrences.values()].filter(
    (occurrence) => occurrence.state === 'quarantined'
  );
  if (quarantined.length > 0) {
    push(
      'runs.quarantined',
      'warning',
      quarantined.map((occurrence) => occurrence.id),
      `${quarantined.length} occurrence(s) quarantined after exhausting retries`
    );
  }

  for (const run of model.runs.values()) {
    if (run.ambiguous) {
      push(
        'runs.ambiguous-outcome',
        'error',
        [run.id],
        'runtime lost after a possible side effect; outcome unknown'
      );
    }
  }
  const cancelled = [...model.runs.values()].filter((run) => run.status === 'cancelled');
  if (cancelled.length > 0) {
    push('runs.cancelled', 'info', cancelled.map((run) => run.id), `${cancelled.length} cancelled run(s)`);
  }

  // Authority findings.
  const authorityRefusals = [...model.occurrences.values()].filter(
    (occurrence) =>
      typeof occurrence.failureReason === 'string' &&
      occurrence.failureReason.startsWith('authority:')
  );
  if (authorityRefusals.length > 0) {
    push(
      'authority.unresolved-refusal',
      'error',
      authorityRefusals.map((occurrence) => occurrence.id),
      'unresolved authority refusals are holding work'
    );
  }
  if (model.authority.revoked.size > 0) {
    push('authority.stale-identity', 'error', [...model.authority.revoked], 'familiar identity revoked or stale');
  }

  // Delivery and receipts.
  const deliveryFailures = [...model.runs.values()].filter((run) => run.outputCommit === 'failed');
  if (deliveryFailures.length > 0) {
    push(
      'delivery.commit-failures',
      'error',
      deliveryFailures.map((run) => run.id),
      `${deliveryFailures.length} run(s) failed to commit delivery`
    );
  }
  for (const receipt of model.receipts.values()) {
    if (!model.verifyReceipt(receipt)) {
      push('receipts.unverified', 'error', [receipt.receiptId], 'receipt digest does not verify');
      break;
    }
  }

  // Changefeed health: a subscriber left behind by more than a few events.
  if (model.subscribedCursor !== null && model.eventCursor - model.subscribedCursor > 3) {
    push(
      'events.cursor-lag',
      'warning',
      [],
      `subscriber is ${model.eventCursor - model.subscribedCursor} events behind the changefeed`
    );
  }

  // Retention pressure: changefeed outgrows its retention budget.
  if (model.retentionWindowMs !== null) {
    const budget = Math.max(1, Math.floor(model.retentionWindowMs / 1000));
    const published = model.events.length;
    if (published > budget) {
      push(
        'disk.retention-pressure',
        'warning',
        [],
        `${published} retained events exceed the ${model.retentionWindowMs / 1000}s retention budget`
      );
    }
  }

  return doctorReport(findings, model);
}
