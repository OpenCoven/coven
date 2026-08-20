/**
 * The Bound Authority.
 *
 * Owns the ledger and is the only minter of grants. Every grant request
 * re-verifies the Val signature over the policy documents on disk, so
 * tampering with a policy file takes effect immediately — as a denial.
 *
 * There is no branch in this file that returns an enforceable grant when
 * verification, digest comparison, or ledger accounting fails.
 */

import { readFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { json, listen, readJson, type RunningService } from './http.ts';
import {
  computeEffectivePolicy,
  verifyDocument,
  type EffectivePolicy,
  type DenyReason,
} from './policy.ts';
import { trustedKeys, ensureKey } from './keystore.ts';
import { SpendLedger, currentPeriod, defaultLedgerPath } from './ledger.ts';
import { estimate } from './pricing.ts';
import { mintGrant } from './grant.ts';
import { renderDashboard } from './dashboard.ts';

export const COVEN_ID = 'opencoven';
export const AUTHORITY_KEY_ID = 'bound-authority-2026-08';

export interface AuthorityOptions {
  policyDir: string;
  port?: number;
  statePath?: string | null;
  familiars?: string[];
}

export interface PolicyLoad {
  ok: boolean;
  policy?: EffectivePolicy;
  rejected: Record<string, { reason: DenyReason; detail: string }>;
  reason?: DenyReason;
  detail?: string;
}

function readIfPresent(path: string): string | null {
  return existsSync(path) ? readFileSync(path, 'utf8') : null;
}

export function discoverFamiliars(policyDir: string): string[] {
  const dir = join(policyDir, 'familiars');
  if (!existsSync(dir)) return [];
  return readdirSync(dir).filter((name) => statSync(join(dir, name)).isDirectory());
}

/** Load and verify policy from disk on every call. Never cached, never trusted stale. */
export function loadPolicy(policyDir: string, familiars: string[], now = new Date()): PolicyLoad {
  const trusted = trustedKeys();
  const globalMarkdown = readIfPresent(join(policyDir, 'BOUNDS.md'));
  const verified = verifyDocument(globalMarkdown, 'coven', trusted, now);
  if (!verified.ok) {
    return { ok: false, rejected: {}, reason: verified.reason, detail: verified.detail };
  }

  const overrides = familiars.map((familiar) => ({
    familiar,
    markdown: readIfPresent(join(policyDir, 'familiars', familiar, 'BOUND.md')),
  }));

  const { policy, rejected } = computeEffectivePolicy(verified.policy, overrides, trusted, now);
  return { ok: true, policy, rejected };
}

export interface AuthorityHandle extends RunningService {
  ledger: SpendLedger;
  publicKey: string;
  reload: () => PolicyLoad;
}

interface GrantContext {
  policyDir: string;
  familiars: string[];
  ledger: SpendLedger;
  authorityKeyB64u: string;
}

export async function startAuthority(options: AuthorityOptions): Promise<AuthorityHandle> {
  const familiars = options.familiars ?? discoverFamiliars(options.policyDir);
  const initial = loadPolicy(options.policyDir, familiars);
  if (!initial.ok) {
    throw new Error(
      `refusing to start without verified policy: ${initial.reason} — ${initial.detail}`,
    );
  }

  const authorityKey = ensureKey(AUTHORITY_KEY_ID);
  const period = currentPeriod();
  const ledger = new SpendLedger({
    coven: COVEN_ID,
    ceilingUsd: initial.policy!.ceilingUsd,
    period,
    path:
      options.statePath === null
        ? null
        : (options.statePath ?? defaultLedgerPath(COVEN_ID, period)),
  });

  const ctx: GrantContext = {
    policyDir: options.policyDir,
    familiars,
    ledger,
    authorityKeyB64u: authorityKey.privateKey!,
  };

  const service = await listen(
    async (req, res, url) => {
      const path = url.pathname;

      if (req.method === 'GET' && (path === '/' || path === '/index.html')) {
        return renderDashboard(res, ledger.snapshot(), loadPolicy(ctx.policyDir, familiars));
      }

      if (req.method === 'GET' && path === '/v1/state') {
        const load = loadPolicy(ctx.policyDir, familiars);
        return json(res, 200, {
          ok: load.ok,
          policy: load.policy ?? null,
          policyError: load.ok ? null : { reason: load.reason, detail: load.detail },
          rejected: load.rejected,
          ledger: ledger.snapshot(),
        });
      }

      if (req.method === 'GET' && path === '/v1/policy') {
        const load = loadPolicy(ctx.policyDir, familiars);
        if (!load.ok) return json(res, 409, { ok: false, reason: load.reason, detail: load.detail });
        return json(res, 200, { ok: true, policy: load.policy, rejected: load.rejected });
      }

      if (req.method === 'GET' && path === '/v1/ledger') {
        return json(res, 200, { ok: true, ledger: ledger.snapshot() });
      }

      if (req.method === 'GET' && path === '/v1/receipts') {
        return json(res, 200, { ok: true, receipts: ledger.snapshot().receipts });
      }

      if (req.method === 'GET' && path === '/v1/denials') {
        return json(res, 200, { ok: true, denials: ledger.snapshot().denials });
      }

      if (req.method === 'POST' && path === '/v1/grant') return handleGrant(req, res, ctx);
      if (req.method === 'POST' && path === '/v1/settle') return handleSettle(req, res, ledger);

      return json(res, 404, { ok: false, reason: 'not_found', detail: path });
    },
    options.port ?? 8787,
    'authority',
  );

  return {
    ...service,
    ledger,
    publicKey: authorityKey.publicKey,
    reload: () => loadPolicy(ctx.policyDir, familiars),
  };
}

interface GrantRequestBody {
  familiar?: string;
  provider?: string;
  model?: string;
  inputUnits?: number;
  maxOutputUnits?: number;
  policyDigest?: string;
}

async function handleGrant(
  req: IncomingMessage,
  res: ServerResponse,
  ctx: GrantContext,
): Promise<void> {
  const body = await readJson<GrantRequestBody>(req);
  if (!body || !body.familiar || !body.provider || !body.model) {
    return json(res, 400, {
      ok: false,
      reason: 'grant_invalid',
      detail: 'familiar, provider and model are required',
    });
  }
  const familiar = body.familiar;

  // Re-verify signed policy on every grant. A tampered file denies at once.
  const load = loadPolicy(ctx.policyDir, ctx.familiars);
  if (!load.ok) {
    await ctx.ledger.recordDenial(familiar, load.reason!, load.detail!);
    return json(res, 403, { ok: false, reason: load.reason, detail: load.detail });
  }
  const policy = load.policy!;

  if (body.policyDigest && body.policyDigest !== policy.digest) {
    const detail = 'caller policy digest does not match the authority copy';
    await ctx.ledger.recordDenial(familiar, 'policy_digest_mismatch', detail);
    return json(res, 409, { ok: false, reason: 'policy_digest_mismatch', detail });
  }

  const rejection = load.rejected[familiar];
  if (rejection) {
    await ctx.ledger.recordDenial(familiar, rejection.reason, rejection.detail);
    return json(res, 403, { ok: false, reason: rejection.reason, detail: rejection.detail });
  }

  const est = estimate({
    provider: body.provider,
    model: body.model,
    inputUnits: Math.max(0, body.inputUnits ?? 0),
    maxOutputUnits: Math.max(0, body.maxOutputUnits ?? 0),
  });
  if (!est) {
    const detail = `no published price for ${body.provider}/${body.model}`;
    await ctx.ledger.recordDenial(familiar, 'grant_scope_mismatch', detail);
    return json(res, 403, { ok: false, reason: 'grant_scope_mismatch', detail });
  }

  if (!policy.metered.includes(est.actionClass)) {
    const detail = `action class ${est.actionClass} is not covered by the signed policy`;
    await ctx.ledger.recordDenial(familiar, 'grant_scope_mismatch', detail);
    return json(res, 403, { ok: false, reason: 'grant_scope_mismatch', detail });
  }

  // The grant id is chosen first so the reservation and the grant are one
  // object from the ledger's point of view. No grant exists without a hold.
  const grantId = randomUUID();
  const shadow = policy.mode === 'shadow';

  let reserved;
  try {
    reserved = await ctx.ledger.reserve({
      grantId,
      familiar,
      actionClass: est.actionClass,
      provider: body.provider,
      model: body.model,
      maxUsd: est.maxUsd,
      ttlSeconds: policy.grantTtlSeconds,
      subBudgetUsd: policy.subBudgets[familiar],
      force: shadow,
    });
  } catch (error) {
    // Accounting is the gate, not a side effect: a ledger failure denies.
    return json(res, 503, {
      ok: false,
      reason: 'ledger_unavailable',
      detail: (error as Error).message,
    });
  }

  if (!reserved.ok) {
    return json(res, 402, {
      ok: false,
      reason: reserved.reason,
      detail: reserved.detail,
      remainingUsd: reserved.remainingUsd,
    });
  }

  const grant = mintGrant(ctx.authorityKeyB64u, {
    grantId,
    familiar,
    actionClass: est.actionClass,
    provider: body.provider,
    model: body.model,
    maxUsd: est.maxUsd,
    maxOutputUnits: est.maxOutputUnits,
    ttlSeconds: policy.grantTtlSeconds,
  });

  return json(res, 200, {
    ok: true,
    grant,
    shadowed: reserved.forced === true,
    remainingUsd: reserved.remainingUsd,
    gateway: policy.gateway,
    policyDigest: policy.digest,
  });
}

async function handleSettle(
  req: IncomingMessage,
  res: ServerResponse,
  ledger: SpendLedger,
): Promise<void> {
  const body = await readJson<{
    grantId?: string;
    actualUsd?: number;
    inputUnits?: number;
    outputUnits?: number;
  }>(req);
  if (!body || !body.grantId) {
    return json(res, 400, { ok: false, reason: 'grant_invalid', detail: 'grantId is required' });
  }
  const settled = await ledger.settle({
    grantId: body.grantId,
    actualUsd: body.actualUsd ?? 0,
    inputUnits: body.inputUnits ?? 0,
    outputUnits: body.outputUnits ?? 0,
  });
  return json(res, 200, {
    ok: settled.ok,
    receipt: settled.receipt,
    duplicate: settled.duplicate,
    overReservation: settled.overReservation,
    ledger: ledger.snapshot(),
  });
}
