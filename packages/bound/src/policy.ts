/**
 * Policy documents: signature verification and effective-policy computation.
 *
 * This module performs no I/O. It takes bytes and trusted keys, and returns
 * results. Every failure path resolves to "no authority", never to a default.
 */

import { createPublicKey, verify as edVerify, createHash } from 'node:crypto';
import { canonicalBytes, canonicalJson, extractBlock, type BoundBlock } from './canonical.ts';

export type TrustedKeys = Record<string, string>; // keyId -> base64url raw ed25519 public key

export type DenyReason =
  | 'policy_missing'
  | 'policy_unparsable'
  | 'policy_unsigned'
  | 'policy_bad_signature'
  | 'policy_unknown_key'
  | 'policy_expired'
  | 'policy_role_mismatch'
  | 'policy_digest_mismatch'
  | 'override_unsigned'
  | 'cap_reached'
  | 'sub_budget_reached'
  | 'ledger_unavailable'
  | 'authority_unavailable'
  | 'grant_invalid'
  | 'grant_expired'
  | 'grant_replayed'
  | 'grant_scope_mismatch'
  | 'clock_skew'
  | 'no_credential'
  | 'mode_denies';

export interface SignatureTrailer {
  version: string;
  alg: string;
  keyId: string;
  sig: string;
}

export interface PolicyDocument {
  block: BoundBlock;
  signature: SignatureTrailer | null;
  role: string;
}

export type VerifyResult =
  | { ok: true; policy: PolicyDocument; digest: string }
  | { ok: false; reason: DenyReason; detail: string };

const SIG_RE = /<!--\s*bound:sig\s+v=(\S+)\s+alg=(\S+)\s+key=(\S+)\s+sig=(\S+)\s*-->/;

export function parseSignature(markdown: string): SignatureTrailer | null {
  const m = SIG_RE.exec(markdown);
  if (!m) return null;
  return { version: m[1]!, alg: m[2]!, keyId: m[3]!, sig: m[4]! };
}

function rawEd25519PublicKey(base64url: string) {
  const raw = Buffer.from(base64url, 'base64url');
  if (raw.length !== 32) throw new Error('ed25519 public key must be 32 bytes');
  // SPKI prefix for Ed25519.
  const spki = Buffer.concat([
    Buffer.from('302a300506032b6570032100', 'hex'),
    raw,
  ]);
  return createPublicKey({ key: spki, format: 'der', type: 'spki' });
}

export function policyDigest(block: BoundBlock, role: string): string {
  return createHash('sha256').update(canonicalBytes(block, role)).digest('base64url');
}

/**
 * Verify a policy document for a declared role.
 *
 * `now` is injected so tests and the authority share one clock discipline.
 */
export function verifyDocument(
  markdown: string | null | undefined,
  role: string,
  trusted: TrustedKeys,
  now: Date = new Date(),
): VerifyResult {
  if (markdown === null || markdown === undefined || markdown.trim() === '') {
    return { ok: false, reason: 'policy_missing', detail: `no document for role ${role}` };
  }

  const parsed = extractBlock(markdown);
  if (!parsed.ok) {
    return { ok: false, reason: 'policy_unparsable', detail: `${parsed.reason}: ${parsed.detail}` };
  }

  const declaredScope = String(parsed.block.scope);
  if (declaredScope !== role) {
    return {
      ok: false,
      reason: 'policy_role_mismatch',
      detail: `document declares scope ${declaredScope}, loaded as ${role}`,
    };
  }

  const signature = parseSignature(markdown);
  if (!signature) {
    return { ok: false, reason: 'policy_unsigned', detail: `no signature trailer for ${role}` };
  }
  if (signature.alg !== 'ed25519' || signature.version !== '1') {
    return {
      ok: false,
      reason: 'policy_bad_signature',
      detail: `unsupported signature v=${signature.version} alg=${signature.alg}`,
    };
  }

  const publicKey = trusted[signature.keyId];
  if (!publicKey) {
    return { ok: false, reason: 'policy_unknown_key', detail: `untrusted key id ${signature.keyId}` };
  }

  let good = false;
  try {
    good = edVerify(
      null,
      canonicalBytes(parsed.block, role),
      rawEd25519PublicKey(publicKey),
      Buffer.from(signature.sig, 'base64url'),
    );
  } catch (error) {
    return {
      ok: false,
      reason: 'policy_bad_signature',
      detail: `verification error: ${(error as Error).message}`,
    };
  }
  if (!good) {
    return { ok: false, reason: 'policy_bad_signature', detail: `signature does not cover ${role}` };
  }

  const notAfter = parsed.block.not_after;
  if (typeof notAfter === 'string' && notAfter !== '') {
    const expiry = Date.parse(notAfter);
    if (Number.isNaN(expiry)) {
      return { ok: false, reason: 'policy_unparsable', detail: `bad not_after: ${notAfter}` };
    }
    if (now.getTime() >= expiry) {
      return { ok: false, reason: 'policy_expired', detail: `policy expired at ${notAfter}` };
    }
  }

  return {
    ok: true,
    policy: { block: parsed.block, signature, role },
    digest: policyDigest(parsed.block, role),
  };
}

