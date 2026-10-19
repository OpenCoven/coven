import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import {
  RELEASE_SIGNERS_PATH,
  allowedSignersText,
  parseReleaseSigners,
  resolveSignerKeys,
  signerPrincipal,
  verifyReleaseTag
} from './verify-release-tag.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPOSITORY = 'OpenCoven/coven';
const TAG = 'v1.2.3';
const LOGIN = 'release-bot';
const USER_ID = 1001;
const KEY_ID_TRUSTED = 501;
const KEY_ID_OTHER = 502;
const PRINCIPAL = `${USER_ID}+${LOGIN}@users.noreply.github.com`;
const FIXED_NOW = () => new Date('2026-10-19T00:00:00Z');

function run(command, args, cwd, env = {}) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: 'utf8',
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', ...env }
  });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed: ${result.stderr}`);
  }
  return result.stdout.trim();
}

function pins(entries) {
  return `${JSON.stringify({ schemaVersion: 1, kind: 'coven.release-signers', signers: entries }, null, 2)}\n`;
}

function pin(sshSigningKeyId = KEY_ID_TRUSTED, overrides = {}) {
  return {
    githubLogin: LOGIN,
    githubUserId: USER_ID,
    sshSigningKeyId,
    keyType: 'ssh-ed25519',
    ...overrides
  };
}

/// A bare origin plus a work clone whose main pins the trusted key, with a
/// signed release tag. Each test then bends one fact and expects a refusal.
function createFixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'verify-release-tag-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const keys = {};
  for (const name of ['trusted', 'other']) {
    const keyPath = path.join(root, name);
    run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-C', name, '-f', keyPath], root);
    keys[name] = {
      path: keyPath,
      publicKey: readFileSync(`${keyPath}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' '),
      fingerprint: run('ssh-keygen', ['-l', '-E', 'sha256', '-f', `${keyPath}.pub`], root).split(/\s+/)[1]
    };
  }
  const origin = path.join(root, 'origin.git');
  const work = path.join(root, 'work');
  run('git', ['init', '-q', '--bare', '-b', 'main', origin], root);
  run('git', ['clone', '-q', origin, work], root);
  run('git', ['symbolic-ref', 'HEAD', 'refs/heads/main'], work);
  const identity = ['-c', 'user.name=Release Bot', '-c', `user.email=${PRINCIPAL}`];
  const commit = (message, files = {}) => {
    for (const [file, content] of Object.entries(files)) {
      mkdirSync(path.dirname(path.join(work, file)), { recursive: true });
      writeFileSync(path.join(work, file), content);
      run('git', ['add', file], work);
    }
    run('git', [...identity, 'commit', '-q', '--allow-empty', '-m', message], work);
    return run('git', ['rev-parse', 'HEAD'], work);
  };
  const signedTag = (name, keyName = 'trusted', target = 'HEAD') => {
    run('git', [...identity, '-c', 'gpg.format=ssh', '-c', `user.signingkey=${keys[keyName].path}`, 'tag', '-s', name, '-m', `Release ${name}`, target], work);
    return run('git', ['rev-parse', `refs/tags/${name}`], work);
  };
  const push = (ref = 'main') => run('git', ['push', '-q', 'origin', ref], work);

  commit('initial', { [RELEASE_SIGNERS_PATH]: pins([pin()]) });
  push();
  const tagObjectSha = signedTag(TAG);
  const headSha = run('git', ['rev-parse', `refs/tags/${TAG}^{commit}`], work);

  const state = {
    tagRef: { ref: `refs/tags/${TAG}`, object: { type: 'tag', sha: tagObjectSha } },
    tagObject: { tag: TAG, verification: { verified: true, reason: 'valid' }, object: { type: 'commit', sha: headSha } },
    user: { login: LOGIN, id: USER_ID },
    signingKeys: [
      { id: KEY_ID_TRUSTED, key: `${keys.trusted.publicKey} trusted` },
      { id: KEY_ID_OTHER, key: keys.other.publicKey }
    ]
  };
  const ghApi = async (endpoint) => {
    if (endpoint === `/repos/${REPOSITORY}/git/ref/tags/${encodeURIComponent(state.tagRef.ref.slice('refs/tags/'.length))}`) {
      return state.tagRef;
    }
    if (endpoint === `/repos/${REPOSITORY}/git/tags/${state.tagRef.object.sha}`) {
      return state.tagObject;
    }
    if (endpoint === `/users/${LOGIN}`) {
      return state.user;
    }
    if (endpoint === `/users/${LOGIN}/ssh_signing_keys?per_page=100`) {
      return state.signingKeys;
    }
    throw new Error(`unexpected GitHub API read ${endpoint}`);
  };
  const verify = (overrides = {}) => verifyReleaseTag({
    repository: REPOSITORY,
    releaseTag: TAG,
    cwd: work,
    ghApi,
    now: FIXED_NOW,
    ...overrides
  });
  return { root, work, keys, state, commit, signedTag, push, verify, tagObjectSha, headSha, identity };
}

