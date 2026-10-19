// Review guard for bounded doctor metadata forms. This grants no compatibility
// or release authority; capture changes require source-owner review.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { digestCanonical } from '../conformance/automations/runner/conformance.mjs';

const root = new URL('../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root));
const json = (path) => JSON.parse(read(path));
const base = 'crates/coven-cli/src/automations/';
const manifest = json(`${base}doctor-schema-forms.json`);
const review = json(`${base}doctor-schema-form-review.json`);
assert.equal(review.disposition, 'review_only');
assert.equal(review.metadataRecipe, manifest.metadataRecipe);
assert.equal(review.captureSha256, manifest.captureSha256);
assert.deepEqual(Object.keys(review.fixtureObjects).sort(), [...manifest.fixtureForms].sort());
const actualForms = {};
const actualIndexes = {};
const used = new Set();
for (const objects of Object.values(review.fixtureObjects)) {
  assert.deepEqual(Object.keys(objects).sort(), Object.keys(manifest.objects).sort());
  for (const [name, binding] of Object.entries(objects)) {
    assert.deepEqual(Object.keys(binding), ['sha256']);
    const digest = binding.sha256;
    const descriptor = review.descriptors[digest];
    assert.ok(descriptor, `missing readable descriptor for ${name}`);
    assert.equal(digestCanonical(descriptor).value, digest, `descriptor digest: ${name}`);
    (actualForms[name] ??= new Set()).add(digest);
    used.add(digest);
    if (name.startsWith('table:')) {
      const indexes = (actualIndexes[name] ??= new Set());
      for (const index of Object.keys(descriptor.indexDetails)) indexes.add(index);
    }
  }
}
const sortedSets = (value) => Object.fromEntries(Object.entries(value).map(([key, values]) => [key, [...values].sort()]));
assert.deepEqual(sortedSets(actualForms), manifest.objects);
assert.deepEqual(sortedSets(actualIndexes), manifest.knownIndexes);
assert.deepEqual([...used].sort(), Object.keys(review.descriptors).sort());
for (const source of Object.values(review.sources)) {
  assert.equal(createHash('sha256').update(read(source.path)).digest('hex'), source.sha256, `fixture drift: ${source.path}`);
}
if (process.argv[2]) {
  const bytes = readFileSync(process.argv[2]);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), manifest.captureSha256, 'capture bytes changed');
  const capture = JSON.parse(bytes);
  assert.deepEqual(Object.keys(capture.forms).sort(), [...manifest.fixtureForms].sort());
  assert.equal(capture.kind, 'production_initialized_schema_forms_for_review');
  assert.equal(capture.presenceChecks, Object.keys(manifest.objects).length);
  for (const [fixture, objects] of Object.entries(capture.forms)) {
    assert.deepEqual(Object.keys(objects).sort(), Object.keys(review.fixtureObjects[fixture]).sort());
    for (const [name, descriptor] of Object.entries(objects)) {
      assert.equal(digestCanonical(descriptor).value, review.fixtureObjects[fixture][name].sha256, `${fixture}/${name}`);
    }
  }
}
console.log(`Doctor schema review: ${Object.keys(manifest.objects).length} objects, ${used.size} forms, ${manifest.fixtureForms.length} fixture histories; review_only.`);
