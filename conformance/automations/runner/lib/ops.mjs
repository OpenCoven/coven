// Vector operation interpreter for the conformance reference oracle.
//
// Every vector's `input.operations` script runs through applyOperation.
// Operations are target-agnostic: the same script certifies a daemon or a
// packaged release through a target adapter once that target advertises
// `coven.automations.conformance.v1`.

import { ConformanceModel } from './model.mjs';
import { enforceTimeouts, runNowPass, runPass } from './dispatch.mjs';

const CRASH_BOUNDARIES = new Set([
  'plan',
  'claim',
  'adoption',
  'dispatch',
  'session-create',
  'first-event',
  'terminal-observation',
  'delivery',
  'settlement',
  'receipt',
  'event-publication'
]);

function parseWindowMs(window) {
  const match = /^(\d+)([smhd])$/.exec(window ?? '');
  if (!match) throw new Error(`retention window must look like 90d, 720h, 60m, or 3600s; got ${window}`);
  const scale = { s: 1000, m: 60e3, h: 3600e3, d: 24 * 3600e3 };
  return Number(match[1]) * scale[match[2]];
}

function replayEventsAfter(model, afterCursor) {
  return model.events.filter((event) => event.cursor > afterCursor);
}

function applyReducer(model, event) {
  if (model.appliedEventIds.has(event.id)) return false; // duplicate delivery
  model.appliedEventIds.add(event.id);
  model.projection[event.type] = (model.projection[event.type] ?? 0) + 1;
  model.projection.eventCount += 1;
  return true;
}

const PROCESS_RESIDENT_OPS = new Set([
  'tick',
  'runNow',
  'cancel',
  'activate',
  'pause',
  'erase',
  'setDefinitionRevision',
  'redactReceipts'
]);

