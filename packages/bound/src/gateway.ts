/**
 * The Bound Gateway.
 *
 * This is the only process that holds a provider credential, and it will not
 * use it without a valid grant. That single property is what makes the cap a
 * cap: an agent cannot exceed a budget it has no way to spend against.
 *
 * The gateway also clamps requested provider limits down to the grant, so
 * actual cost cannot exceed the reservation the authority already took.
 */

import type { IncomingMessage, ServerResponse } from 'node:http';
import { json, listen, postJson, readJson, type RunningService } from './http.ts';
import { NonceGuard, verifyGrant, type SignedGrant } from './grant.ts';
import { actualCost } from './pricing.ts';

export interface GatewayOptions {
  /** Authority origin, taken from the signed policy — never from agent input. */
  authorityOrigin: string;
  /** Authority grant-verification public key, taken from the signed policy. */
  authorityPublicKey: string;
  providerOrigin: string;
  providerCredential: string;
  clockSkewSeconds?: number;
  port?: number;
}

export interface ProxyRequestBody {
  grant?: SignedGrant;
  provider?: string;
  model?: string;
  prompt?: string;
  maxOutputTokens?: number;
}

/** Shape returned by a metered provider. Never trusted for accounting beyond usage. */
interface ProviderResult {
  model: string;
  output: string;
  usage: { inputUnits: number; outputUnits: number };
}

export async function startGateway(options: GatewayOptions): Promise<RunningService> {
  const nonces = new NonceGuard();
  const clockSkewSeconds = options.clockSkewSeconds ?? 60;

  return listen(
    async (req, res, url) => {
      if (req.method === 'GET' && url.pathname === '/v1/health') {
        return json(res, 200, { ok: true, service: 'bound-gateway' });
      }
      if (req.method !== 'POST' || url.pathname !== '/v1/proxy') {
        return json(res, 404, { ok: false, reason: 'not_found', detail: url.pathname });
      }
      return handleProxy(req, res, options, nonces, clockSkewSeconds);
    },
    options.port ?? 8788,
    'gateway',
  );
}

async function handleProxy(
  req: IncomingMessage,
  res: ServerResponse,
  options: GatewayOptions,
  nonces: NonceGuard,
  clockSkewSeconds: number,
): Promise<void> {
  const body = await readJson<ProxyRequestBody>(req);
  if (!body || !body.provider || !body.model) {
    return json(res, 400, {
      ok: false,
      reason: 'grant_invalid',
      detail: 'provider and model are required',
    });
  }

  const requestedOutputUnits = Math.max(1, body.maxOutputTokens ?? 256);

  const verified = verifyGrant(
    body.grant,
    options.authorityPublicKey,
    { provider: body.provider, model: body.model, requestedOutputUnits },
    { clockSkewSeconds, nonces },
  );
  if (!verified.ok) {
    return json(res, 403, { ok: false, reason: verified.reason, detail: verified.detail });
  }
  const claims = verified.claims;

  // Clamp provider-side limits to the grant. Even a lying client cannot make
  // the provider produce more units than were reserved.
  const clampedOutput = Math.min(requestedOutputUnits, claims.maxOutputUnits);

  let providerResponse: ProviderResult | null = null;
  let providerError: string | null = null;

  try {
    const response = await fetch(`${options.providerOrigin}/v1/complete`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        // The credential lives here and nowhere an agent can read.
        authorization: `Bearer ${options.providerCredential}`,
      },
      body: JSON.stringify({
        model: body.model,
        prompt: body.prompt ?? '',
        maxOutputTokens: clampedOutput,
      }),
      signal: AbortSignal.timeout(10_000),
    });
    if (!response.ok) {
      providerError = `provider returned ${response.status}`;
    } else {
      providerResponse = (await response.json()) as ProviderResult;
    }
  } catch (error) {
    providerError = (error as Error).message;
  }

  const usage = providerResponse?.usage ?? { inputUnits: 0, outputUnits: 0 };
  // Settle even on provider error so the reservation is released rather than
  // silently held until expiry.
  const cost = actualCost(body.provider, body.model, usage.inputUnits, usage.outputUnits);

  const settle = await postJson<{ ok: boolean; receipt?: unknown }>(
    `${options.authorityOrigin}/v1/settle`,
    {
      grantId: claims.grantId,
      actualUsd: cost,
      inputUnits: usage.inputUnits,
      outputUnits: usage.outputUnits,
    },
  );

  if (!settle.ok) {
    // Fail closed: if spend cannot be recorded, do not hand back the result.
    return json(res, 503, {
      ok: false,
      reason: 'authority_unavailable',
      detail: settle.error ?? `settle failed with status ${settle.status}`,
    });
  }

  if (providerError) {
    return json(res, 502, { ok: false, reason: 'provider_error', detail: providerError });
  }

  return json(res, 200, {
    ok: true,
    grantId: claims.grantId,
    result: providerResponse,
    actualUsd: cost,
    clampedOutputUnits: clampedOutput,
    receipt: settle.body,
  });
}

export type { RunningService };
