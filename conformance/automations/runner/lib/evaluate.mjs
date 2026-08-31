// Vector evaluation and invariant checking for the conformance plane.
//
// A vector passes when every declared expectation matches the oracle's final
// state AND every invariant held continuously (invariants are checked after
// every operation, not only at the end). Failures carry the invariant, object
// ids, the event cursor, expected/observed state, and the exact safe
// reproduction command the report requires.

import { latestDueSlot, latestDueSlotBrute, parseRrule } from './clock.mjs';
import { applyOperation } from './ops.mjs';
import { ConformanceModel } from './model.mjs';
import { doctorFindings } from './doctor.mjs';
import { validateAgainstSchema } from './schema.mjs';
import { redactText } from './redact.mjs';

const TERMINAL_OCCURRENCE = new Set(['succeeded', 'failed', 'quarantined']);
const TERMINAL_RUN = new Set(['succeeded', 'failed', 'cancelled']);

const INVARIANT_EXPECTED = {
  'no-duplicate-dispatch-per-fence': 'exactly one dispatch per occurrence fence',
  'no-silent-eligible-occurrence-loss': 'every eligible slot of an active routine is fenced',
  'no-false-success-under-injected-failure': 'a run never reads succeeded under injected failure',
  'terminal-state-monotonicity': 'terminal states never transition again',
  'fence-uniqueness': 'one occurrence per (automation, scheduled slot)',
  'bounded-ledger-growth': 'events and logs stay bounded by work performed',
  'occurrence-planner-agreement': 'the planner and the independent oracle agree on the latest due slot'
};

function failure(vector, invariant, expected, observed, objectIds, eventCursor) {
  return {
    vectorId: vector.vectorId,
    profile: vector.profile,
    invariant,
    objectIds: objectIds ?? [],
    eventCursor: eventCursor ?? null,
    expected,
    observed,
    reproduction: `node conformance/automations/runner/conformance.mjs --profile ${vector.profile} --vector ${vector.vectorId} --target reference`
  };
}

