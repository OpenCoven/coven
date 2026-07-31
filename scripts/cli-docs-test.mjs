import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const coreGuideDocs = [
  {
    path: 'docs/development/cli-core-functionality.md',
    required: ['Command ownership', 'Access contract', 'coven doctor --json', 'coven daemon status --json']
  },
  {
    path: 'docs/guides/index.md',
    required: ['/guides/core-access', '/guides/session-operations', '/guides/automation-json', '/guides/multi-agent-worktrees', '/guides/troubleshooting-core-access']
  },
  {
    path: 'docs/guides/core-access.md',
    required: ['coven doctor', 'coven daemon start', 'coven run codex', 'coven sessions']
  },
  {
    path: 'docs/guides/session-operations.md',
    required: ['coven sessions --plain', 'coven attach', 'coven archive', 'coven sacrifice']
  },
  {
    path: 'docs/guides/automation-json.md',
    required: ['coven doctor --json', 'coven daemon status --json', 'coven sessions --json']
  },
  {
    path: 'docs/guides/multi-agent-worktrees.md',
    required: ['coven wt', 'coven claim acquire', 'coven hooks install']
  },
  {
    path: 'docs/guides/troubleshooting-core-access.md',
    required: ['COVEN_HOME', 'coven daemon status', 'coven doctor']
  }
];

function readRepoFile(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
}

function escaped(phrase) {
  return phrase.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

test('prepublish smoke runs the CLI docs discovery guard', () => {
  const prepublish = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(prepublish, /scripts\/cli-docs-test\.mjs/);
});

test('core CLI docs are discoverable from the README and guide index', () => {
  const readme = readRepoFile('README.md');
  assert.match(readme, /docs\/development\/cli-core-functionality\.md/);
  assert.match(readme, /docs\/guides\/index\.md/);
  assert.match(readme, /docs\/reference\/cli-coven\.md/);

  const topLevelCli = readRepoFile('docs/reference/cli-coven.md');
  assert.doesNotMatch(topLevelCli, /^Stub -- fill in\.?$/m);
  assert.match(topLevelCli, /## Usage/);
  assert.match(topLevelCli, /## Related/);
  assert.match(topLevelCli, /coven chat/);

  for (const { path, required } of coreGuideDocs) {
    const text = readRepoFile(path);
    assert.doesNotMatch(text, /^Stub -- fill in\.?$/m, `${path} must not be a stub`);
    assert.match(text, /## Related/, `${path} must link next steps`);
    for (const phrase of required) {
      assert.match(text, new RegExp(escaped(phrase), 'i'), `${path} must mention ${phrase}`);
    }
  }
});