/// Re-points the GitHub fakes at a re-created tag after a test rewrites it.
function retarget(fixture, name = TAG) {
  const tagObjectSha = run('git', ['rev-parse', `refs/tags/${name}`], fixture.work);
  const headSha = run('git', ['rev-parse', `refs/tags/${name}^{commit}`], fixture.work);
  fixture.state.tagRef = { ref: `refs/tags/${name}`, object: { type: 'tag', sha: tagObjectSha } };
  fixture.state.tagObject = { ...fixture.state.tagObject, tag: name, object: { type: 'commit', sha: headSha } };
  return { tagObjectSha, headSha };
}

test('a tag signed by the pinned key on a main commit verifies and records its evidence', async (t) => {
  const fixture = createFixture(t);
  const result = await fixture.verify();
  assert.deepEqual(result, {
    schemaVersion: 1,
    kind: 'coven.release-tag-verification',
    repository: REPOSITORY,
    releaseTag: TAG,
    tagObjectSha: fixture.tagObjectSha,
    headSha: fixture.headSha,
    defaultBranch: 'main',
    defaultBranchSha: fixture.headSha,
    github: { verified: true, reason: 'valid' },
    signer: {
      principal: PRINCIPAL,
      githubLogin: LOGIN,
      githubUserId: USER_ID,
      sshSigningKeyId: KEY_ID_TRUSTED,
      keyAlgorithm: 'ED25519',
      fingerprint: fixture.keys.trusted.fingerprint
    },
    signerPins: {
      path: RELEASE_SIGNERS_PATH,
      taggedCommitSha256: result.signerPins.taggedCommitSha256,
      defaultBranchSha256: result.signerPins.defaultBranchSha256
    },
    observedAt: '2026-10-19T00:00:00.000Z'
  });
  assert.match(result.signerPins.taggedCommitSha256, /^[0-9a-f]{64}$/);
  assert.equal(result.signerPins.taggedCommitSha256, result.signerPins.defaultBranchSha256);
});

test('a tag on an older main commit verifies against the current pins', async (t) => {
  const fixture = createFixture(t);
  fixture.commit('later work');
  fixture.push();
  const result = await fixture.verify();
  assert.equal(result.headSha, fixture.headSha);
  assert.notEqual(result.defaultBranchSha, fixture.headSha);
});

test('refuses a tag signed by a key the pins do not trust, even when GitHub verified it', async (t) => {
  const fixture = createFixture(t);
  run('git', ['tag', '-d', TAG], fixture.work);
  fixture.signedTag(TAG, 'other');
  retarget(fixture);
  await assert.rejects(fixture.verify(), /not signed by a trusted signer.*No principal matched/);
});

test('refuses an annotated tag with no signature', async (t) => {
  const fixture = createFixture(t);
  run('git', ['tag', '-d', TAG], fixture.work);
  run('git', [...fixture.identity, 'tag', '-a', TAG, '-m', 'unsigned'], fixture.work);
  retarget(fixture);
  await assert.rejects(fixture.verify(), /not signed by a trusted signer/);
});

test('refuses a lightweight tag before asking GitHub anything', async (t) => {
  const fixture = createFixture(t);
  run('git', ['tag', '-d', TAG], fixture.work);
  run('git', ['tag', TAG], fixture.work);
  await assert.rejects(fixture.verify(), /lightweight tag/);
});