// Checks every invariant against the oracle state. Returns violations.
export function checkInvariants(model) {
  const violations = [];

  // fence-uniqueness: one occurrence per (automation, scheduled slot).
  const seenFences = new Map();
  for (const occurrence of model.occurrences.values()) {
    const key = `${occurrence.automationId}|${occurrence.scheduledFor}`;
    if (seenFences.has(key)) {
      violations.push({
        invariant: 'fence-uniqueness',
        objectIds: [seenFences.get(key), occurrence.id],
        observed: `duplicate fence for ${key}`
      });
    } else {
      seenFences.set(key, occurrence.id);
    }
  }

  // no-duplicate-dispatch-per-fence: at most one dispatch per attempt, and a
  // fence may only be re-dispatched after its previous attempt FAILED. A
  // second dispatch after a succeeded, cancelled, or ambiguous attempt is a
  // duplicate side effect and always a violation.
  const dispatchesByFence = new Map();
  for (const entry of model.dispatchLog) {
    const list = dispatchesByFence.get(entry.occurrenceId) ?? [];
    list.push(entry);
    dispatchesByFence.set(entry.occurrenceId, list);
  }
  for (const [occurrenceId, entries] of dispatchesByFence) {
    const attempts = new Set(entries.map((entry) => entry.attempt));
    if (attempts.size !== entries.length) {
      violations.push({
        invariant: 'no-duplicate-dispatch-per-fence',
        objectIds: [occurrenceId],
        observed: `${entries.length} dispatches across ${attempts.size} attempts for one fence`
      });
      continue;
    }
    for (let index = 1; index < entries.length; index += 1) {
      const previousRun = model.runs.get(entries[index - 1].runId);
      if (!previousRun || previousRun.status !== 'failed') {
        violations.push({
          invariant: 'no-duplicate-dispatch-per-fence',
          objectIds: [occurrenceId],
          observed: `re-dispatch after a previous attempt in status "${previousRun?.status ?? 'unknown'}"`
        });
        break;
      }
    }
  }

  // terminal-state-monotonicity: nothing leaves a terminal state.
  const terminalReached = new Set();
  for (const transition of model.transitions) {
    const key = `${transition.kind}:${transition.id}`;
    if (terminalReached.has(key)) {
      violations.push({
        invariant: 'terminal-state-monotonicity',
        objectIds: [transition.id],
        observed: `transition ${transition.from} -> ${transition.to} after a terminal state`
      });
      continue;
    }
    if (TERMINAL_OCCURRENCE.has(transition.to) || TERMINAL_RUN.has(transition.to)) {
      terminalReached.add(key);
    }
  }

  // no-false-success-under-injected-failure.
  for (const run of model.runs.values()) {
    if (run.status !== 'succeeded') continue;
    const availableLike = ['available', 'duplicate-response', 'out-of-order-response'];
    if (!availableLike.includes(run.behaviorAtLaunch) || run.ambiguous) {
      violations.push({
        invariant: 'no-false-success-under-injected-failure',
        objectIds: [run.id],
        observed: `run succeeded despite behavior "${run.behaviorAtLaunch}"`
      });
    }
    const occurrence = run.occurrenceId ? model.occurrences.get(run.occurrenceId) : null;
    if (occurrence && occurrence.state !== 'succeeded') {
      violations.push({
        invariant: 'no-false-success-under-injected-failure',
        objectIds: [run.id, occurrence.id],
        observed: `run succeeded but its occurrence is "${occurrence.state}"`
      });
    }
    const receipt = model.receipts.get(`receipt-${run.id}`);
    if (receipt && receipt.outcome !== 'succeeded') {
      violations.push({
        invariant: 'no-false-success-under-injected-failure',
        objectIds: [run.id, receipt.receiptId],
        observed: `receipt outcome "${receipt.outcome}" for a succeeded run`
      });
    }
  }

  // no-silent-eligible-occurrence-loss: the latest due slot of every active
  // routine must be fenced once a successful pass ran at or after it. The
  // oracle here is latestDueSlotBrute — structurally independent of the
  // planner's direct computation — so the invariant cannot share a bug with
  // the code it polices. The planner's answer is also cross-checked against
  // it; any disagreement is its own violation.
  for (const record of model.definitions.values()) {
    if (record.definition.status !== 'ACTIVE') continue;
    const brute = latestDueSlotBrute(
      record.definition.rrule,
      record.definition.timezone,
      record.createdAtMs,
      model.now,
      model.hostTimezone
    );
    const direct = latestDueSlot(
      record.definition.rrule,
      record.definition.timezone,
      record.createdAtMs,
      model.now,
      model.hostTimezone
    );
    if (brute !== direct) {
      violations.push({
        invariant: 'occurrence-planner-agreement',
        objectIds: [record.definition.id],
        observed: `planner says ${direct === null ? 'no slot' : new Date(direct).toISOString()} but the independent oracle says ${brute === null ? 'no slot' : new Date(brute).toISOString()}`
      });
    }
    const latest = brute;
    if (latest === null) continue;
    const fenced = [...model.occurrences.values()].some(
      (occurrence) =>
        occurrence.automationId === record.definition.id &&
        occurrence.scheduledForMs === latest
    );
    if (fenced) continue;
    const fenceablePass = model.passes.some(
      (pass) => pass.at >= Math.max(latest, record.createdAtMs) && pass.activeIds.includes(record.definition.id)
    );
    if (!fenceablePass) continue;
    violations.push({
      invariant: 'no-silent-eligible-occurrence-loss',
      objectIds: [record.definition.id],
      observed: `latest due slot ${new Date(latest).toISOString()} has no fenced occurrence after a successful pass`
    });
  }

  // bounded-ledger-growth: events stay bounded by work performed.
  const eventBudget = 24 * (model.definitions.size + model.passCount + model.runs.size) + 64;
  if (model.events.length > eventBudget) {
    violations.push({
      invariant: 'bounded-ledger-growth',
      objectIds: [],
      observed: `${model.events.length} events exceed the bounded budget`
    });
  }

  return violations;
}

