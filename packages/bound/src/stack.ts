/** Starts the full local Bound stack on loopback: provider stub, gateway, authority. */

import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startAuthority, loadPolicy, discoverFamiliars, type AuthorityHandle } from './authority.ts';
import { startGateway } from './gateway.ts';
import { startProviderStub, DEMO_CREDENTIAL } from './provider-stub.ts';
import type { RunningService } from './http.ts';

export const PACKAGE_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

export interface Stack {
  authority: AuthorityHandle;
  gateway: RunningService;
  provider: RunningService;
  dashboardUrl: string;
  close: () => Promise<void>;
}

export interface StackOptions {
  policyDir?: string;
  authorityPort?: number;
  gatewayPort?: number;
  providerPort?: number;
  statePath?: string | null;
}

export async function startStack(options: StackOptions = {}): Promise<Stack> {
  const policyDir = options.policyDir ?? PACKAGE_ROOT;

  const provider = await startProviderStub(options.providerPort ?? 0);
  const authority = await startAuthority({
    policyDir,
    port: options.authorityPort ?? 0,
    statePath: options.statePath,
  });

  const load = loadPolicy(policyDir, discoverFamiliars(policyDir));
  if (!load.ok) throw new Error(`policy not verified: ${load.reason} — ${load.detail}`);

  // The grant trust root comes from the *signed* policy, not from the process
  // environment and not from the caller. If the running authority key does not
  // match the one Val signed, refuse to start rather than trust the running one.
  const signedAuthorityKey = load.policy!.authorityKey;
  if (signedAuthorityKey !== authority.publicKey) {
    await authority.close();
    await provider.close();
    throw new Error(
      `signed policy pins authority key ${signedAuthorityKey} but this authority holds ${authority.publicKey}`,
    );
  }

  const gateway = await startGateway({
    authorityOrigin: authority.origin,
    authorityPublicKey: signedAuthorityKey,
    providerOrigin: provider.origin,
    providerCredential: process.env.BOUND_PROVIDER_CREDENTIAL ?? DEMO_CREDENTIAL,
    clockSkewSeconds: load.policy!.clockSkewSeconds,
    port: options.gatewayPort ?? 0,
  });

  return {
    authority,
    gateway,
    provider,
    dashboardUrl: authority.origin,
    close: async () => {
      await gateway.close();
      await authority.close();
      await provider.close();
    },
  };
}

export const DEFAULT_POLICY_DIR = PACKAGE_ROOT;
export { join };
