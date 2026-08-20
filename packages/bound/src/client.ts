/**
 * BoundClient — the only Bound surface an agent links against.
 *
 * A denial is terminal. `spend()` never retries, never falls back to a cheaper
 * model, and never degrades open: it returns a structured BoundDenied that the
 * agent reports and moves on from. Free and local work is unaffected because
 * it never comes through here at all.
 */

import { postJson } from './http.ts';
import type { SignedGrant } from './grant.ts';

export interface BoundDenied {
  ok: false;
  reason: string;
  detail: string;
  remainingUsd?: number;
}

export interface BoundAllowed<T> {
  ok: true;
  result: T;
  actualUsd: number;
  grantId: string;
  remainingUsd: number;
}

export interface SpendInput {
  familiar: string;
  provider: string;
  model: string;
  prompt: string;
  maxOutputTokens: number;
  /** Optional: prove the caller is looking at the same policy the authority is. */
  policyDigest?: string;
}

export interface BoundClientOptions {
  authorityOrigin: string;
  gatewayOrigin: string;
}

export class BoundClient {
  #authority: string;
  #gateway: string;

  constructor(options: BoundClientOptions) {
    this.#authority = options.authorityOrigin.replace(/\/$/, '');
    this.#gateway = options.gatewayOrigin.replace(/\/$/, '');
  }

  async requestGrant(input: SpendInput): Promise<
    | { ok: true; grant: SignedGrant; remainingUsd: number; policyDigest: string }
    | BoundDenied
  > {
    const inputUnits = Math.max(1, Math.ceil(input.prompt.length / 4));
    const response = await postJson<{
      ok: boolean;
      grant?: SignedGrant;
      reason?: string;
      detail?: string;
      remainingUsd?: number;
      policyDigest?: string;
    }>(`${this.#authority}/v1/grant`, {
      familiar: input.familiar,
      provider: input.provider,
      model: input.model,
      inputUnits,
      maxOutputUnits: input.maxOutputTokens,
      policyDigest: input.policyDigest,
    });

    if (!response.body) {
      return {
        ok: false,
        reason: 'authority_unavailable',
        detail: response.error ?? `authority returned status ${response.status}`,
      };
    }
    if (!response.body.ok || !response.body.grant) {
      return {
        ok: false,
        reason: response.body.reason ?? 'authority_unavailable',
        detail: response.body.detail ?? 'grant refused',
        remainingUsd: response.body.remainingUsd,
      };
    }
    return {
      ok: true,
      grant: response.body.grant,
      remainingUsd: response.body.remainingUsd ?? 0,
      policyDigest: response.body.policyDigest ?? '',
    };
  }

  async spend<T = unknown>(input: SpendInput): Promise<BoundAllowed<T> | BoundDenied> {
    const granted = await this.requestGrant(input);
    if (!granted.ok) return granted;

    const response = await postJson<{
      ok: boolean;
      reason?: string;
      detail?: string;
      result?: T;
      actualUsd?: number;
      grantId?: string;
    }>(`${this.#gateway}/v1/proxy`, {
      grant: granted.grant,
      provider: input.provider,
      model: input.model,
      prompt: input.prompt,
      maxOutputTokens: input.maxOutputTokens,
    });

    if (!response.body) {
      return {
        ok: false,
        reason: 'gateway_unavailable',
        detail: response.error ?? `gateway returned status ${response.status}`,
      };
    }
    if (!response.body.ok) {
      return {
        ok: false,
        reason: response.body.reason ?? 'grant_invalid',
        detail: response.body.detail ?? 'gateway refused',
      };
    }

    return {
      ok: true,
      result: response.body.result as T,
      actualUsd: response.body.actualUsd ?? 0,
      grantId: response.body.grantId ?? granted.grant.claims.grantId,
      remainingUsd: granted.remainingUsd,
    };
  }
}