test('refuses a tag GitHub does not report as verified', async (t) => {
  const fixture = createFixture(t);
  fixture.state.tagObject.verification = { verified: false, reason: 'unknown_key' };
  await assert.rejects(fixture.verify(), /GitHub-verified signature \(reason=unknown_key\)/);
});

test('refuses when GitHub resolves the tag ref to a different tag object', async (t) => {
  const fixture = createFixture(t);
  fixture.state.tagRef = { ...fixture.state.tagRef, object: { type: 'tag', sha: 'a'.repeat(40) } };
  await assert.rejects(fixture.verify(), /does not match the GitHub tag object/);
});

test('refuses a GitHub tag ref that is lightweight or names another ref', async (t) => {
  const fixture = createFixture(t);
  fixture.state.tagRef = { ...fixture.state.tagRef, object: { type: 'commit', sha: fixture.headSha } };
  await assert.rejects(fixture.verify(), /must resolve to an annotated tag object/);
});

test('refuses a GitHub tag payload that names another tag or targets a non-commit', async (t) => {
  const fixture = createFixture(t);
  fixture.state.tagObject = { ...fixture.state.tagObject, tag: 'v9.9.9' };
  await assert.rejects(fixture.verify(), /payload named "v9\.9\.9"/);
  fixture.state.tagObject = { ...fixture.state.tagObject, tag: TAG, object: { type: 'tree', sha: fixture.headSha } };
  await assert.rejects(fixture.verify(), /targets "tree", not a commit/);
});

test('refuses when GitHub and the local tag disagree about the target commit', async (t) => {
  const fixture = createFixture(t);
  fixture.state.tagObject = { ...fixture.state.tagObject, object: { type: 'commit', sha: 'b'.repeat(40) } };
  await assert.rejects(fixture.verify(), /not the GitHub-verified commit/);
});

test('refuses a signed tag whose commit never reached main', async (t) => {
  const fixture = createFixture(t);
  run('git', ['switch', '-q', '-c', 'side'], fixture.work);
  fixture.commit('side work');
  run('git', ['tag', '-d', TAG], fixture.work);
  fixture.signedTag(TAG);
  retarget(fixture);
  await assert.rejects(fixture.verify(), /is not contained in origin\/main/);
});

test('a signer removed on main is revoked for tags cut before the removal', async (t) => {
  const fixture = createFixture(t);
  fixture.commit('revoke the trusted key', { [RELEASE_SIGNERS_PATH]: pins([pin(KEY_ID_OTHER)]) });
  fixture.push();
  await assert.rejects(fixture.verify(), /not signed by a trusted signer/);
});

test('a signer pinned on main does not authorize a tag whose commit pins someone else', async (t) => {
  const fixture = createFixture(t);
  fixture.commit('pin only the other key', { [RELEASE_SIGNERS_PATH]: pins([pin(KEY_ID_OTHER)]) });
  run('git', ['tag', '-d', TAG], fixture.work);
  fixture.signedTag(TAG);
  fixture.commit('pin the trusted key again', { [RELEASE_SIGNERS_PATH]: pins([pin()]) });
  fixture.push();
  retarget(fixture);
  await assert.rejects(fixture.verify(), /not signed by a trusted signer/);
});

test('refuses when the pinned key was deleted from GitHub', async (t) => {
  const fixture = createFixture(t);
  fixture.state.signingKeys = fixture.state.signingKeys.filter((key) => key.id !== KEY_ID_TRUSTED);
  await assert.rejects(fixture.verify(), /has no SSH signing key 501/);
});

test('refuses when the pinned login now belongs to another account', async (t) => {
  const fixture = createFixture(t);
  fixture.state.user = { login: LOGIN, id: 2002 };
  await assert.rejects(fixture.verify(), /now belongs to user 2002, not pinned user 1001/);
});

test('refuses a pinned key whose type differs from the pin', async (t) => {
  const fixture = createFixture(t);
  fixture.state.signingKeys[0] = { id: KEY_ID_TRUSTED, key: 'ssh-rsa AAAAB3NzaC1yc2E' };
  await assert.rejects(fixture.verify(), /is not a ssh-ed25519 public key/);
});