// Redaction scan: every forbidden substring must be absent from everything
// the plane would publish about this vector's final state.
export function scanRedaction(vector, model) {
  const forbidden = vector.expected?.redaction?.forbiddenSubstrings ?? [];
  if (forbidden.length === 0) return [];
  const prompts = (vector.input?.definitions ?? [])
    .map((document) => document.prompt)
    .filter((prompt) => typeof prompt === 'string');
  const published = redactText(
    JSON.stringify({
      definitions: [...model.definitions.values()].map((record) => record.definition),
      occurrences: [...model.occurrences.values()],
      runs: [...model.runs.values()],
      events: model.events,
      refusals: model.refusals,
      receipts: [...model.receipts.values()],
      doctor: doctorFindings(model)
    }),
    prompts
  );
  const violations = [];
  for (const needle of forbidden) {
    if (published.includes(needle)) {
      violations.push({
        vectorId: vector.vectorId,
        profile: vector.profile,
        invariant: 'redaction',
        objectIds: [],
        eventCursor: model.eventCursor,
        expected: 'forbidden substring absent from diagnostic output',
        observed: 'forbidden substring detected in published output'
      });
    }
  }
  return violations;
}

// Compares the oracle's final state with the vector's expected block.
export function compareExpected(vector, model) {
  const failures = [];
  const expected = vector.expected ?? {};

  for (const refusal of expected.refusals ?? []) {
    const record = model.refusals.find((entry) => entry.label === refusal.refusalId);
    if (!record) {
      failures.push(
        failure(vector, 'refusal-behavior', `refusal "${refusal.refusalId}"`, 'no refusal recorded')
      );
    } else if (refusal.reasonMatches && !record.reason.includes(refusal.reasonMatches)) {
      failures.push(
        failure(vector, 'refusal-reason', refusal.reasonMatches, record.reason, [refusal.refusalId])
      );
    }
  }

  for (const expectedOccurrence of expected.occurrences ?? []) {
    const matches = [...model.occurrences.values()].filter(
      (occurrence) =>
        occurrence.automationId === expectedOccurrence.automationId &&
        (expectedOccurrence.scheduledFor === undefined ||
          occurrence.scheduledFor === expectedOccurrence.scheduledFor)
    );
    if (matches.length === 0) {
      failures.push(
        failure(vector, 'occurrence-state', expectedOccurrence, 'no matching occurrence')
      );
      continue;
    }
    const matched = matches.some(
      (occurrence) =>
        occurrence.state === expectedOccurrence.state &&
        (expectedOccurrence.attempt === undefined ||
          occurrence.attempt === expectedOccurrence.attempt) &&
        (expectedOccurrence.failureReasonMatches === undefined ||
          (occurrence.failureReason ?? '').includes(expectedOccurrence.failureReasonMatches))
    );
    if (!matched) {
      failures.push(
        failure(
          vector,
          'occurrence-state',
          expectedOccurrence,
          matches.map((occurrence) => ({
            id: occurrence.id,
            state: occurrence.state,
            attempt: occurrence.attempt,
            failureReason: occurrence.failureReason
          }))
        )
      );
    }
  }

  const runsByAutomation = new Map();
  for (const run of model.runs.values()) {
    const list = runsByAutomation.get(run.automationId) ?? [];
    list.push(run);
    runsByAutomation.set(run.automationId, list);
  }
  for (const expectedRun of expected.runs ?? []) {
    const runs = runsByAutomation.get(expectedRun.automationId) ?? [];
    const matched = runs.some(
      (run) =>
        run.status === expectedRun.status &&
        (expectedRun.ambiguous === undefined || run.ambiguous === expectedRun.ambiguous) &&
        (expectedRun.outputCommit === undefined ||
          run.outputCommit === expectedRun.outputCommit)
    );
    const countOk = expectedRun.count === undefined ? true : runs.length === expectedRun.count;
    if (!matched || !countOk) {
      failures.push(
        failure(
          vector,
          'run-state',
          expectedRun,
          runs.map((run) => ({
            id: run.id,
            status: run.status,
            ambiguous: run.ambiguous,
            outputCommit: run.outputCommit
          }))
        )
      );
    }
  }

  if (
    expected.dispatchCount !== undefined &&
    model.dispatchLog.length !== expected.dispatchCount
  ) {
    failures.push(
      failure(
        vector,
        'no-duplicate-dispatch-per-fence',
        `exactly ${expected.dispatchCount} runtime launch(es)`,
        `${model.dispatchLog.length} dispatches`,
        model.dispatchLog.map((entry) => entry.occurrenceId),
        model.eventCursor
      )
    );
  }

  for (const expectedEvent of expected.events ?? []) {
    const matching = model.events.filter(
      (event) =>
        event.type === expectedEvent.type &&
        (expectedEvent.automationId === undefined ||
          event.payload.automationId === expectedEvent.automationId)
    );
    if (matching.length < (expectedEvent.count ?? 1)) {
      failures.push(
        failure(
          vector,
          'event-cursor',
          `${expectedEvent.type} count >= ${expectedEvent.count ?? 1}`,
          `observed ${matching.length}`,
          [],
          model.eventCursor
        )
      );
    }
  }

  if (expected.replay !== undefined) {
    const actual = model.lastReplayTypes ?? [];
    if (JSON.stringify(actual) !== JSON.stringify(expected.replay.expectedEventTypes)) {
      failures.push(
        failure(
          vector,
          'cursor-replay',
          expected.replay.expectedEventTypes,
          actual,
          [],
          model.subscribedCursor
        )
      );
    }
  }

  if (expected.projection !== undefined) {
    if (JSON.stringify(model.projection) !== JSON.stringify(expected.projection)) {
      failures.push(failure(vector, 'reducer-projection', expected.projection, model.projection));
    }
  }

  const receiptsExpected = expected.receipts ?? null;
  if (receiptsExpected) {
    const receipts = [...model.receipts.values()];
    if (receiptsExpected.count !== undefined && receipts.length !== receiptsExpected.count) {
      failures.push(
        failure(vector, 'receipts', `${receiptsExpected.count} receipts`, `${receipts.length} receipts`)
      );
    }
    if (receiptsExpected.allVerify === true) {
      for (const receipt of receipts) {
        if (!model.verifyReceipt(receipt)) {
          failures.push(
            failure(vector, 'receipt-verification', 'valid digest', 'digest mismatch', [
              receipt.receiptId
            ])
          );
          break;
        }
      }
    }
    if (receiptsExpected.verifyAfterRedaction === true) {
      model.redactReceiptsAndLogs();
      for (const receipt of model.receipts.values()) {
        if (!model.verifyReceipt(receipt)) {
          failures.push(
            failure(
              vector,
              'receipt-verification-after-redaction',
              'valid digest after redaction',
              'digest mismatch',
              [receipt.receiptId]
            )
          );
          break;
        }
      }
    }
    if (receiptsExpected.verifyAfterTamper === false) {
      model.tamperFirstReceipt();
      const stillValid = [...model.receipts.values()].every((receipt) =>
        model.verifyReceipt(receipt)
      );
      if (stillValid) {
        failures.push(
          failure(
            vector,
            'receipt-tamper-detection',
            'tampered receipt fails verification',
            'tampered receipt verified'
          )
        );
      }
    }
    if (receiptsExpected.outcomes !== undefined) {
      const outcomes = receipts.map((receipt) => receipt.outcome).sort();
      const wanted = [...receiptsExpected.outcomes].sort();
      if (JSON.stringify(outcomes) !== JSON.stringify(wanted)) {
        failures.push(failure(vector, 'receipt-outcomes', receiptsExpected.outcomes, outcomes));
      }
    }
  }

  const doctorExpected = expected.doctor;
  if (doctorExpected) {
    const report = doctorFindings(model);
    const actualCodes = [...new Set(report.findings.map((finding) => finding.code))].sort();
    const expectedCodes = [...(doctorExpected.findingCodes ?? [])].sort();
    if (JSON.stringify(actualCodes) !== JSON.stringify(expectedCodes)) {
      failures.push(failure(vector, 'doctor-findings', expectedCodes, actualCodes));
    }
    for (const code of doctorExpected.noCodes ?? []) {
      if (actualCodes.includes(code)) {
        failures.push(failure(vector, 'doctor-findings', `no finding "${code}"`, actualCodes.join(',')));
      }
    }
  }

  return failures;
}

