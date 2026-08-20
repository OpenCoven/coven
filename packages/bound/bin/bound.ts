#!/usr/bin/env node
/**
 * bound — CLI for the Bound spend authority.
 *
 *   bound keygen [keyId]        generate a signing key in the out-of-repo keystore
 *   bound keys                  list trusted key ids
 *   bound sign <file> <role>    sign a policy document (role: coven | familiar:<id>)
 *   bound verify <file> <role>  verify a policy document
 *   bound status                print the verified effective policy
 *   bound dev [--port N]        run the full local stack and dashboard
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { extractBlock, canonicalBytes } from '../src/canonical.ts';
import { verifyDocument, computeEffectivePolicy } from '../src/policy.ts';
import { ensureKey, listKeys, signBytes, trustedKeys, keystoreDir, loadKey } from '../src/keystore.ts';
import { loadPolicy, discoverFamiliars } from '../src/authority.ts';
import { startStack, PACKAGE_ROOT } from '../src/stack.ts';

export const VAL_KEY_ID = 'val-hw-2026-08';

const SIG_LINE = /^<!--\s*bound:sig[\s\S]*?-->\s*$/m;

function die(message: string): never {
  console.error(`bound: ${message}`);
  process.exit(1);
}

function cmdKeygen(keyId: string): void {
  const record = ensureKey(keyId);
  console.log(`key ${record.keyId}`);
  console.log(`  public   ${record.publicKey}`);
  console.log(`  keystore ${keystoreDir()}`);
}

function cmdKeys(): void {
  const keys = listKeys();
  if (keys.length === 0) return console.log('no keys; run `bound keygen`');
  for (const key of keys) console.log(`${key.keyId}\t${key.publicKey}\t${key.createdAt}`);
}

function cmdSign(file: string, role: string, keyId: string): void {
  const path = resolve(file);
  const original = readFileSync(path, 'utf8');
  const parsed = extractBlock(original);
  if (!parsed.ok) die(`cannot sign: ${parsed.reason} — ${parsed.detail}`);

  const declared = String(parsed.block.scope);
  if (declared !== role) die(`document declares scope "${declared}" but role "${role}" was given`);

  const key = loadKey(keyId);
  if (!key) die(`no key ${keyId} in ${keystoreDir()} — run \`bound keygen ${keyId}\``);

  const sig = signBytes(key, canonicalBytes(parsed.block, role));
  const trailer = `<!-- bound:sig v=1 alg=ed25519 key=${keyId} sig=${sig} -->`;
  const body = original.replace(SIG_LINE, '').trimEnd();
  writeFileSync(path, `${body}\n\n${trailer}\n`);
  console.log(`signed ${path} as role ${role} with ${keyId}`);
}

function cmdVerify(file: string, role: string): void {
  const path = resolve(file);
  const result = verifyDocument(readFileSync(path, 'utf8'), role, trustedKeys());
  if (!result.ok) {
    console.error(`INVALID  ${path}\n  ${result.reason}: ${result.detail}`);
    process.exit(2);
  }
  console.log(`VALID    ${path}\n  role ${role}  digest ${result.digest}`);
}

function cmdStatus(policyDir: string): void {
  const familiars = discoverFamiliars(policyDir);
  const load = loadPolicy(policyDir, familiars);
  if (!load.ok) {
    console.error(`policy NOT verified: ${load.reason} — ${load.detail}`);
    console.error('all paid actions would be denied.');
    process.exit(2);
  }
  const p = load.policy!;
  console.log(`policy verified`);
  console.log(`  ceiling      $${p.ceilingUsd.toFixed(2)} (${p.period})`);
  console.log(`  mode         ${p.mode}`);
  console.log(`  grant TTL    ${p.grantTtlSeconds}s   skew ${p.clockSkewSeconds}s`);
  console.log(`  metered      ${p.metered.join(', ')}`);
  console.log(`  digest       ${p.digest}`);
  for (const [familiar, cap] of Object.entries(p.subBudgets)) {
    const bad = load.rejected[familiar];
    console.log(`  ${familiar.padEnd(12)} $${cap.toFixed(2)}${bad ? `  REJECTED: ${bad.detail}` : ''}`);
  }
}

async function cmdDev(port: number): Promise<void> {
  // Bind the gateway to the port the signed policy advertises, so the URL a
  // familiar reads in BOUNDS.md is the URL that actually enforces.
  const stack = await startStack({
    authorityPort: port,
    gatewayPort: Number(process.env.BOUND_GATEWAY_PORT ?? 8788),
    providerPort: Number(process.env.BOUND_PROVIDER_PORT ?? 8789),
  });
  const load = loadPolicy(PACKAGE_ROOT, discoverFamiliars(PACKAGE_ROOT));
  console.log('');
  console.log('  Bound is running.');
  console.log('');
  console.log(`  dashboard   ${stack.dashboardUrl}`);
  console.log(`  authority   ${stack.authority.origin}`);
  console.log(`  gateway     ${stack.gateway.origin}   (holds the provider credential)`);
  console.log(`  provider    ${stack.provider.origin}   (demo stub, spends no real money)`);
  console.log('');
  console.log(`  ceiling     $${load.policy!.ceilingUsd.toFixed(2)} / ${load.policy!.period}`);
  console.log(`  mode        ${load.policy!.mode}`);
  console.log('');
  console.log('  try:  node demo/agent.ts     honest familiars spending to the cap');
  console.log('        node demo/attacks.ts   eleven bypass attempts, all denied');
  console.log('');
  console.log('  tamper test:');
  console.log("        sed -i '' 's/ceiling_usd: 25.00/ceiling_usd: 9999.00/' BOUNDS.md");
  console.log('        node bin/bound.ts status     # policy NOT verified, everything denies');
  console.log('        git checkout BOUNDS.md');
  console.log('');
  console.log('  Ctrl-C to stop.');

  const shutdown = async () => {
    await stack.close();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

async function main(): Promise<void> {
  const [command, ...args] = process.argv.slice(2);
  switch (command) {
    case 'keygen':
      return cmdKeygen(args[0] ?? VAL_KEY_ID);
    case 'keys':
      return cmdKeys();
    case 'sign': {
      const file = args[0] ?? die('usage: bound sign <file> <role> [keyId]');
      const role = args[1] ?? die('usage: bound sign <file> <role> [keyId]');
      return cmdSign(file, role, args[2] ?? VAL_KEY_ID);
    }
    case 'verify': {
      const file = args[0] ?? die('usage: bound verify <file> <role>');
      const role = args[1] ?? die('usage: bound verify <file> <role>');
      return cmdVerify(file, role);
    }
    case 'status':
      return cmdStatus(args[0] ? resolve(args[0]) : PACKAGE_ROOT);
    case 'dev':
      return cmdDev(Number(process.env.BOUND_PORT ?? args[0] ?? 8787));
    default:
      console.log(readFileSync(new URL(import.meta.url), 'utf8').split('\n').slice(2, 12).join('\n'));
      process.exit(command ? 1 : 0);
  }
}

await main();

export { computeEffectivePolicy };
