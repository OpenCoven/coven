// Core state and primitives for the conformance reference oracle.
//
// This module owns the durable state shape and the primitives every
// automation surface shares: definitions, the occurrence fence with
// misfire-latest planning, compare-and-set claims with leases, the run
// ledger with terminal monotonicity and bounded logs, and sealed receipts
// over canonical JSON. Dispatch lives in dispatch.mjs, vector operations in
// ops.mjs, and outcome evaluation in evaluate.mjs.

import { createHash } from 'node:crypto';
import { iso, parseIso, latestDueSlot } from './clock.mjs';

export const LOG_ENTRY_MAX_CHARS = 64 * 1024;
export const TICK_CADENCE_MS = 60e3;
export const TERMINAL_OCCURRENCE_STATES = new Set(['succeeded', 'failed', 'quarantined']);
export const CRASH_BOUNDARIES = [
  'plan',
  'claim',
  'adoption',
  'dispatch',
  'session-create',
  'terminal-observation',
  'delivery',
  'settlement',
  'receipt'
];

export function sha256Hex(text) {
  return createHash('sha256').update(text).digest('hex');
}

// Canonical JSON: recursively sorted object keys, no insignificant whitespace.
export function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  const keys = Object.keys(value).sort();
  return `{${keys.map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`;
}

export class ConformanceModel {
  constructor({ start, startIso, hostTimezone = 'UTC' } = {}) {
    // Vectors declare virtualTime.start; startIso is accepted as an alias.
    const startInstant = startIso ?? start;
    if (!startInstant) throw new Error('the reference oracle requires a virtual start instant');
    this.now = parseIso(startInstant);
    this.hostTimezone = hostTimezone;
    this.dead = false;
    this.pendingCrash = null;
    this.definitions = new Map(); // id -> { definition, revision, createdAtMs }
    this.occurrences = new Map(); // id -> occurrence record
    this.fenceIndex = new Map(); // `automationId|slotIso` -> occurrence id
    this.runs = new Map();
    this.dispatchLog = [];
    this.transitions = [];
    this.refusals = [];
    this.definitionRefusals = [];
    this.events = [];
    this.eventBuffer = [];
    this.eventCursor = 0;
    this.projection = { eventCount: 0 };
    this.appliedEventIds = new Set();
    this.subscribedCursor = null;
    this.lastReplayTypes = [];
    this.lastReceiptVerification = null;
    this.passes = []; // { at, activeIds } per successful scheduler pass
    this.authority = {
      available: true,
      bindings: new Map(),
      familiarRevisions: new Map(),
      revoked: new Set(),
      runtimeCapabilities: new Map(),
      approvals: new Map()
    };
    this.runtimeBehavior = 'available';
    this.deliveryBehavior = 'commit';
    this.retryPolicy = { maxAttempts: 1, backoffSeconds: 0 };
    this.retentionWindowMs = null;
    this.leader = { isLeader: true, lastSuccessfulPassAtMs: null, lastPassDurationMs: 0 };
    this.passCount = 0;
    this.receipts = new Map();
    this.receiptTampered = false;
    this.redactionApplied = false;
    this.erasedAutomationIds = new Set();
    this.tickCount = 0;
    this.operationsApplied = 0;
  }

  tzFor(timezone) {
    if (timezone === 'utc') return 'UTC';
    if (timezone === 'local') return this.hostTimezone;
    return timezone;
  }

  emit(type, payload = {}) {
    this.eventCursor += 1;
    this.eventBuffer.push({
      cursor: this.eventCursor,
      id: `evt-${this.eventCursor}`,
      type,
      at: iso(this.now),
      payload
    });
    return this.eventCursor;
  }

  flushEvents() {
    this.events.push(...this.eventBuffer);
    this.eventBuffer = [];
  }

  recordSuccessfulPass(now, activeIds = []) {
    this.passCount += 1;
    this.passes.push({ at: now, activeIds });
    this.leader.lastSuccessfulPassAtMs = now;
    this.leader.lastPassDurationMs = 0; // deterministic virtual pass
  }

  recordTransition(kind, id, from, to) {
    this.transitions.push({ kind, id, from, to, at: iso(this.now) });
  }

  refuse(label, reason) {
    if (label !== undefined && label !== null) {
      this.refusals.push({ label, reason });
    }
    return { refused: true, reason };
  }

  setNow(at) {
    if (at === undefined || at === null) return;
    const millis = parseIso(at);
    this.now = millis; // clock jumps (including backward) are modeled exactly
  }