// Executes one vector. Structural vectors (with invalidDefinitions) validate
// documents against the definition schema; every other vector drives the
// reference oracle. Returns { model, failures }.
export function evaluateVector(vector, { definitionSchema } = {}) {
  const input = vector.input ?? {};
  const failures = [];

  // Structural mode: definition vectors validate documents against the
  // schema and the RRULE vocabulary gate; invalid ones must be refused with a
  // reason matching the declared refusal.
  if (vector.category === 'definitions' || (input.invalidDefinitions ?? []).length > 0) {
    for (const [index, document] of (input.definitions ?? []).entries()) {
      const errors = definitionSchema ? validateAgainstSchema(document, definitionSchema) : [];
      if (errors.length > 0) {
        failures.push(
          failure(
            vector,
            'schema-validity',
            'valid definition passes the schema',
            errors.join('; '),
            [document.id ?? `valid-${index}`]
          )
        );
      }
      try {
        parseRrule(document.rrule ?? '');
      } catch (error) {
        failures.push(
          failure(vector, 'rrule-vocabulary', 'valid rrule parses', error.message, [
            document.id ?? ''
          ])
        );
      }
    }
    for (const [index, document] of (input.invalidDefinitions ?? []).entries()) {
      // Refusal mirrors the implementation contract: schema validation first,
      // then the RRULE vocabulary gate. Both must refuse, never downgrade.
      const schemaErrors = definitionSchema
        ? validateAgainstSchema(document, definitionSchema)
        : ['no schema'];
      let rruleError = null;
      try {
        parseRrule(document.rrule ?? '');
      } catch (error) {
        rruleError = error.message;
      }
      const refusalReason =
        schemaErrors.length > 0 ? schemaErrors.join('; ') : rruleError;
      const expectation = (vector.expected?.refusals ?? [])[index];
      if (refusalReason === null) {
        failures.push(
          failure(
            vector,
            'definition-refusal',
            'invalid definition refused',
            'accepted',
            [document.id ?? `invalid-${index}`]
          )
        );
      } else if (expectation && !refusalReason.includes(expectation.reasonMatches)) {
        failures.push(
          failure(vector, 'definition-refusal-reason', expectation.reasonMatches, refusalReason)
        );
      }
    }
    return { model: null, failures };
  }

  // Oracle mode: seed, run the operation script, check invariants per step.
  const model = new ConformanceModel(vector.virtualTime ?? {});
  for (const document of input.definitions ?? []) {
    model.insertDefinition(document);
  }
  for (const binding of input.bindings ?? []) {
    model.authority.bindings.set(binding.automationId, structuredClone(binding));
    // The registry is independent of the binding: a binding that claims a
    // revision the registry does not know is forged and must refuse.
    if (!model.authority.familiarRevisions.has(binding.familiarId)) {
      model.authority.familiarRevisions.set(binding.familiarId, 1);
    }
    const runtime =
      binding.runtime ?? model.definitions.get(binding.automationId)?.definition.runtime;
    if (runtime && !model.authority.runtimeCapabilities.has(runtime)) {
      model.authority.runtimeCapabilities.set(runtime, binding.requiredCapabilities ?? []);
    }
  }

  const invariantFailures = [];
  for (const op of input.operations ?? []) {
    applyOperation(model, op);
    for (const violation of checkInvariants(model)) {
      invariantFailures.push(
        failure(
          vector,
          violation.invariant,
          INVARIANT_EXPECTED[violation.invariant] ?? 'invariant holds',
          violation.observed,
          violation.objectIds,
          model.eventCursor
        )
      );
    }
  }

  failures.push(...compareExpected(vector, model));
  failures.push(...scanRedaction(vector, model));
  const seen = new Set(failures.map((entry) => JSON.stringify(entry)));
  for (const invariantFailure of invariantFailures) {
    const key = JSON.stringify(invariantFailure);
    if (!seen.has(key)) {
      failures.push(invariantFailure);
      seen.add(key);
    }
  }

  return { model, failures };
}

