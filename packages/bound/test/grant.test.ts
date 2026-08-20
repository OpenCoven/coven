import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { mintGrant, verifyGrant, NonceGuard } from '../src/grant.ts';
import { estimate, actualCost } from '../src/pricing.ts';

function authorityKeys(): { publicKey: string; privateKey: string } {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  return {
    publicKey: Buffer.from(publicKey.export({ type: 'spki', format: 'der' }).subarray(-32)).toString(
      'base64url',
    ),
    privateKey: Buffer.from(privateKey.export({ type: 'pkcs8', format: 'der' })).toString(
      'base64url',
    ),
  };
}

const KEYS = authorityKeys();

function grant(overrides: Partial<Parameters<typeof mintGrant>[1]> = {}) {
  return mintGrant(KEYS.privateKey, {
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'demo-provider',
    model: 'demo-large',
    maxUsd: 1,
    maxOutputUnits: 1000,
    ttlSeconds: 120,
    ...overrides,
  });
}

const SCOPE = { provider: 'demo-provider', model: 'demo-large', requestedOutputUnits: 500 };

test('a freshly minted grant verifies', () => {
  const result = verifyGrant(grant(), KEYS.publicKey, SCOPE, {
    clockSkewSeconds: 60,
    nonces: new NonceGuard(),
  });
  assert.equal(result.ok, true);
});

test('no grant denies', () => {
  const result = verifyGrant(null, KEYS.publicKey, SCOPE, {
    clockSkewSeconds: 60,
    nonces: new NonceGuard(),
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'grant_invalid');
});

test('a tampered claim denies', () => {
  const g = grant();
  g.claims.maxUsd = 9999;
  const result = verifyGrant(g, KEYS.publicKey, SCOPE, {
    clockSkewSeconds: 60,
    nonces: new NonceGuard(),
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'grant_invalid');
});

test('a grant signed by another key denies', () => {
  const other = authorityKeys();
  const g = mintGrant(other.privateKey, {
    familiar: 'cody',
    actionClass: 'model.tokens',
    provider: 'demo-provider',
    model: 'demo-large',
    maxUsd: 9999,
    maxOutputUnits: 1e6,
    ttlSeconds: 120,
  });
  const result = verifyGrant(g, KEYS.publicKey, SCOPE, {
    clockSkewSeconds: 60,
    nonces: new NonceGuard(),
  });
  assert.equal(result.ok, false);
});

test('an expired grant denies', () => {
  const g = grant({ ttlSeconds: 1, now: new Date(Date.now() - 600_000) });
  const result = verifyGrant(g, KEYS.publicKey, SCOPE, {
    clockSkewSeconds: 60,
    nonces: new NonceGuard(),
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'grant_expired');
});

test('a grant issued far in the future denies on skew', () => {
  const g = grant({ now: new Date(Date.now() + 600_000) });
  const result = verifyGrant(g, KEYS.publicKey, SCOPE, {
    clockSkewSeconds: 60,
    nonces: new NonceGuard(),
  });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'clock_skew');
});

test('a replayed nonce denies', () => {
  const nonces = new NonceGuard();
  const g = grant();
  assert.equal(verifyGrant(g, KEYS.publicKey, SCOPE, { clockSkewSeconds: 60, nonces }).ok, true);
  const second = verifyGrant(g, KEYS.publicKey, SCOPE, { clockSkewSeconds: 60, nonces });
  assert.equal(second.ok, false);
  if (second.ok) return;
  assert.equal(second.reason, 'grant_replayed');
});

test('a different provider or model denies', () => {
  const nonces = new NonceGuard();
  const wrongModel = verifyGrant(
    grant(),
    KEYS.publicKey,
    { ...SCOPE, model: 'demo-small' },
    { clockSkewSeconds: 60, nonces },
  );
  assert.equal(wrongModel.ok, false);
  if (wrongModel.ok) return;
  assert.equal(wrongModel.reason, 'grant_scope_mismatch');
});

test('requesting more units than granted denies', () => {
  const result = verifyGrant(
    grant({ maxOutputUnits: 100 }),
    KEYS.publicKey,
    { ...SCOPE, requestedOutputUnits: 100_000 },
    { clockSkewSeconds: 60, nonces: new NonceGuard() },
  );
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'grant_scope_mismatch');
});

test('estimation is an upper bound on actual cost', () => {
  const est = estimate({
    provider: 'demo-provider',
    model: 'demo-large',
    inputUnits: 100,
    maxOutputUnits: 1000,
  });
  assert.ok(est);
  const actual = actualCost('demo-provider', 'demo-large', 100, 900);
  assert.ok(actual <= est!.maxUsd, `${actual} should not exceed ${est!.maxUsd}`);
});

test('an unpriced model produces no estimate, therefore no grant', () => {
  assert.equal(
    estimate({ provider: 'demo-provider', model: 'not-in-catalogue', inputUnits: 1, maxOutputUnits: 1 }),
    null,
  );
});