test('refuses a truncated signing-key listing', async (t) => {
  const fixture = createFixture(t);
  fixture.state.signingKeys = Array.from({ length: 100 }, (_, index) => ({ id: index + 1, key: 'ssh-ed25519 AAAA' }));
  await assert.rejects(fixture.verify(), /complete SSH signing-key list/);
});

test('refuses a tagged commit that carries no signer pins', async (t) => {
  const fixture = createFixture(t);
  run('git', ['rm', '-q', RELEASE_SIGNERS_PATH], fixture.work);
  fixture.commit('drop the pins');
  run('git', ['tag', '-d', TAG], fixture.work);
  fixture.signedTag(TAG);
  fixture.commit('restore the pins', { [RELEASE_SIGNERS_PATH]: pins([pin()]) });
  fixture.push();
  retarget(fixture);
  await assert.rejects(fixture.verify(), /release-signers\.json is missing at the tagged commit/);
});

test('rejects unsafe repository, tag, and branch names before running git', async (t) => {
  const fixture = createFixture(t);
  await assert.rejects(fixture.verify({ repository: 'not a repo' }), /OWNER\/REPO/);
  await assert.rejects(fixture.verify({ releaseTag: '--upload-pack=x' }), /unsupported tag or branch name/);
  await assert.rejects(fixture.verify({ defaultBranch: 'main:refs/heads/x' }), /unsupported tag or branch name/);
});

test('signer pins are validated strictly', () => {
  const valid = [pin()];
  assert.deepEqual(parseReleaseSigners(pins(valid)), [{ ...pin(), principal: PRINCIPAL }]);
  const cases = [
    ['not json', /not valid JSON/],
    [JSON.stringify({ schemaVersion: 2, kind: 'coven.release-signers', signers: valid }), /schemaVersion 1/],
    [pins([]), /at least one trusted signer/],
    [pins([{ ...pin(), comment: 'x' }]), /must have exactly/],
    [pins([pin(KEY_ID_TRUSTED, { principal: PRINCIPAL })]), /must have exactly/],
    [pins([pin(KEY_ID_TRUSTED, { githubLogin: 'bad login' })]), /not a GitHub login/],
    [pins([pin(KEY_ID_TRUSTED, { githubUserId: '1001' })]), /positive integers/],
    [pins([pin(0)]), /positive integers/],
    [pins([pin(KEY_ID_TRUSTED, { keyType: 'ssh-rsa' })]), /keyType "ssh-rsa"/],
    [pins([pin(), pin(KEY_ID_OTHER)]), /repeats a signer/],
    [pins([pin(), pin(KEY_ID_TRUSTED, { githubLogin: 'second-bot', githubUserId: 2002 })]), /repeats a signer or signing key/]
  ];
  for (const [text, expected] of cases) {
    assert.throws(() => parseReleaseSigners(text), expected, text);
  }
});

test('allowed signers carry only principal, key type, and key', async () => {
  const resolved = await resolveSignerKeys(parseReleaseSigners(pins([pin()])), {
    ghApi: async (endpoint) => (endpoint.endsWith('ssh_signing_keys?per_page=100')
      ? [{ id: KEY_ID_TRUSTED, key: 'ssh-ed25519 AAAAC3Nza laptop comment' }]
      : { id: USER_ID })
  });
  assert.equal(allowedSignersText(resolved), `${PRINCIPAL} ssh-ed25519 AAAAC3Nza\n`);
});

test('the checked-in pins name the maintainer signing key the release variable trusted', () => {
  const signers = parseReleaseSigners(readFileSync(path.join(repoRoot, RELEASE_SIGNERS_PATH), 'utf8'));
  assert.deepEqual(signers, [{
    principal: signerPrincipal({ githubUserId: 68980965, githubLogin: 'BunsDev' }),
    githubLogin: 'BunsDev',
    githubUserId: 68980965,
    sshSigningKeyId: 880193,
    keyType: 'ssh-ed25519'
  }]);
});
