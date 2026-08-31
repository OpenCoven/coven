// Dispatch and scheduler-pass semantics for the conformance reference oracle.
//
// Mirrors the shipped scheduler contract: plan (misfire-latest) -> recover
// expired leases -> claim (compare-and-set, bounded lease) -> dispatch every
// claimed occurrence through the fail-closed authority check, delivery
// commit, settlement, and receipt sealing. Every durable or external
// boundary the certification inventory names is an injectable crash point:
// plan, claim, adoption, dispatch, session-create, terminal-observation,
// delivery, settlement, receipt, event-publication.

import { iso } from './clock.mjs';

// The authority check runs at dispatch time — the only real dispatch point —
// so the decision and the dispatch cannot drift apart. Fail-closed: any
// doubt refuses the run and records the refusal.
export function authorityRefusal(model, record) {
  const binding = model.authority.bindings.get(record.definition.id);
  if (!binding) return null; // legacy v1 surface without an authority binding
  if (!model.authority.available) {
    return 'authority unavailable; dispatch refused fail-closed';
  }
  if (
    model.authority.revoked.has(binding.familiarId) ||
    model.authority.familiarRevisions.get(binding.familiarId) !== binding.familiarRevision
  ) {
    return 'familiar revision stale or revoked';
  }
  const held = model.authority.runtimeCapabilities.get(record.definition.runtime) ?? [];
  const missing = (binding.requiredCapabilities ?? []).filter(
    (capability) => !held.includes(capability)
  );
  if (missing.length > 0) {
    return `runtime capability downgrade: missing ${missing.join(',')}`;
  }
  if (binding.approval?.required) {
    const approval = model.authority.approvals.get(binding.approval.nonce ?? '');
    if (!approval) return 'approval required; dispatch refused';
    if (approval.consumed) return 'approval replayed; nonces are single-use';
    if (approval.expiresAtMs !== null && approval.expiresAtMs <= model.now) {
      return 'approval expired';
    }
    if (approval.automationId !== record.definition.id) {
      return 'approval issued for a different automation (confused deputy refused)';
    }
  }
  return null;
}

// Consumes a pending crash at the named boundary: the pass dies with every
// durable effect up to and including that boundary. Buffered changefeed
// events live in process memory, so they are lost with the crash — the store
// keeps state, the changefeed keeps only what a completed pass published.
export function hitCrashBoundary(model, boundary) {
  if (model.pendingCrash !== boundary) return false;
  model.pendingCrash = null;
  model.dead = true;
  model.eventBuffer = [];
  return true;
}

export function enforceTimeouts(model, now) {
  for (const run of model.runs.values()) {
    if (run.status !== 'running') continue;
    if (run.timeoutAtMs === null || run.timeoutAtMs > now) continue;
    const occurrence = run.occurrenceId ? model.occurrences.get(run.occurrenceId) : null;
    if (
      occurrence &&
      occurrence.leaseExpiresAtMs !== null &&
      occurrence.leaseExpiresAtMs < now
    ) {
      continue; // lease recovery owns this run; the process was down
    }
    const cancelled = model.finishRun(run.id, { status: 'cancelled' }, now);
    if (cancelled) {
      run.failureReason = 'timeout';
      if (run.occurrenceId && model.occurrences.get(run.occurrenceId)) {
        model.settleOccurrence(run.occurrenceId, 'failed', 'timeout', now);
      }
    }
  }
}

// Dispatches every claimed occurrence, oldest slot first. Mirrors the shared
// session-launch path: re-read the definition, enforce authority fail-closed
// at dispatch time, record the ledger row, launch, settle.
export function dispatchClaimed(model, now) {
  const claimed = [...model.occurrences.values()]
    .filter((occurrence) => occurrence.state === 'claimed')
    .sort((a, b) => a.scheduledForMs - b.scheduledForMs);
  for (const occurrence of claimed) {
    if (model.dead) break;
    dispatchOne(model, occurrence, now);
    if (model.dead) break;
  }
}

