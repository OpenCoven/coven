/**
 * Honest familiar demo.
 *
 * Spends real (metered, fake-money) budget until Bound denies it, then proves
 * two things that matter: the denial is terminal, and free local work keeps
 * working afterwards. Run `node bin/bound.ts dev` first, or run this alone and
 * it will start its own stack.
 */

import { readdirSync } from 'node:fs';
import { BoundClient } from '../src/client.ts';
import { startStack, PACKAGE_ROOT } from '../src/stack.ts';

const money = (n: number) => `$${n.toFixed(4)}`;

function bar(used: number, ceiling: number): string {
  const width = 28;
  const filled = Math.min(width, Math.round((used / ceiling) * width));
  return `[${'#'.repeat(filled)}${'.'.repeat(width - filled)}]`;
}

async function runFamiliar(
  client: BoundClient,
  familiar: string,
  ceiling: number,
  maxOutputTokens: number,
): Promise<void> {
  console.log(`\n── ${familiar} ${'─'.repeat(Math.max(0, 46 - familiar.length))}`);
  let spent = 0;
  for (let attempt = 1; attempt <= 200; attempt += 1) {
    const outcome = await client.spend<{ output: string }>({
      familiar,
      provider: 'demo-provider',
      model: 'demo-large',
      prompt: `analysis request ${attempt} from ${familiar}`,
      maxOutputTokens,
    });

    if (!outcome.ok) {
      console.log(`  ${String(attempt).padStart(3)}  DENIED  ${outcome.reason}`);
      console.log(`       ${outcome.detail}`);
      console.log(`\n  Bound stopped ${familiar} after ${money(spent)}. No retry, no cheaper model.`);
      return;
    }

    spent += outcome.actualUsd;
    console.log(
      `  ${String(attempt).padStart(3)}  paid ${money(outcome.actualUsd)}  ${bar(spent, ceiling)}  total ${money(spent)}`,
    );
  }
  console.log('  loop limit reached before the cap — check the pricing table');
}

async function main(): Promise<void> {
  const external = process.env.BOUND_AUTHORITY_ORIGIN && process.env.BOUND_GATEWAY_ORIGIN;
  const stack = external ? null : await startStack({ statePath: null });

  const authorityOrigin = process.env.BOUND_AUTHORITY_ORIGIN ?? stack!.authority.origin;
  const gatewayOrigin = process.env.BOUND_GATEWAY_ORIGIN ?? stack!.gateway.origin;
  const client = new BoundClient({ authorityOrigin, gatewayOrigin });

  console.log('Bound demo — honest familiars spending until the signed ceiling stops them.');
  console.log(`  authority ${authorityOrigin}`);
  console.log(`  gateway   ${gatewayOrigin}   (sole holder of the provider credential)`);
  console.log(`  dashboard ${authorityOrigin}`);

  // cody carries a signed $10 sub-budget; nova has no override and so is
  // limited only by the $25 coven ceiling.
  await runFamiliar(client, 'cody', 10, 60_000);
  await runFamiliar(client, 'nova', 25, 60_000);

  console.log('\n── free work after the cap ' + '─'.repeat(23));
  const files = readdirSync(PACKAGE_ROOT).length;
  console.log(`  read ${files} entries from the package root — no grant needed, not denied.`);
  console.log('  Bound denies paid actions only. Local and free work is untouched.\n');

  const state = (await fetch(`${authorityOrigin}/v1/state`).then((r) => r.json())) as {
    ledger: { settledUsd: number; ceilingUsd: number; receipts: unknown[]; denials: unknown[] };
  };
  console.log(
    `  ledger: settled ${money(state.ledger.settledUsd)} of ${money(
      state.ledger.ceilingUsd,
    )}  ·  receipts ${state.ledger.receipts.length}  ·  denials ${state.ledger.denials.length}`,
  );

  if (stack) await stack.close();
}

await main();