export function applyOperation(model, op) {
  if (model.dead && PROCESS_RESIDENT_OPS.has(op.op)) {
    return { skipped: true, reason: 'process is dead (crashed); restart to continue' };
  }
  model.operationsApplied += 1;

  switch (op.op) {
    case 'tick': {
      model.setNow(op.at);
      return runPass(model, model.now);
    }
    case 'advance':
    case 'suspend':
    case 'resume': {
      model.setNow(op.at);
      enforceTimeouts(model, model.now);
      return { ok: true };
    }
    case 'setHostTimezone': {
      model.hostTimezone = op.hostTimezone;
      return { ok: true };
    }
    case 'activate':
    case 'pause': {
      model.setNow(op.at);
      const record = model.definitions.get(op.automationId);
      if (!record) {
        return model.refuse(op.label, `no routine with id \`${op.automationId}\``);
      }
      record.definition.status = op.op === 'activate' ? 'ACTIVE' : 'PAUSED';
      model.emit('definition.updated', { automationId: op.automationId });
      return { ok: true };
    }
    case 'runNow': {
      model.setNow(op.at);
      return runNowPass(model, op);
    }
    case 'cancel': {
      model.setNow(op.at);
      let target = null;
      if (op.runId) {
        target = model.runs.get(op.runId) ?? null;
      } else if (op.automationId) {
        for (const run of model.runs.values()) {
          if (run.automationId === op.automationId && run.status === 'running') target = run;
        }
      }
      if (!target || target.status !== 'running') {
        return { ok: true, noOp: true, reason: 'nothing running to cancel; the race loser is a no-op' };
      }
      model.finishRun(target.id, { status: 'cancelled' }, model.now);
      target.failureReason = `cancelled: ${op.reason ?? 'operator requested'}`;
      if (target.occurrenceId && model.occurrences.get(target.occurrenceId)) {
        model.settleOccurrence(
          target.occurrenceId,
          'failed',
          `cancelled: ${op.reason ?? 'operator requested'}`,
          model.now
        );
      }
      return { ok: true };
    }
    case 'crash': {
      if (op.during !== undefined) {
        if (!CRASH_BOUNDARIES.has(op.during)) {
          throw new Error(`unknown crash boundary: ${op.during}`);
        }
        model.pendingCrash = op.during;
        return { ok: true, pendingCrash: op.during };
      }
      model.dead = true;
      return { ok: true };
    }
    case 'restart': {
      model.setNow(op.at);
      model.dead = false;
      return runPass(model, model.now); // startup reconcile: plan, recover, claim, dispatch
    }
    case 'runtimeBehavior': {
      model.runtimeBehavior = op.behavior;
      return { ok: true };
    }
    case 'deliveryBehavior': {
      model.deliveryBehavior = op.delivery;
      return { ok: true };
    }
    case 'setRetryPolicy': {
      model.retryPolicy = {
        maxAttempts: op.maxAttempts ?? 1,
        backoffSeconds: op.backoffSeconds ?? 0
      };
      return { ok: true };
    }
    case 'grantApproval': {
      model.setNow(op.at);
      model.authority.approvals.set(op.nonce, {
        automationId: op.automationId,
        expiresAtMs: op.ttlMinutes ? model.now + op.ttlMinutes * 60e3 : null,
        consumed: false
      });
      return { ok: true };
    }
    case 'expireApprovals': {
      model.setNow(op.at);
      for (const approval of model.authority.approvals.values()) {
        if (approval.expiresAtMs !== null && approval.expiresAtMs <= model.now) {
          approval.expired = true;
        }
      }
      return { ok: true };
    }
    case 'revokeFamiliar': {
      const binding = model.authority.bindings.get(op.automationId);
      if (!binding) {
        return model.refuse(op.label, `no authority binding for \`${op.automationId}\``);
      }
      model.authority.revoked.add(binding.familiarId);
      return { ok: true };
    }
    case 'setRuntimeCapabilities': {
      const binding = model.authority.bindings.get(op.automationId);
      const runtime = binding?.runtime ?? model.definitions.get(op.automationId)?.definition.runtime;
      if (!runtime) {
        return model.refuse(op.label, `no runtime to cap for \`${op.automationId}\``);
      }
      model.authority.runtimeCapabilities.set(runtime, op.capabilities ?? []);
      return { ok: true };
    }
    case 'setAuthority': {
      model.authority.available = op.available ?? false;
      return { ok: true };
    }
    case 'setDefinitionRevision': {
      const record = model.definitions.get(op.automationId);
      if (!record) {
        return model.refuse(op.label, `no routine with id \`${op.automationId}\``);
      }
      if (op.revision < record.revision) {
        return model.refuse(
          op.label,
          `stale definition update: revision ${op.revision} below current ${record.revision}`
        );
      }
      record.revision = op.revision;
      model.emit('definition.updated', { automationId: op.automationId, revision: op.revision });
      return { ok: true };
    }
    case 'subscribe': {
      model.subscribedCursor = op.afterCursor ?? 0;
      const replay = replayEventsAfter(model, model.subscribedCursor);
      model.lastReplayTypes = replay.map((event) => event.type);
      return { ok: true, replayed: model.lastReplayTypes.length };
    }
    case 'deliver': {
      const replay = replayEventsAfter(model, op.afterCursor ?? model.subscribedCursor ?? 0);
      const times = op.times ?? 2;
      for (let round = 0; round < times; round += 1) {
        for (const event of replay) applyReducer(model, event);
      }
      model.lastReplayTypes = replay.map((event) => event.type);
      return { ok: true, delivered: replay.length * times, unique: replay.length };
    }
    case 'verifyReceipts': {
      let all = true;
      for (const receipt of model.receipts.values()) {
        if (!model.verifyReceipt(receipt)) all = false;
      }
      model.lastReceiptVerification = all;
      return { ok: true, verified: all };
    }
    case 'tamperReceipt': {
      model.tamperFirstReceipt();
      return { ok: true };
    }
    case 'redactReceipts': {
      model.redactReceiptsAndLogs();
      return { ok: true };
    }
    case 'erase': {
      model.setNow(op.at);
      return model.eraseAutomation(op.automationId);
    }
    case 'setRetention': {
      model.retentionWindowMs = parseWindowMs(op.window);
      return { ok: true };
    }
    case 'setLeader': {
      model.leader.isLeader = op.available ?? false;
      return { ok: true };
    }
    default:
      throw new Error(`unknown operation: ${op.op}`);
  }
}

// Runs one vector's input against a fresh model. Returns the model; the
// seeded bindings get their runtime capability set initialized to the
// required set so "downgrade" is always an explicit operation.
export function runVectorInput(vector) {
  const virtualTime = vector.virtualTime ?? {};
  const model = new ConformanceModel(virtualTime);
  const input = vector.input ?? {};

  for (const document of input.definitions ?? []) {
    model.insertDefinition(document);
  }
  for (const binding of input.bindings ?? []) {
    model.authority.bindings.set(binding.automationId, structuredClone(binding));
    if (!model.authority.familiarRevisions.has(binding.familiarId)) {
      model.authority.familiarRevisions.set(binding.familiarId, binding.familiarRevision);
    }
    const record = model.definitions.get(binding.automationId);
    const runtime = binding.runtime ?? record?.definition.runtime;
    if (runtime && !model.authority.runtimeCapabilities.has(runtime)) {
      model.authority.runtimeCapabilities.set(runtime, binding.requiredCapabilities ?? []);
    }
  }
  for (const op of input.operations ?? []) {
    applyOperation(model, op);
  }
  return model;
}