function dispatchOne(model, occurrence, now) {
  const record = model.definitions.get(occurrence.automationId);
  if (!record) {
    model.settleOccurrence(occurrence.id, 'failed', 'routine vanished during dispatch', now);
    return;
  }
  const cwd = record.definition.cwd;
  if (typeof cwd !== 'string' || cwd.trim() === '') {
    model.settleOccurrence(
      occurrence.id,
      'failed',
      'routine has no cwd; add a cwd before running',
      now
    );
    return;
  }
  if (record.definition.overlap === 'forbid') {
    const unsettled = [...model.occurrences.values()].some(
      (other) =>
        other.automationId === record.definition.id &&
        other.id !== occurrence.id &&
        ['claimed', 'running'].includes(other.state)
    );
    if (unsettled) return; // overlap forbid: skipped this pass, retried next pass
  }

  const refusal = authorityRefusal(model, record);
  if (refusal !== null) {
    model.settleOccurrence(occurrence.id, 'failed', `authority: ${refusal}`, now);
    model.emit('authority.refused', {
      automationId: record.definition.id,
      occurrenceId: occurrence.id,
      reason: 'dispatch refused fail-closed'
    });
    return;
  }

  const runId = `run-${occurrence.id}-a${occurrence.attempt}`;
  const run = {
    id: runId,
    automationId: record.definition.id,
    occurrenceId: occurrence.id,
    sessionId: null,
    familiarId: record.definition.familiarId ?? null,
    runtime: record.definition.runtime,
    status: 'running',
    exitCode: null,
    logJson: null,
    outputCommit: null,
    ambiguous: false,
    adoptionKey: occurrence.adoptionKey ?? null,
    orchestrator: occurrence.orchestrator ?? 'direct',
    behaviorAtLaunch: model.runtimeBehavior,
    failureReason: null,
    startedAtMs: now,
    finishedAtMs: null,
    timeoutAtMs: now + record.definition.timeoutMinutes * 60e3
  };
  model.runs.set(runId, run);
  model.emit('run.started', { runId, automationId: record.definition.id });
  if (hitCrashBoundary(model, 'session-create')) return;

  // The launch attempt is logged exactly once per occurrence; the behavior
  // decided above determines its outcome.
  model.dispatchLog.push({
    occurrenceId: occurrence.id,
    automationId: record.definition.id,
    runId,
    attempt: occurrence.attempt,
    at: iso(model.now),
    behaviorAtLaunch: model.runtimeBehavior
  });
  run.sessionId = `session-${runId}`;
  const binding = model.authority.bindings.get(record.definition.id);
  if (binding?.approval?.required) {
    const approval = model.authority.approvals.get(binding.approval.nonce ?? '');
    if (
      approval &&
      !approval.consumed &&
      (approval.expiresAtMs === null || approval.expiresAtMs > model.now)
    ) {
      approval.consumed = true;
      model.emit('approval.consumed', { automationId: record.definition.id });
    }
  }
  if (hitCrashBoundary(model, 'terminal-observation')) return;

  if (model.runtimeBehavior === 'unavailable') {
    retryableDispatchFailure(model, occurrence, runId, 'runtime unavailable', now);
    return;
  }
  if (model.runtimeBehavior === 'lost-after-side-effect') {
    model.finishRun(runId, { status: 'failed', ambiguous: true }, now);
    run.failureReason = 'runtime lost; outcome ambiguous';
    model.settleOccurrence(occurrence.id, 'failed', 'runtime lost; outcome ambiguous', now);
    model.emit('run.ambiguous', { runId, occurrenceId: occurrence.id });
    return;
  }
  if (model.runtimeBehavior === 'timeout') {
    // The runtime never answers; the timeout observation lands on a later
    // pass (enforceTimeouts).
    return;
  }

  // Delivery commits before settlement so a failed delivery can never read
  // as a successful run (no false success under injected failure).
  if (model.deliveryBehavior === 'commit-failure') {
    model.emit('delivery.failed', {
      automationId: record.definition.id,
      occurrenceId: occurrence.id
    });
    model.finishRun(runId, { status: 'failed', exitCode: null, outputCommit: 'failed' }, now);
    run.failureReason = 'delivery commit failed after successful runtime outcome';
    model.settleOccurrence(
      occurrence.id,
      'failed',
      'delivery commit failed after runtime outcome',
      now
    );
    model.sealReceipt(model.runs.get(runId), now);
    return;
  }
  if (hitCrashBoundary(model, 'delivery')) return;

  model.settleOccurrence(occurrence.id, 'succeeded', null, now);
  model.finishRun(runId, { status: 'succeeded', exitCode: 0, outputCommit: 'committed' }, now);
  if (
    model.runtimeBehavior === 'duplicate-response' ||
    model.runtimeBehavior === 'out-of-order-response'
  ) {
    // A duplicate or late terminal callback arrives: terminal monotonicity
    // makes it a no-op, never a second settlement or a second dispatch.
    model.finishRun(runId, { status: 'failed' }, now);
    model.settleOccurrence(occurrence.id, 'failed', 'late callback', now);
  }
  if (hitCrashBoundary(model, 'settlement')) return;
  if (hitCrashBoundary(model, 'receipt')) return;
  model.sealReceipt(run, now);
}

export function retryableDispatchFailure(model, occurrence, runId, reason, now) {
  const run = model.runs.get(runId);
  model.finishRun(runId, { status: 'failed', exitCode: null }, now);
  if (run) run.failureReason = reason;
  const attempt = occurrence.attempt;
  const maxAttempts = model.retryPolicy.maxAttempts ?? 1;
  if (attempt >= maxAttempts) {
    const quarantined =
      maxAttempts > 1 ? model.settleOccurrence(occurrence.id, 'quarantined', reason, now) : false;
    if (!quarantined) model.settleOccurrence(occurrence.id, 'failed', reason, now);
    return;
  }
  const backoffMs = (model.retryPolicy.backoffSeconds ?? 0) * 1000 * 2 ** (attempt - 1);
  occurrence.state = 'planned';
  occurrence.nextEligibleAtMs = model.now + backoffMs;
  occurrence.failureReason = reason;
  occurrence.leaseOwner = null;
  occurrence.leaseExpiresAtMs = null;
  model.emit('occurrence.planned', {
    automationId: occurrence.automationId,
    occurrenceId: occurrence.id,
    retry: true
  });
}

