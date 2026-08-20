/**
 * Grants — short-lived, scoped, ed25519-signed spend authorizations.
 *
 * Only the authority holds the grant signing key, so an agent can no more mint
 * a grant than it can mint a policy signature. The gateway verifies the grant
 * against the authority public key carried in the *signed* policy, which is
 * why an agent cannot repoint the trust root with an environment variable.
 */

import { randomUUID, createPrivateKey, sign as edSign, verify as edVerify, createPublicKey } from 'node:crypto';
import type { DenyReason } from './policy.ts';

export interface GrantClaims {
  grantId: string;
  familiar: string;
  actionClass: string;
  provider: string;
  model: string;
  maxUsd: number;
  maxOutputUnits: number;
  nonce: string;
  iat: number; // epoch seconds
  exp: number; // epoch seconds
}

export interface SignedGrant {
  claims: GrantClaims;
  sig: string; // base64url over canonical claims
}

export function canonicalClaims(claims: GrantClaims): Buffer {
  const keys = Object.keys(claims).sort() as (keyof GrantClaims)[];
  const body = keys.map((k) => `${JSON.stringify(k)}:${JSON.stringify(claims[k])}`).join(',');
  return Buffer.from(`bound-grant:v1{${body}}`, 'utf8');
}

export function mintGrant(
  privateKeyB64u: string,
  input: Omit<GrantClaims, 'grantId' | 'nonce' | 'iat' | 'exp'> & {
    ttlSeconds: number;
    grantId?: string;
    now?: Date;
  },
): SignedGrant {
  const now = input.now ?? new Date();
  const iat = Math.floor(now.getTime() / 1000);
  const claims: GrantClaims = {
    grantId: input.grantId ?? randomUUID(),
    familiar: input.familiar,
    actionClass: input.actionClass,
    provider: input.provider,
    model: input.model,
    maxUsd: input.maxUsd,
    maxOutputUnits: input.maxOutputUnits,
    nonce: randomUUID(),
    iat,
    exp: iat + input.ttlSeconds,
  };
  const key = createPrivateKey({
    key: Buffer.from(privateKeyB64u, 'base64url'),
    format: 'der',
    type: 'pkcs8',
  });
  return { claims, sig: edSign(null, canonicalClaims(claims), key).toString('base64url') };
}

function publicKeyFromRaw(base64url: string) {
  const raw = Buffer.from(base64url, 'base64url');
  if (raw.length !== 32) throw new Error('ed25519 public key must be 32 bytes');
  return createPublicKey({
    key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), raw]),
    format: 'der',
    type: 'spki',
  });
}

export interface GrantRequestScope {
  provider: string;
  model: string;
  requestedOutputUnits: number;
}

export type GrantVerifyResult =
  | { ok: true; claims: GrantClaims }
  | { ok: false; reason: DenyReason; detail: string };

export class NonceGuard {
  #seen = new Map<string, number>();

  /** Returns false when the nonce has been used before. */
  admit(nonce: string, expEpochSeconds: number, now: Date = new Date()): boolean {
    const nowSec = Math.floor(now.getTime() / 1000);
    for (const [key, exp] of this.#seen) if (exp <= nowSec) this.#seen.delete(key);
    if (this.#seen.has(nonce)) return false;
    this.#seen.set(nonce, expEpochSeconds);
    return true;
  }
}

export function verifyGrant(
  grant: SignedGrant | null | undefined,
  authorityPublicKeyB64u: string,
  scope: GrantRequestScope,
  options: { clockSkewSeconds: number; nonces: NonceGuard; now?: Date },
): GrantVerifyResult {
  if (!grant || !grant.claims || typeof grant.sig !== 'string') {
    return { ok: false, reason: 'grant_invalid', detail: 'no grant presented' };
  }

  const now = options.now ?? new Date();
  const nowSec = Math.floor(now.getTime() / 1000);

  let good = false;
  try {
    good = edVerify(
      null,
      canonicalClaims(grant.claims),
      publicKeyFromRaw(authorityPublicKeyB64u),
      Buffer.from(grant.sig, 'base64url'),
    );
  } catch (error) {
    return { ok: false, reason: 'grant_invalid', detail: (error as Error).message };
  }
  if (!good) return { ok: false, reason: 'grant_invalid', detail: 'grant signature does not verify' };

  if (grant.claims.iat > nowSec + options.clockSkewSeconds) {
    return { ok: false, reason: 'clock_skew', detail: 'grant issued in the future' };
  }
  if (grant.claims.exp + options.clockSkewSeconds <= nowSec) {
    return { ok: false, reason: 'grant_expired', detail: `grant expired at ${grant.claims.exp}` };
  }

  if (grant.claims.provider !== scope.provider || grant.claims.model !== scope.model) {
    return {
      ok: false,
      reason: 'grant_scope_mismatch',
      detail: `grant covers ${grant.claims.provider}/${grant.claims.model}, request targets ${scope.provider}/${scope.model}`,
    };
  }
  if (scope.requestedOutputUnits > grant.claims.maxOutputUnits) {
    return {
      ok: false,
      reason: 'grant_scope_mismatch',
      detail: `requested ${scope.requestedOutputUnits} units exceeds grant limit ${grant.claims.maxOutputUnits}`,
    };
  }

  if (!options.nonces.admit(grant.claims.nonce, grant.claims.exp, now)) {
    return { ok: false, reason: 'grant_replayed', detail: 'grant nonce already used' };
  }

  return { ok: true, claims: grant.claims };
}
