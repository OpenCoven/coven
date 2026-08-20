import { test } from 'node:test';
import assert from 'node:assert/strict';
import { extractBlock, canonicalJson, canonicalBytes } from '../src/canonical.ts';

const VALID = `# BOUNDS.md

Prose here.

\`\`\`bound
version: 1
scope: coven
issued_at: 2026-08-20T00:00:00Z
ceiling_usd: 25.00
mode: deny-paid
metered:
  - model.tokens
  - model.images
\`\`\`

More prose.
`;

test('parses a well-formed bound block', () => {
  const result = extractBlock(VALID);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.block.version, 1);
  assert.equal(result.block.scope, 'coven');
  assert.equal(result.block.ceiling_usd, 25);
  assert.equal(result.block.mode, 'deny-paid');
  assert.deepEqual(result.block.metered, ['model.tokens', 'model.images']);
});

test('strips trailing comments but keeps values intact', () => {
  const result = extractBlock(VALID.replace('mode: deny-paid', 'mode: deny-paid   # only Val'));
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.block.mode, 'deny-paid');
});

test('editing prose does not change canonical bytes', () => {
  const a = extractBlock(VALID);
  const b = extractBlock(VALID.replace('More prose.', 'Completely different prose.'));
  assert.equal(a.ok && b.ok, true);
  if (!a.ok || !b.ok) return;
  assert.equal(
    canonicalBytes(a.block, 'coven').toString('hex'),
    canonicalBytes(b.block, 'coven').toString('hex'),
  );
});

test('canonical json is key-order independent', () => {
  const one = canonicalJson({ b: 2, a: 1 });
  const two = canonicalJson({ a: 1, b: 2 });
  assert.equal(one, two);
});

test('canonical bytes bind the role', () => {
  const parsed = extractBlock(VALID);
  assert.equal(parsed.ok, true);
  if (!parsed.ok) return;
  assert.notEqual(
    canonicalBytes(parsed.block, 'coven').toString('hex'),
    canonicalBytes(parsed.block, 'familiar:cody').toString('hex'),
  );
});

test('rejects a document with no bound block', () => {
  const result = extractBlock('# just markdown\n');
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'no_block');
});

test('rejects two bound blocks', () => {
  const result = extractBlock(`${VALID}\n${VALID}`);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'multiple_blocks');
});

test('rejects duplicate keys', () => {
  const result = extractBlock(VALID.replace('mode: deny-paid', 'scope: familiar:cody'));
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'duplicate_key');
});

test('rejects tabs and unparsable lines', () => {
  assert.equal(extractBlock(VALID.replace('mode: deny-paid', '\tmode: x')).ok, false);
  assert.equal(extractBlock(VALID.replace('mode: deny-paid', 'not a pair')).ok, false);
});

test('rejects an unsupported version', () => {
  const result = extractBlock(VALID.replace('version: 1', 'version: 2'));
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'bad_version');
});

test('rejects a block missing a required field', () => {
  const result = extractBlock(VALID.replace('issued_at: 2026-08-20T00:00:00Z\n', ''));
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.reason, 'missing_field');
});

test('never throws on arbitrary input', () => {
  const inputs = ['', '```bound\n```', '```bound\n\u0000\n```', '`'.repeat(500), VALID.slice(0, 40)];
  for (const input of inputs) {
    assert.doesNotThrow(() => extractBlock(input));
  }
});