  // ---------------------------------------------------------------- writing

  insertDefinition(document) {
    if (this.definitions.has(document.id)) {
      this.definitionRefusals.push(`routine \`${document.id}\` already exists`);
      return false;
    }
    this.definitions.set(document.id, {
      definition: structuredClone(document),
      revision: 1,
      createdAtMs: this.now
    });
    this.emit('definition.created', { automationId: document.id });
    return true;
  }

  maxFencedMs(automationId) {
    let max = null;
    for (const occurrence of this.occurrences.values()) {
      if (occurrence.automationId !== automationId) continue;
      if (max === null || occurrence.scheduledForMs > max) max = occurrence.scheduledForMs;
    }
    return max;
  }

  // Plans the latest due slot for one definition (misfire-latest). The walk
  // starts at the later of the definition's creation and its latest fenced
  // slot: slots before the routine existed are never backfilled and fenced
  // slots are never re-planned.
  planLatest(record, now) {
    const { definition } = record;
    if (definition.status !== 'ACTIVE') return { outcome: 'paused' };
    const latestFencedMs = this.maxFencedMs(definition.id);
    const cursorMs = Math.max(record.createdAtMs, latestFencedMs ?? Number.NEGATIVE_INFINITY);
    const slot = latestDueSlot(definition.rrule, definition.timezone, cursorMs, now, this.hostTimezone);
    if (slot === null) return { outcome: 'not-due' };
    const slotIso = iso(slot);
    const fenceKey = `${definition.id}|${slotIso}`;
    if (this.fenceIndex.has(fenceKey)) return { outcome: 'already-fenced' };
    const occurrenceId = `${definition.id}-${slot}`;
    this.occurrences.set(occurrenceId, {
      id: occurrenceId,
      automationId: definition.id,
      scheduledFor: slotIso,
      scheduledForMs: slot,
      state: 'planned',
      leaseOwner: null,
      leaseExpiresAtMs: null,
      attempt: 0,
      failureReason: null,
      nextEligibleAtMs: null,
      createdAtMs: now,
      updatedAtMs: now
    });
    this.fenceIndex.set(fenceKey, occurrenceId);
    this.emit('occurrence.planned', {
      automationId: definition.id,
      occurrenceId,
      scheduledFor: slotIso
    });
    return { outcome: 'planned', occurrenceId };
  }

  // Claims the earliest eligible PLANNED occurrence with a bounded lease.
  // Only 'planned' rows move to 'claimed', so a second claimant finds nothing.
  claimDue(automationId, owner, leaseMinutes, now) {
    if (!Number.isInteger(leaseMinutes) || leaseMinutes <= 0 || leaseMinutes > 24 * 60) {
      return { error: 'lease minutes must be 1..=1440' };
    }
    let earliest = null;
    for (const occurrence of this.occurrences.values()) {
      if (occurrence.automationId !== automationId) continue;
      if (occurrence.state !== 'planned') continue;
      if (occurrence.scheduledForMs > now) continue;
      if ((occurrence.nextEligibleAtMs ?? 0) > now) continue;
      if (earliest === null || occurrence.scheduledForMs < earliest.scheduledForMs) {
        earliest = occurrence;
      }
    }
    if (!earliest) return { claimed: null };
    earliest.state = 'claimed';
    earliest.leaseOwner = owner;
    earliest.leaseExpiresAtMs = now + leaseMinutes * 60e3;
    earliest.attempt += 1;
    earliest.updatedAtMs = now;
    this.emit('occurrence.claimed', {
      automationId,
      occurrenceId: earliest.id,
      leaseOwner: owner
    });
    return { claimed: earliest.id };
  }

  recoverExpiredLeases(now) {
    let recovered = 0;
    for (const occurrence of this.occurrences.values()) {
      if (!['claimed', 'running'].includes(occurrence.state)) continue;
      if (occurrence.leaseExpiresAtMs === null || occurrence.leaseExpiresAtMs >= now) continue;
      this.settleOccurrence(occurrence.id, 'failed', 'lease expired', now);
      for (const run of this.runs.values()) {
        if (run.occurrenceId === occurrence.id && run.status === 'running') {
          this.finishRun(run.id, { status: 'failed' }, now);
          run.failureReason = 'lease expired';
        }
      }
      recovered += 1;
    }
    return recovered;
  }

