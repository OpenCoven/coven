/**
 * Demo metered provider.
 *
 * Stands in for a real vendor so the demo spends no real money. It behaves
 * like one in the way that matters: it refuses to serve without a credential,
 * and it reports usage the gateway meters.
 */

import { json, listen, readJson, type RunningService } from './http.ts';

export const DEMO_CREDENTIAL = 'sk-demo-provider-credential-not-a-real-key';

export interface ProviderRequest {
  model?: string;
  prompt?: string;
  maxOutputTokens?: number;
}

export interface ProviderResponse {
  model: string;
  output: string;
  usage: { inputUnits: number; outputUnits: number };
}

export async function startProviderStub(port = 0): Promise<RunningService> {
  return listen(
    async (req, res, url) => {
      if (req.method !== 'POST' || url.pathname !== '/v1/complete') {
        return json(res, 404, { error: 'not_found' });
      }

      const auth = req.headers.authorization ?? '';
      if (auth !== `Bearer ${DEMO_CREDENTIAL}`) {
        // This is what an agent hits when it tries to call the vendor directly.
        return json(res, 401, { error: 'missing_or_invalid_credential' });
      }

      const body = await readJson<ProviderRequest>(req);
      const model = body?.model ?? 'demo-large';
      const maxOutput = Math.max(1, body?.maxOutputTokens ?? 256);
      const inputUnits = Math.max(1, Math.ceil((body?.prompt ?? '').length / 4));

      // Deterministic usage keeps demo arithmetic legible.
      const outputUnits = model === 'demo-image' ? 1 : Math.ceil(maxOutput * 0.9);

      const response: ProviderResponse = {
        model,
        output:
          model === 'demo-image'
            ? 'data:image/png;base64,<demo-image>'
            : `demo completion for: ${(body?.prompt ?? '').slice(0, 40)}`,
        usage: { inputUnits, outputUnits },
      };
      return json(res, 200, response);
    },
    port,
    'provider-stub',
  );
}
