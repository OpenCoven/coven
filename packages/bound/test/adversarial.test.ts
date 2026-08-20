import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync } from 'node:fs';
import { runAttacks } from '../demo/attacks.ts';
import { startStack, PACKAGE_ROOT } from '../src/stack.ts';
import { BoundClient } from '../src/client.ts';

/**
 * The whole point of Bound. Every one of these is a way a capable agent might
 * try to spend past the signed ceiling; every one must deny.
 */
test('every documented bypass attempt is denied', { timeout: 180_000 }, async () => {
  const results = await runAttacks();
  assert.ok(results.length >= 11, `expected the full suite, got ${results.length}`);
  for (const result of results) {
    assert.equal(
      result.denied,
      true,
      `bypass ${result.id} (${result.name}) was NOT denied: ${result.reason} — ${result.detail}`,
    );
  }
});

test('free and local work still succeeds after the cap is reached', { timeout: 120_000 }, async () => {
  const stack = await startStack({ statePath: null });
  try {
    const client = new BoundClient({
      authorityOrigin: stack.authority.origin,
      gatewayOrigin: stack.gateway.origin,
    });

    // Burn the coven ceiling with a familiar that has no sub-budget.
    let denied: string | null = null;
    for (let i = 0; i < 60 && denied === null; i += 1) {
      const outcome = await client.spend({
        familiar: 'burner',
        provider: 'demo-provider',
        model: 'demo-large',
        prompt: 'burn',
        maxOutputTokens: 60_000,
      });
      if (!outcome.ok) denied = outcome.reason;
    }
    assert.equal(denied, 'cap_reached');

    // Free work: no grant, no gateway, no denial.
    const entries = readdirSync(PACKAGE_ROOT);
    assert.ok(entries.length > 0);

    // And the ledger never went past the ceiling.
    const snapshot = stack.authority.ledger.snapshot();
    assert.ok(
      snapshot.settledUsd <= snapshot.ceilingUsd,
      `settled ${snapshot.settledUsd} exceeded ceiling ${snapshot.ceilingUsd}`,
    );
  } finally {
    await stack.close();
  }
});

test('a denial is terminal, not a retry loop', { timeout: 120_000 }, async () => {
  const stack = await startStack({ statePath: null });
  try {
    const client = new BoundClient({
      authorityOrigin: stack.authority.origin,
      gatewayOrigin: stack.gateway.origin,
    });
    // cody carries a signed 10.00 sub-budget in the repo policy.
    let denials = 0;
    for (let i = 0; i < 40; i += 1) {
      const outcome = await client.spend({
        familiar: 'cody',
        provider: 'demo-provider',
        model: 'demo-large',
        prompt: 'probe',
        maxOutputTokens: 60_000,
      });
      if (!outcome.ok) {
        denials += 1;
        assert.equal(outcome.reason, 'sub_budget_reached');
      }
    }
    assert.ok(denials > 0, 'the sub-budget never engaged');
    const snapshot = stack.authority.ledger.snapshot();
    const codySpend = snapshot.receipts
      .filter((r) => r.familiar === 'cody')
      .reduce((sum, r) => sum + r.actualUsd, 0);
    assert.ok(codySpend <= 10, `cody spent ${codySpend}, above its 10.00 sub-budget`);
  } finally {
    await stack.close();
  }
});
