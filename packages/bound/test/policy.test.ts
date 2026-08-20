import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign as edSign, createPrivateKey } from 'node:crypto';
import { extractBlock, canonicalBytes } from '../src/canonical.ts';
import { verifyDocument, computeEffectivePolicy } from '../src/policy.ts';

function makeKey(): { id: string; publicKey: string; privateKey: string } {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  return {
    id: 'test-key',
    publicKey: Buffer.from(publicKey.export({ type: 'spki', format: 'der' }).subarray(-32)).toString(
      'base64url',
    ),
    privateKey: Buffer.from(privateKey.export({ type: 'pkcs8', format: 'der' })).toString(
      'base64url',
    ),
  };
}

const KEY = makeKey();
const TRUSTED = { [KEY.id]: KEY.publicKey };

function doc(fields: Record<string, string | number>, role: string, keyId = KEY.id): string {
  const lines = Object.entries(fields)
    .map(([k, v]) => `${k}: ${v}`)
    .join('\n');
  const body = `# policy\n\n\`\`\`bound\n${lines}\n\`\`\`\n`;
  const parsed = extractBlock(body);
  if (!parsed.ok) throw new Error(`fixture unparsable: ${parsed.detail}`);
  const key = createPrivateKey({
    key: Buffer.from(KEY.privateKey, 'base64url'),
    format: 'der',
    type: 'pkcs8',
  });
  const sig = edSign(null, canonicalBytes(parsed.block, role), key).toString('base64url');
  return `${body}\n<!-- bound:sig v=1 alg=ed25519 key=${keyId} sig=${sig} -->\n`;
}

const GLOBAL_FIELDS = {
  version: 1,
  scope: 'coven',
  issued_at: '2026-08-20T00:00:00Z',
  not_after: '2027-08-20T00:00:00Z',
  ceiling_usd: 25.0,
  mode: 'deny-paid',
  authority: 'http://127.0.0.1:8787',
  authority_key: 'ed25519:abc',
  gateway: 'http://127.0.0.1:8788',
  grant_ttl_seconds: 120,
  clock_skew_seconds: 60,
};

test('a correctly signed document verifies', () => {
  const result = verifyDocument(doc(GLOBAL_FIELDS, 'coven'), 'coven', TRUSTED);
  assert.equal(result.ok, true);
});

test('a missing document denies', () => {
  const result = verifyDocument(null, 'coven', TRUSTED);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'policy_missing');
});

test('a tampered ceiling denies', () => {
  const tampered = doc(GLOBAL_FIELDS, 'coven').replace('ceiling_usd: 25', 'ceiling_usd: 9999');
  const result = verifyDocument(tampered, 'coven', TRUSTED);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'policy_bad_signature');
});

test('a signature for one role does not verify for another', () => {
  const familiarDoc = doc(
    { version: 1, scope: 'familiar:cody', issued_at: '2026-08-20T00:00:00Z', ceiling_usd: 10 },
    'familiar:cody',
  );
  // Same bytes, presented as the coven policy.
  const result = verifyDocument(
    familiarDoc.replace('scope: familiar:cody', 'scope: coven'),
    'coven',
    TRUSTED,
  );
  assert.equal(result.ok, false);
});

test('a document whose declared scope differs from the load role denies', () => {
  const result = verifyDocument(doc(GLOBAL_FIELDS, 'coven'), 'familiar:cody', TRUSTED);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'policy_role_mismatch');
});

test('an unsigned document denies', () => {
  const unsigned = doc(GLOBAL_FIELDS, 'coven').replace(/<!--[\s\S]*?-->/, '');
  const result = verifyDocument(unsigned, 'coven', TRUSTED);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'policy_unsigned');
});

test('an unknown key id denies', () => {
  const result = verifyDocument(doc(GLOBAL_FIELDS, 'coven', 'someone-elses-key'), 'coven', TRUSTED);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'policy_unknown_key');
});

