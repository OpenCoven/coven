import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SpendLedger, currentPeriod } from '../src/ledger.ts';

function ledger(ceilingUsd = 25): SpendLedger {
  return new SpendLedger({ coven: 'test', ceilingUsd, path: null });
}

test('reserve then settle reduces remaining by the actual amount', async () => {
  const l = ledger();
  const reserved = await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 5,
    ttlSeconds: 60,
  });
  assert.equal(reserved.ok, true);
  assert.equal(l.remainingUsd(), 20);

  await l.settle({ grantId: 'g1', actualUsd: 2, inputUnits: 10, outputUnits: 20 });
  assert.equal(l.remainingUsd(), 23);
  assert.equal(l.snapshot().settledUsd, 2);
});

test('concurrent reserves never exceed the ceiling', async () => {
  const l = ledger(10);
  const results = await Promise.all(
    Array.from({ length: 50 }, (_, i) =>
      l.reserve({
        grantId: `g${i}`,
        familiar: `sub-${i}`,
        actionClass: 'model.tokens',
        provider: 'p',
        model: 'm',
        maxUsd: 1,
        ttlSeconds: 60,
      }),
    ),
  );
  const allowed = results.filter((r) => r.ok).length;
  assert.equal(allowed, 10);
  assert.ok(l.remainingUsd() >= 0);
  assert.equal(l.snapshot().reservedUsd, 10);
});

test('settle is idempotent per grant id', async () => {
  const l = ledger();
  await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 5,
    ttlSeconds: 60,
  });
  const first = await l.settle({ grantId: 'g1', actualUsd: 3, inputUnits: 1, outputUnits: 1 });
  const second = await l.settle({ grantId: 'g1', actualUsd: 3, inputUnits: 1, outputUnits: 1 });
  assert.equal(first.duplicate, false);
  assert.equal(second.duplicate, true);
  assert.equal(second.receipt.receiptId, first.receipt.receiptId);
  assert.equal(l.snapshot().settledUsd, 3);
});

test('expired reservations release exactly once', async () => {
  const l = ledger(10);
  const t0 = new Date('2026-08-20T00:00:00Z');
  await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 9,
    ttlSeconds: 60,
    now: t0,
  });
  assert.equal(l.remainingUsd(t0), 1);

  const later = new Date(t0.getTime() + 61_000);
  assert.equal(l.remainingUsd(later), 10);
  assert.equal(l.remainingUsd(later), 10);
  assert.equal(l.snapshot(later).reservations.length, 0);
});

test('a familiar sub-budget denies before the coven ceiling', async () => {
  const l = ledger(25);
  const first = await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 9,
    ttlSeconds: 60,
    subBudgetUsd: 10,
  });
  assert.equal(first.ok, true);

  const second = await l.reserve({
    grantId: 'g2',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 5,
    ttlSeconds: 60,
    subBudgetUsd: 10,
  });
  assert.equal(second.ok, false);
  if (second.ok) return;
  assert.equal(second.reason, 'sub_budget_reached');
  // Plenty of coven headroom remains — the sub-budget is what stopped it.
  assert.ok(l.remainingUsd() > 5);
});

test('a denial is recorded with a reason', async () => {
  const l = ledger(1);
  await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 5,
    ttlSeconds: 60,
  });
  const denials = l.snapshot().denials;
  assert.equal(denials.length, 1);
  assert.equal(denials[0]!.reason, 'cap_reached');
});

test('shadow mode records the denial but still issues the hold', async () => {
  const l = ledger(1);
  const result = await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 5,
    ttlSeconds: 60,
    force: true,
  });
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.forced, true);
  assert.equal(l.snapshot().denials.length, 1);
});

test('over-reservation is flagged rather than hidden', async () => {
  const l = ledger();
  await l.reserve({
    grantId: 'g1',
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'p',
    model: 'm',
    maxUsd: 1,
    ttlSeconds: 60,
  });
  const settled = await l.settle({ grantId: 'g1', actualUsd: 4, inputUnits: 1, outputUnits: 1 });
  assert.equal(settled.overReservation, true);
  assert.ok(l.snapshot().denials.some((d) => d.reason === 'over_reservation'));
});

test('the period key is UTC month', () => {
  assert.equal(currentPeriod(new Date('2026-01-31T23:59:59Z')), '2026-01');
  assert.equal(currentPeriod(new Date('2026-12-01T00:00:00Z')), '2026-12');
});
