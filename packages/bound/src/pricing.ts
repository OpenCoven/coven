/**
 * Pricing and bounded cost estimation.
 *
 * Bound reserves the *maximum* an action can cost before it runs, then settles
 * the actual. Estimation must never understate: the gateway clamps provider
 * limits to the grant, so an honest estimator plus clamping makes overspend
 * structurally impossible rather than merely unlikely.
 */

export type ActionClass =
  | 'model.tokens'
  | 'model.images'
  | 'model.audio'
  | 'tools.hosted'
  | 'search.api'
  | 'compute.render'
  | 'storage.egress'
  | 'infra.paid';

export interface UnitPrice {
  actionClass: ActionClass;
  /** USD per unit; unit meaning depends on the class. */
  inputUsdPerUnit: number;
  outputUsdPerUnit: number;
  unit: string;
}

/** Demo catalogue. Real deployments load this from the gateway's provider adapters. */
export const PRICES: Record<string, UnitPrice> = {
  'demo-provider/demo-large': {
    actionClass: 'model.tokens',
    inputUsdPerUnit: 0.000003,
    outputUsdPerUnit: 0.000015,
    unit: 'token',
  },
  'demo-provider/demo-small': {
    actionClass: 'model.tokens',
    inputUsdPerUnit: 0.0000005,
    outputUsdPerUnit: 0.0000015,
    unit: 'token',
  },
  'demo-provider/demo-image': {
    actionClass: 'model.images',
    inputUsdPerUnit: 0,
    outputUsdPerUnit: 0.04,
    unit: 'image',
  },
};

export function priceKey(provider: string, model: string): string {
  return `${provider}/${model}`;
}

export function lookupPrice(provider: string, model: string): UnitPrice | null {
  return PRICES[priceKey(provider, model)] ?? null;
}

export interface EstimateInput {
  provider: string;
  model: string;
  inputUnits: number;
  maxOutputUnits: number;
}

export interface Estimate {
  actionClass: ActionClass;
  maxUsd: number;
  maxOutputUnits: number;
}

export function estimate(input: EstimateInput): Estimate | null {
  const price = lookupPrice(input.provider, input.model);
  if (!price) return null;
  const maxUsd =
    input.inputUnits * price.inputUsdPerUnit + input.maxOutputUnits * price.outputUsdPerUnit;
  return {
    actionClass: price.actionClass,
    maxUsd: round6(maxUsd),
    maxOutputUnits: input.maxOutputUnits,
  };
}

export function actualCost(
  provider: string,
  model: string,
  inputUnits: number,
  outputUnits: number,
): number {
  const price = lookupPrice(provider, model);
  if (!price) return 0;
  return round6(inputUnits * price.inputUsdPerUnit + outputUnits * price.outputUsdPerUnit);
}

export function round6(value: number): number {
  return Math.round(value * 1e6) / 1e6;
}