// Deterministic randomized property testing: a seeded operation sequence
// that must preserve every invariant after every operation.
export function fuzzInvariants({ operations = 200, seed = 858 } = {}) {
  let state = seed >>> 0;
  const random = () => {
    state = (state + 0x6d2b79f5) | 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };

  const model = new ConformanceModel({
    startIso: '2026-03-01T00:00:00.000Z',
    hostTimezone: 'UTC'
  });
  model.insertDefinition({
    schemaVersion: 1,
    id: 'fuzz-daily',
    name: 'Fuzz daily',
    status: 'ACTIVE',
    rrule: 'FREQ=DAILY;BYHOUR=9,17',
    timezone: 'utc',
    misfire: 'latest',
    overlap: 'forbid',
    timeoutMinutes: 30,
    runtime: 'coven-code',
    cwd: 'work/fuzz',
    prompt: 'Fuzz the scheduler.'
  });
  model.authority.familiarRevisions.set('charm', 1);
  model.authority.runtimeCapabilities.set('coven-code', ['session.create']);

  const violations = [];
  let lastOp = 'seed';
  for (let step = 0; step < operations; step += 1) {
    const roll = random();
    if (roll < 0.4) {
      const minutes = 10 + Math.floor(random() * 110);
      model.setNow(new Date(model.now + minutes * 60e3).toISOString());
      applyOperation(model, { op: 'tick' });
      lastOp = 'tick';
    } else if (roll < 0.55) {
      applyOperation(model, {
        op: 'runNow',
        automationId: 'fuzz-daily',
        adoptionKey: `key-${Math.floor(random() * 3)}`
      });
      lastOp = 'runNow';
    } else if (roll < 0.65) {
      applyOperation(model, { op: 'crash', during: 'claim' });
      applyOperation(model, { op: 'tick' });
      applyOperation(model, { op: 'restart' });
      lastOp = 'crashRestart';
    } else if (roll < 0.8) {
      const available = random() < 0.5;
      model.runtimeBehavior = available ? 'available' : 'unavailable';
      if (!available) {
        model.retryPolicy = { maxAttempts: 2, backoffSeconds: 60 };
      }
      lastOp = 'flipBehavior';
    } else {
      model.authority.available = random() < 0.8;
      lastOp = 'setAuthority';
    }
    for (const violation of checkInvariants(model)) {
      violations.push({
        step,
        op: lastOp,
        invariant: violation.invariant,
        observed: violation.observed
      });
      if (violations.length >= 5) {
        return { violations, steps: step + 1, stopped: true };
      }
    }
  }
  return { violations, steps: operations, stopped: false, checkedOps: model.operationsApplied };
}
