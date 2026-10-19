import assert from 'node:assert/strict';
import test from 'node:test';

import {
  checkActionPins,
  nodeVersion,
  readRepositoryInputs,
  rustChannel,
  verifyRemoteLock
} from './check-action-pins.mjs';

const CHECKOUT = '3d3c42e5aac5ba805825da76410c181273ba90b1';
const SETUP_NODE = '949feb2413d6458794dcd2491c4babbbce0c15c1';
const TOOLCHAIN = '683b588c487a69d01f6c6ddf40f036ff682cc2c8';

function inputs(workflow, overrides = {}) {
  return {
    lock: {
      schemaVersion: 1,
      kind: 'coven.actions-lock',
      actions: {
        'actions/checkout': { sha: CHECKOUT, ref: 'v7.0.1', refKind: 'tag' },
        'actions/setup-node': { sha: SETUP_NODE, ref: 'v7.1.0', refKind: 'tag' },
        'dtolnay/rust-toolchain': { sha: TOOLCHAIN, ref: 'master', refKind: 'branch' }
      }
    },
    rustToolchain: '[toolchain]\nchannel = "1.99.1"\n',
    nodeVersion: '24.22.0\n',
    files: [{ path: '.github/workflows/example.yml', text: workflow }],
    ...overrides
  };
}

const GOOD = `name: Example
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@${CHECKOUT} # v7.0.1
      - uses: dtolnay/rust-toolchain@${TOOLCHAIN} # master
        with:
          toolchain: 1.99.1
          components: clippy
      - uses: actions/setup-node@${SETUP_NODE} # v7.1.0
        with:
          node-version-file: .node-version
      - run: node --test scripts/example.test.mjs
  shell-only:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@${CHECKOUT} # v7.0.1
      - run: cargo test --workspace --locked
`;

test('the repository pins every action, toolchain, and Node runtime', () => {
  assert.deepEqual(checkActionPins(readRepositoryInputs()), []);
});

test('a consistent synthetic workflow passes', () => {
  assert.deepEqual(checkActionPins(inputs(GOOD)), []);
});

const drift = [
  ['a moving tag', (text) => text.replace(`actions/checkout@${CHECKOUT} # v7.0.1\n      - uses: dtolnay`, 'actions/checkout@v7.0.1\n      - uses: dtolnay'), /actions\/checkout@v7\.0\.1 is not pinned to a full commit SHA/],
  ['a moving branch', (text) => text.replace(`dtolnay/rust-toolchain@${TOOLCHAIN} # master`, 'dtolnay/rust-toolchain@stable'), /rust-toolchain@stable is not pinned/],
  ['a short SHA', (text) => text.replace(`actions/setup-node@${SETUP_NODE}`, `actions/setup-node@${SETUP_NODE.slice(0, 12)}`), /not pinned to a full commit SHA/],
  ['a SHA that is not the locked one', (text) => text.replace(`actions/setup-node@${SETUP_NODE}`, `actions/setup-node@${'a'.repeat(40)}`), /differs from the locked/],
  ['a missing version comment', (text) => text.replace(`@${SETUP_NODE} # v7.1.0`, `@${SETUP_NODE}`), /must carry the comment "# v7\.1\.0", got null/],
  ['a wrong version comment', (text) => text.replace(`@${SETUP_NODE} # v7.1.0`, `@${SETUP_NODE} # v6.0.0`), /got "v6\.0\.0"/],
  ['an unlocked action', (text) => text.replace('      - run: node --test', `      - uses: someone/else@${'b'.repeat(40)} # v1\n      - run: node --test`), /someone\/else is not recorded/],
  ['a docker action by tag', (text) => text.replace('      - run: node --test', '      - uses: docker://alpine:3\n      - run: node --test'), /must be pinned by @sha256 digest/],
  ['a floating Rust toolchain', (text) => text.replace('toolchain: 1.99.1', 'toolchain: stable'), /installs "stable", rust-toolchain\.toml pins 1\.99\.1/],
  ['an omitted Rust toolchain', (text) => text.replace('          toolchain: 1.99.1\n', ''), /installs "clippy"|installs null/],
  ['a literal Node version', (text) => text.replace('node-version-file: .node-version', 'node-version: 24'), /must read \.node-version instead of a literal node-version/],
  ['a job that runs node on the runner image', (text) => text.replace('      - run: cargo test --workspace --locked', '      - run: node scripts/build.mjs'), /job shell-only runs node without installing \.node-version/],
  ['npx on the runner image', (text) => text.replace('      - run: cargo test --workspace --locked', '      - run: |\n          set -e\n          npx something'), /job shell-only runs node/]
];