  settleOccurrence(occurrenceId, terminalState, failureReason, now) {
    const occurrence = this.occurrences.get(occurrenceId);
    if (!occurrence) return false;
    if (!['succeeded', 'failed', 'quarantined'].includes(terminalState)) {
      throw new Error(`terminal state must be succeeded, failed, or quarantined; got ${terminalState}`);
    }
    if (!['claimed', 'running'].includes(occurrence.state)) return false; // monotonic
    this.recordTransition('occurrence', occurrenceId, occurrence.state, terminalState);
    occurrence.state = terminalState;
    occurrence.failureReason = failureReason ?? null;
    occurrence.leaseOwner = null;
    occurrence.leaseExpiresAtMs = null;
    occurrence.updatedAtMs = now;
    this.emit('occurrence.settled', {
      automationId: occurrence.automationId,
      occurrenceId,
      state: terminalState
    });
    return true;
  }

  finishRun(runId, finish, now) {
    const run = this.runs.get(runId);
    if (!run) return false;
    if (run.status !== 'running') return false; // terminal monotonicity
    if (!['succeeded', 'failed', 'cancelled'].includes(finish.status)) {
      throw new Error('run status must be succeeded, failed, or cancelled');
    }
    let log = finish.logJson ?? run.logJson ?? null;
    if (log !== null && log.length > LOG_ENTRY_MAX_CHARS) {
      log = `${log.slice(0, LOG_ENTRY_MAX_CHARS)}…(truncated)`;
    }
    this.recordTransition('run', runId, run.status, finish.status);
    run.status = finish.status;
    run.exitCode = finish.exitCode ?? null;
    if (finish.sessionId !== undefined) run.sessionId = finish.sessionId;
    if (finish.logJson !== undefined && finish.logJson !== null) run.logJson = log;
    if (finish.outputCommit !== undefined) run.outputCommit = finish.outputCommit;
    if (finish.ambiguous !== undefined) run.ambiguous = finish.ambiguous;
    run.finishedAtMs = now;
    this.emit('run.finished', { runId, status: finish.status });
    return true;
  }

  // --------------------------------------------------------------- receipts

  bindingDigest(automationId) {
    const binding = this.authority.bindings.get(automationId);
    return binding ? sha256Hex(canonicalJson(binding)) : 'unbound';
  }

  sealReceipt(run, now) {
    if (!run) return null;
    const record = this.definitions.get(run.automationId);
    const receipt = {
      receiptVersion: 1,
      receiptId: `receipt-${run.id}`,
      runId: run.id,
      occurrenceId: run.occurrenceId,
      automationId: run.automationId,
      bindingDigest: this.bindingDigest(run.automationId),
      orchestrator: run.orchestrator ?? 'direct',
      outcome: run.ambiguous ? 'ambiguous' : run.status,
      outputCommit: run.outputCommit ?? null,
      delivery:
        run.outputCommit === 'failed'
          ? 'failed'
          : run.outputCommit === 'committed'
            ? 'committed'
            : 'not-applicable',
      at: iso(now),
      artifacts: {
        'coven.automations.definition.v1': sha256Hex(canonicalJson(record?.definition ?? {}))
      }
    };
    receipt.digest = sha256Hex(canonicalJson(receipt));
    this.receipts.set(receipt.receiptId, receipt);
    this.emit('receipt.sealed', {
      runId: run.id,
      receiptId: receipt.receiptId,
      outcome: receipt.outcome
    });
    return receipt;
  }

  verifyReceipt(receipt) {
    const { digest, ...rest } = receipt;
    return sha256Hex(canonicalJson(rest)) === digest;
  }

  tamperFirstReceipt() {
    const first = this.receipts.values().next();
    if (first.done) return null;
    const receipt = first.value;
    const tampered = structuredClone(receipt);
    tampered.digest = 'f'.repeat(64);
    this.receipts.set(receipt.receiptId, tampered);
    this.receiptTampered = true;
    return tampered;
  }

  redactReceiptsAndLogs() {
    for (const run of this.runs.values()) {
      if (run.logJson !== null) run.logJson = '[redacted]';
    }
    this.redactionApplied = true;
  }

  eraseAutomation(automationId) {
    const record = this.definitions.get(automationId);
    if (!record) return { refused: true, reason: `no routine with id \`${automationId}\`` };
    record.definition.prompt = '[erased]';
    record.definition.status = 'PAUSED';
    this.erasedAutomationIds.add(automationId);
    this.emit('definition.tombstoned', { automationId });
    return { ok: true };
  }
}