export interface EffectivePolicy {
  period: string;
  ceilingUsd: number;
  mode: 'deny-paid' | 'shadow';
  metered: string[];
  authority: string;
  authorityKey: string;
  gateway: string;
  grantTtlSeconds: number;
  clockSkewSeconds: number;
  subBudgets: Record<string, number>;
  digest: string;
}

export interface OverrideInput {
  familiar: string;
  markdown: string | null;
}

export interface EffectiveResult {
  policy: EffectivePolicy;
  /** Familiars whose override failed verification. These get zero paid budget. */
  rejected: Record<string, { reason: DenyReason; detail: string }>;
}

function num(block: BoundBlock, key: string, fallback: number): number {
  const value = block[key];
  return typeof value === 'number' ? value : fallback;
}

function str(block: BoundBlock, key: string, fallback: string): string {
  const value = block[key];
  return typeof value === 'string' && value !== '' ? value : fallback;
}

/**
 * Global plus overrides.
 *
 * The global ceiling is a hard clamp applied after merge: no signed override
 * can raise a familiar above the coven ceiling. An override that fails
 * verification yields zero budget rather than silently inheriting the global.
 */
export function computeEffectivePolicy(
  globalDoc: PolicyDocument,
  overrides: OverrideInput[],
  trusted: TrustedKeys,
  now: Date = new Date(),
): EffectiveResult {
  const g = globalDoc.block;
  const ceilingUsd = num(g, 'ceiling_usd', 0);
  const modeRaw = str(g, 'mode', 'deny-paid');

  const policy: EffectivePolicy = {
    period: str(g, 'period', 'monthly-utc'),
    ceilingUsd,
    mode: modeRaw === 'shadow' ? 'shadow' : 'deny-paid',
    metered: Array.isArray(g.metered) ? (g.metered as string[]) : [],
    authority: str(g, 'authority', ''),
    authorityKey: str(g, 'authority_key', '').replace(/^ed25519:/, ''),
    gateway: str(g, 'gateway', ''),
    grantTtlSeconds: num(g, 'grant_ttl_seconds', 120),
    clockSkewSeconds: num(g, 'clock_skew_seconds', 60),
    subBudgets: {},
    digest: policyDigest(g, globalDoc.role),
  };

  const rejected: EffectiveResult['rejected'] = {};

  for (const override of overrides) {
    const role = `familiar:${override.familiar}`;
    const verified = verifyDocument(override.markdown, role, trusted, now);
    if (!verified.ok) {
      // An unverifiable override is not "ignore and inherit" — it is zero.
      rejected[override.familiar] = { reason: 'override_unsigned', detail: verified.detail };
      policy.subBudgets[override.familiar] = 0;
      continue;
    }
    const requested = num(verified.policy.block, 'ceiling_usd', 0);
    policy.subBudgets[override.familiar] = Math.max(0, Math.min(requested, ceilingUsd));
  }

  return { policy, rejected };
}

export { canonicalJson };