for (const [label, mutate, expected] of drift) {
  test(`refuses ${label}`, () => {
    const mutated = mutate(GOOD);
    assert.notEqual(mutated, GOOD, 'mutation did not apply');
    const findings = checkActionPins(inputs(mutated));
    assert.ok(findings.some((finding) => expected.test(finding)), findings.join('\n'));
  });
}

test('refuses toolchain files and lock entries that do not pin exactly', () => {
  assert.throws(() => rustChannel('[toolchain]\nchannel = "stable"\n'), /exact x\.y\.z/);
  assert.throws(() => rustChannel('[toolchain]\n'), /exact x\.y\.z/);
  assert.throws(() => nodeVersion('24\n'), /exact x\.y\.z/);
  assert.equal(rustChannel('channel = "1.99.1"'), '1.99.1');
  assert.equal(nodeVersion('24.22.0\n'), '24.22.0');
  const loose = inputs(GOOD);
  loose.lock.actions['actions/checkout'] = { sha: CHECKOUT, ref: 'v7.0.1' };
  assert.ok(checkActionPins(loose).some((finding) => /needs a 40-character sha, a ref, and refKind/.test(finding)));
  const stale = inputs(GOOD);
  stale.lock.actions['actions/cache'] = { sha: 'c'.repeat(40), ref: 'v6.1.0', refKind: 'tag' };
  assert.ok(checkActionPins(stale).some((finding) => /records actions\/cache, which no workflow uses/.test(finding)));
  assert.deepEqual(checkActionPins({ ...inputs(GOOD), lock: { kind: 'other' } }), [
    '.github/actions-lock.json must be coven.actions-lock schemaVersion 1.'
  ]);
});

test('commented-out uses lines and local actions are not dependencies', () => {
  const text = GOOD.replace('      - run: node --test', '      # - uses: actions/checkout@v4\n      - uses: ./.github/actions/local\n      - run: node --test');
  assert.deepEqual(checkActionPins(inputs(text)), []);
});

test('remote verification follows annotated tags and requires branch containment', () => {
  const lock = inputs(GOOD).lock;
  const answers = {
    '/repos/actions/checkout/git/ref/tags/v7.0.1': { object: { type: 'tag', sha: 'd'.repeat(40) } },
    [`/repos/actions/checkout/git/tags/${'d'.repeat(40)}`]: { object: { sha: CHECKOUT } },
    '/repos/actions/setup-node/git/ref/tags/v7.1.0': { object: { type: 'commit', sha: SETUP_NODE } },
    [`/repos/dtolnay/rust-toolchain/compare/${TOOLCHAIN}...master`]: { status: 'ahead' }
  };
  const ghApi = (endpoint) => {
    if (!(endpoint in answers)) {
      throw new Error(`unexpected ${endpoint}`);
    }
    return answers[endpoint];
  };
  assert.deepEqual(verifyRemoteLock(lock, ghApi), []);
  answers['/repos/actions/setup-node/git/ref/tags/v7.1.0'] = { object: { type: 'commit', sha: 'e'.repeat(40) } };
  answers[`/repos/dtolnay/rust-toolchain/compare/${TOOLCHAIN}...master`] = { status: 'diverged' };
  assert.deepEqual(verifyRemoteLock(lock, ghApi), [
    `actions/setup-node tag v7.1.0 now resolves to ${'e'.repeat(40)}, the lock pins ${SETUP_NODE}.`,
    `dtolnay/rust-toolchain commit ${TOOLCHAIN} is no longer contained in branch master (diverged).`
  ]);
});