test('an expired policy denies', () => {
  const expired = doc({ ...GLOBAL_FIELDS, not_after: '2026-01-01T00:00:00Z' }, 'coven');
  const result = verifyDocument(expired, 'coven', TRUSTED, new Date('2026-08-20T00:00:00Z'));
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'policy_expired');
});

function globalPolicy() {
  const verified = verifyDocument(doc(GLOBAL_FIELDS, 'coven'), 'coven', TRUSTED);
  assert.equal(verified.ok, true);
  if (!verified.ok) throw new Error('unreachable');
  return verified.policy;
}

test('an absent override inherits the coven ceiling', () => {
  const { policy } = computeEffectivePolicy(globalPolicy(), [], TRUSTED);
  assert.equal(policy.ceilingUsd, 25);
  assert.deepEqual(policy.subBudgets, {});
});

test('a signed override narrows the familiar', () => {
  const override = doc(
    { version: 1, scope: 'familiar:cody', issued_at: '2026-08-20T00:00:00Z', ceiling_usd: 10 },
    'familiar:cody',
  );
  const { policy, rejected } = computeEffectivePolicy(
    globalPolicy(),
    [{ familiar: 'cody', markdown: override }],
    TRUSTED,
  );
  assert.equal(policy.subBudgets.cody, 10);
  assert.deepEqual(rejected, {});
});

test('an explicit zero override means no paid actions', () => {
  const override = doc(
    { version: 1, scope: 'familiar:cody', issued_at: '2026-08-20T00:00:00Z', ceiling_usd: 0 },
    'familiar:cody',
  );
  const { policy } = computeEffectivePolicy(
    globalPolicy(),
    [{ familiar: 'cody', markdown: override }],
    TRUSTED,
  );
  assert.equal(policy.subBudgets.cody, 0);
});

test('an override above the coven ceiling is clamped, never honoured', () => {
  const override = doc(
    { version: 1, scope: 'familiar:cody', issued_at: '2026-08-20T00:00:00Z', ceiling_usd: 100000 },
    'familiar:cody',
  );
  const { policy } = computeEffectivePolicy(
    globalPolicy(),
    [{ familiar: 'cody', markdown: override }],
    TRUSTED,
  );
  assert.equal(policy.subBudgets.cody, 25);
});

test('an unsigned override yields zero, not inheritance', () => {
  const { policy, rejected } = computeEffectivePolicy(
    globalPolicy(),
    [{ familiar: 'cody', markdown: '```bound\nversion: 1\nscope: familiar:cody\nissued_at: x\n```' }],
    TRUSTED,
  );
  assert.equal(policy.subBudgets.cody, 0);
  assert.equal(rejected.cody?.reason, 'override_unsigned');
});

test('property: no override can raise a familiar above the coven ceiling', () => {
  const global = globalPolicy();
  for (const requested of [-100, 0, 0.01, 24.99, 25, 25.01, 1e9, Number.MAX_SAFE_INTEGER]) {
    const override = doc(
      {
        version: 1,
        scope: 'familiar:probe',
        issued_at: '2026-08-20T00:00:00Z',
        ceiling_usd: requested,
      },
      'familiar:probe',
    );
    const { policy } = computeEffectivePolicy(
      global,
      [{ familiar: 'probe', markdown: override }],
      TRUSTED,
    );
    assert.ok(
      policy.subBudgets.probe! <= policy.ceilingUsd,
      `requested ${requested} produced ${policy.subBudgets.probe}`,
    );
    assert.ok(policy.subBudgets.probe! >= 0);
  }
});

test('arbitrary garbage never verifies', () => {
  const inputs = ['', '?', '```bound```', '<!-- bound:sig v=1 alg=ed25519 key=k sig=z -->'];
  for (const input of inputs) {
    const result = verifyDocument(input, 'coven', TRUSTED);
    assert.equal(result.ok, false);
  }
});