// Publishes buffered changefeed events durably, one at a time. The
// 'first-event' crash boundary lands after the first event of the pass is
// durable: that event survives, the rest of the buffer is lost with the
// crash, and the pass dies. When nothing is buffered the boundary never
// fires (there is no first event to crash after).
export function publishEvents(model, boundary = null) {
  let published = 0;
  while (model.eventBuffer.length > 0) {
    model.events.push(model.eventBuffer.shift());
    published += 1;
    if (boundary !== null && hitCrashBoundary(model, boundary)) {
      return { published, aborted: boundary };
    }
  }
  return { published, aborted: null };
}

// One scheduler pass: plan due slots, recover expired leases, claim due
// occurrences, dispatch. Events are buffered and published only when the
// pass completes, so a crash loses the pass's changefeed entries, never
// state. Crash boundaries are consumed in place by hitCrashBoundary as the
// pass reaches them.
export function runPass(model, now) {
  if (model.dead) return { skipped: true, reason: 'process is dead' };
  enforceTimeouts(model, now);

  for (const record of activeDefinitions(model)) {
    model.planLatest(record, now);
    if (hitCrashBoundary(model, 'plan')) return { aborted: 'plan' };
  }
  model.recoverExpiredLeases(now);
  for (const record of activeDefinitions(model)) {
    model.claimDue(record.definition.id, 'daemon', 60, now);
    if (hitCrashBoundary(model, 'claim')) return { aborted: 'claim' };
  }
  if (hitCrashBoundary(model, 'dispatch')) return { aborted: 'dispatch' };
  dispatchClaimed(model, now);
  if (model.dead) return { aborted: 'dispatch' };
  if (hitCrashBoundary(model, 'event-publication')) {
    model.eventBuffer = [];
    return { aborted: 'event-publication' };
  }
  const published = publishEvents(model, 'first-event');
  if (published.aborted !== null) return { aborted: published.aborted };
  model.recordSuccessfulPass(now, activeDefinitions(model).map((record) => record.definition.id));
  return { ok: true };
}

// A manual run-now pass: adoption-keyed fence, immediate claim, dispatch.
export function runNowPass(model, op) {
  if (model.dead) return { skipped: true, reason: 'process is dead' };
  const automationId = op.automationId;
  const record = model.definitions.get(automationId);
  if (!record) {
    return model.refuse(op.label, `no routine with id \`${automationId}\``);
  }
  const adoptionKey = op.adoptionKey ?? 'default';
  const occurrenceId = `runnow-${automationId}-${adoptionKey}`;
  if (model.occurrences.has(occurrenceId)) {
    return model.refuse(op.label, 'adoption key already consumed; one adoption key fences one run');
  }
  const slotIso = iso(model.now);
  const fenceKey = `${automationId}|${slotIso}`;
  if (model.fenceIndex.has(fenceKey)) {
    return model.refuse(op.label, 'immediate occurrence fence collided; retry');
  }
  const occurrence = {
    id: occurrenceId,
    automationId,
    scheduledFor: slotIso,
    scheduledForMs: model.now,
    state: 'planned',
    leaseOwner: null,
    leaseExpiresAtMs: null,
    attempt: 0,
    failureReason: null,
    nextEligibleAtMs: null,
    adoptionKey,
    orchestrator: op.orchestrator ?? 'direct',
    createdAtMs: model.now,
    updatedAtMs: model.now
  };
  model.occurrences.set(occurrenceId, occurrence);
  model.fenceIndex.set(fenceKey, occurrenceId);
  model.emit('occurrence.planned', { automationId, occurrenceId, scheduledFor: slotIso });
  if (hitCrashBoundary(model, 'adoption')) return { aborted: 'adoption' };
  occurrence.state = 'claimed';
  occurrence.leaseOwner = 'manual';
  occurrence.leaseExpiresAtMs = model.now + 60 * 60e3;
  occurrence.attempt += 1;
  model.emit('occurrence.claimed', { automationId, occurrenceId, leaseOwner: 'manual' });
  if (hitCrashBoundary(model, 'claim')) return { aborted: 'claim' };
  if (hitCrashBoundary(model, 'dispatch')) return { aborted: 'dispatch' };
  dispatchOne(model, occurrence, model.now);
  const published = publishEvents(model, 'first-event');
  if (published.aborted !== null) return { aborted: published.aborted };
  return { ok: true };
}

function activeDefinitions(model) {
  return [...model.definitions.values()]
    .filter((record) => record.definition.status === 'ACTIVE')
    .sort((a, b) => (a.definition.id < b.definition.id ? -1 : 1));
}