test('update locks a verified Dependabot-style bump and writes one changed line', async () => {
  const { updateLock, renderLock } = await import('./check-action-pins.mjs');
  const bumped = '7'.repeat(40);
  const workflow = GOOD.replaceAll(`actions/checkout@${CHECKOUT} # v7.0.1`, `actions/checkout@${bumped} # v7.0.2`);
  let written = null;
  const result = updateLock(inputs(workflow), {
    ghApi: (endpoint) => {
      assert.equal(endpoint, '/repos/actions/checkout/git/ref/tags/v7.0.2', 'only the changed pin is verified');
      return { object: { type: 'commit', sha: bumped } };
    },
    write: (text) => { written = text; }
  });
  assert.deepEqual(result.findings, []);
  assert.deepEqual(result.proposal.changed, ['actions/checkout']);
  assert.deepEqual(JSON.parse(written).actions['actions/checkout'], { sha: bumped, ref: 'v7.0.2', refKind: 'tag' });
  assert.equal(written, renderLock(JSON.parse(written)));
  assert.equal(JSON.parse(written).actions['dtolnay/rust-toolchain'].refKind, 'branch', 'an unchanged branch pin stays a branch');
});

test('update refuses a bump whose tag names another commit and writes nothing', async () => {
  const { updateLock } = await import('./check-action-pins.mjs');
  const workflow = GOOD.replaceAll(`actions/checkout@${CHECKOUT} # v7.0.1`, `actions/checkout@${'7'.repeat(40)} # v7.0.2`);
  let written = false;
  const result = updateLock(inputs(workflow), {
    ghApi: () => ({ object: { type: 'commit', sha: '8'.repeat(40) } }),
    write: () => { written = true; }
  });
  assert.match(result.findings.join('\n'), /tag v7\.0\.2 now resolves to 8{40}, the lock pins 7{40}/);
  assert.equal(written, false);
});

test('update refuses conflicting or uncommented pins and drops unused entries', async () => {
  const { proposeLock, updateLock } = await import('./check-action-pins.mjs');
  const conflicting = GOOD.replace(`actions/checkout@${CHECKOUT} # v7.0.1\n      - run: cargo`, `actions/checkout@${'9'.repeat(40)} # v7.0.9\n      - run: cargo`);
  assert.match(proposeLock(inputs(conflicting)).problems.join('\n'), /disagrees with .*use one pin per action/);
  const uncommented = GOOD.replace(`@${SETUP_NODE} # v7.1.0`, `@${SETUP_NODE}`);
  assert.match(proposeLock(inputs(uncommented)).problems.join('\n'), /with a "# <ref>" comment/);
  let written = false;
  const refused = updateLock(inputs(uncommented), { ghApi: () => { throw new Error('no network'); }, write: () => { written = true; } });
  assert.ok(refused.findings.length > 0);
  assert.equal(written, false);

  const withoutNode = GOOD
    .replace(`      - uses: actions/setup-node@${SETUP_NODE} # v7.1.0\n        with:\n          node-version-file: .node-version\n`, '')
    .replace('      - run: node --test scripts/example.test.mjs\n', '');
  let text = null;
  const pruned = updateLock(inputs(withoutNode), { ghApi: () => { throw new Error('nothing changed to verify'); }, write: (value) => { text = value; } });
  assert.deepEqual(pruned.findings, []);
  assert.deepEqual(pruned.proposal.removed, ['actions/setup-node']);
  assert.equal('actions/setup-node' in JSON.parse(text).actions, false);
});

test('update writes nothing when the lock already matches', async () => {
  const { updateLock } = await import('./check-action-pins.mjs');
  let written = false;
  const result = updateLock(inputs(GOOD), { ghApi: () => { throw new Error('nothing to verify'); }, write: () => { written = true; } });
  assert.deepEqual(result.findings, []);
  assert.equal(written, false);
});
