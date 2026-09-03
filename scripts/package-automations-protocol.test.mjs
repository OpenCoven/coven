import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { gunzipSync, gzipSync } from 'node:zlib';

import {
  packageAutomationsProtocol,
  verifyAutomationsProtocolBundle
} from './package-automations-protocol.mjs';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const scriptPath = path.join(repositoryRoot, 'scripts', 'package-automations-protocol.mjs');

function runGit(cwd, args) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8' });
  assert.equal(
    result.status,
    0,
    `git ${args.join(' ')} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`
  );
  return result.stdout.trim();
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function createFixtureRepository(scratchDir) {
  const repoRoot = path.join(scratchDir, 'repo');
  const specDir = path.join(repoRoot, 'spec', 'coven-automations', 'v1');
  mkdirSync(path.join(specDir, 'nested'), { recursive: true });
  writeFileSync(path.join(specDir, 'README.md'), '# Contract\n');
  writeFileSync(path.join(specDir, 'b.json'), '{"b":2}\n');
  writeFileSync(path.join(specDir, 'nested', 'a.json'), '{"a":1}\n');
  runGit(repoRoot, ['init']);
  runGit(repoRoot, ['config', 'user.name', 'Protocol Test']);
  runGit(repoRoot, ['config', 'user.email', 'protocol@example.invalid']);
  runGit(repoRoot, ['add', '.']);
  runGit(repoRoot, ['commit', '-m', 'test: seed protocol']);
  return {
    repoRoot,
    specDir,
    sourceCommit: runGit(repoRoot, ['rev-parse', 'HEAD'])
  };
}

function parseTarGz(bytes) {
  const tar = gunzipSync(bytes);
  const entries = [];
  for (let offset = 0; offset + 512 <= tar.length; ) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      break;
    }
    const readString = (start, length) =>
      header
        .subarray(start, start + length)
        .toString('utf8')
        .replace(/\0.*$/s, '');
    const readOctal = (start, length) => Number.parseInt(readString(start, length).trim() || '0', 8);
    const name = readString(0, 100);
    const size = readOctal(124, 12);
    entries.push({
      name,
      mode: readOctal(100, 8),
      uid: readOctal(108, 8),
      gid: readOctal(116, 8),
      size,
      mtime: readOctal(136, 12),
      type: readString(156, 1),
      data: tar.subarray(offset + 512, offset + 512 + size)
    });
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  return entries;
}

function expectedContentDigest(files) {
  return sha256(
    Buffer.from(files.map(({ path: relativePath, sha256: digest }) => `${relativePath}\0${digest}\n`).join(''))
  );
}

function withScratchDir(name, callback) {
  const scratchDir = mkdtempSync(path.join(tmpdir(), `coven-${name}-`));
  try {
    return callback(scratchDir);
  } finally {
    rmSync(scratchDir, { recursive: true, force: true });
  }
}

test('packages a deterministic source-bound protocol archive with normalized metadata', () => {
  withScratchDir('automation-protocol-package', (scratchDir) => {
    const fixture = createFixtureRepository(scratchDir);
    const firstOutput = path.join(scratchDir, 'first');
    const secondOutput = path.join(scratchDir, 'second');

    const first = packageAutomationsProtocol({
      repoRoot: fixture.repoRoot,
      outputDir: firstOutput,
      sourceCommit: fixture.sourceCommit
    });
    const second = packageAutomationsProtocol({
      repoRoot: fixture.repoRoot,
      outputDir: secondOutput,
      sourceCommit: fixture.sourceCommit
    });

    assert.equal(
      path.basename(first.bundlePath),
      `coven-automations-v1-contract-${fixture.sourceCommit}.tar.gz`
    );
    assert.deepEqual(readFileSync(first.bundlePath), readFileSync(second.bundlePath));
    assert.deepEqual(readFileSync(first.manifestPath), readFileSync(second.manifestPath));

    const manifest = JSON.parse(readFileSync(first.manifestPath, 'utf8'));
    assert.equal(manifest.schemaVersion, 'coven.automations.bundle.v1');
    assert.equal(manifest.contractProfile, 'coven.automations.v1');
    assert.equal(manifest.sourceCommit, fixture.sourceCommit);
    assert.deepEqual(
      manifest.files.map((file) => file.path),
      ['README.md', 'b.json', 'nested/a.json']
    );
    assert.equal(manifest.contractContentSha256, expectedContentDigest(manifest.files));

    const entries = parseTarGz(readFileSync(first.bundlePath));
    assert.deepEqual(
      entries.map((entry) => entry.name),
      [
        'coven-automations-v1/README.md',
        'coven-automations-v1/b.json',
        'coven-automations-v1/nested/a.json',
        'manifest.json'
      ]
    );
    for (const entry of entries) {
      assert.equal(entry.mode, 0o644);
      assert.equal(entry.uid, 0);
      assert.equal(entry.gid, 0);
      assert.equal(entry.mtime, 0);
      assert.equal(entry.type, '0');
    }
    assert.deepEqual(
      JSON.parse(entries.at(-1).data.toString('utf8')),
      manifest
    );
    assert.equal(first.bundleSha256, sha256(readFileSync(first.bundlePath)));
    assert.deepEqual(
      [...readFileSync(first.bundlePath).subarray(0, 10)],
      [0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff]
    );
  });
});

test('keeps the content digest stable while binding bundle bytes to the source commit', () => {
  withScratchDir('automation-protocol-source-binding', (scratchDir) => {
    const fixture = createFixtureRepository(scratchDir);
    const first = packageAutomationsProtocol({
      repoRoot: fixture.repoRoot,
      outputDir: path.join(scratchDir, 'first'),
      sourceCommit: fixture.sourceCommit
    });
    writeFileSync(path.join(fixture.repoRoot, 'unrelated.txt'), 'different source commit\n');
    runGit(fixture.repoRoot, ['add', 'unrelated.txt']);
    runGit(fixture.repoRoot, ['commit', '-m', 'test: advance source']);
    const secondCommit = runGit(fixture.repoRoot, ['rev-parse', 'HEAD']);
    const second = packageAutomationsProtocol({
      repoRoot: fixture.repoRoot,
      outputDir: path.join(scratchDir, 'second'),
      sourceCommit: secondCommit
    });

    const firstManifest = JSON.parse(readFileSync(first.manifestPath, 'utf8'));
    const secondManifest = JSON.parse(readFileSync(second.manifestPath, 'utf8'));
    assert.equal(firstManifest.contractContentSha256, secondManifest.contractContentSha256);
    assert.notEqual(first.bundleSha256, second.bundleSha256);
  });
});

test('refuses dirty, mismatched, or non-regular protocol input trees', () => {
  withScratchDir('automation-protocol-refusal', (scratchDir) => {
    const fixture = createFixtureRepository(scratchDir);
    assert.throws(
      () =>
        packageAutomationsProtocol({
          repoRoot: fixture.repoRoot,
          outputDir: path.join(scratchDir, 'mismatch'),
          sourceCommit: '0'.repeat(40)
        }),
      /source commit .* does not match HEAD/i
    );

    writeFileSync(path.join(fixture.specDir, 'README.md'), '# Dirty contract\n');
    assert.throws(
      () =>
        packageAutomationsProtocol({
          repoRoot: fixture.repoRoot,
          outputDir: path.join(scratchDir, 'dirty'),
          sourceCommit: fixture.sourceCommit
        }),
      /protocol input tree is dirty/i
    );
    runGit(fixture.repoRoot, ['restore', 'spec/coven-automations/v1/README.md']);

    writeFileSync(path.join(fixture.repoRoot, '.gitignore'), '*.log\n');
    runGit(fixture.repoRoot, ['add', '.gitignore']);
    runGit(fixture.repoRoot, ['commit', '-m', 'test: ignore logs']);
    const ignoredCommit = runGit(fixture.repoRoot, ['rev-parse', 'HEAD']);
    writeFileSync(path.join(fixture.specDir, 'ignored.log'), 'must not enter bundle\n');
    assert.throws(
      () =>
        packageAutomationsProtocol({
          repoRoot: fixture.repoRoot,
          outputDir: path.join(scratchDir, 'ignored'),
          sourceCommit: ignoredCommit
        }),
      /does not exactly match tracked source files/i
    );
    rmSync(path.join(fixture.specDir, 'ignored.log'));

    symlinkSync('README.md', path.join(fixture.specDir, 'linked.md'));
    runGit(fixture.repoRoot, ['add', 'spec/coven-automations/v1/linked.md']);
    runGit(fixture.repoRoot, ['commit', '-m', 'test: add protocol symlink']);
    const symlinkCommit = runGit(fixture.repoRoot, ['rev-parse', 'HEAD']);
    assert.throws(
      () =>
        packageAutomationsProtocol({
          repoRoot: fixture.repoRoot,
          outputDir: path.join(scratchDir, 'symlink'),
          sourceCommit: symlinkCommit
        }),
      /must contain only regular files/i
    );
  });
});

test('verifies every bundled file, content digest, source commit, and archive digest', () => {
  withScratchDir('automation-protocol-verify', (scratchDir) => {
    const fixture = createFixtureRepository(scratchDir);
    const packaged = packageAutomationsProtocol({
      repoRoot: fixture.repoRoot,
      outputDir: path.join(scratchDir, 'out'),
      sourceCommit: fixture.sourceCommit
    });
    const verified = verifyAutomationsProtocolBundle({
      bundlePath: packaged.bundlePath,
      expectedSourceCommit: fixture.sourceCommit,
      expectedBundleSha256: packaged.bundleSha256
    });
    assert.equal(verified.contractContentSha256, packaged.contractContentSha256);
    assert.equal(verified.fileCount, 3);

    assert.throws(
      () =>
        verifyAutomationsProtocolBundle({
          bundlePath: packaged.bundlePath,
          expectedSourceCommit: 'f'.repeat(40),
          expectedBundleSha256: packaged.bundleSha256
        }),
      /source commit mismatch/i
    );
    assert.throws(
      () =>
        verifyAutomationsProtocolBundle({
          bundlePath: packaged.bundlePath,
          expectedSourceCommit: fixture.sourceCommit,
          expectedBundleSha256: 'f'.repeat(64)
        }),
      /bundle SHA-256 mismatch/i
    );

    const nonNormalizedGzip = Buffer.from(readFileSync(packaged.bundlePath));
    nonNormalizedGzip[9] = 0x03;
    const nonNormalizedGzipPath = path.join(scratchDir, 'non-normalized-gzip.tar.gz');
    writeFileSync(nonNormalizedGzipPath, nonNormalizedGzip);
    assert.throws(
      () =>
        verifyAutomationsProtocolBundle({
          bundlePath: nonNormalizedGzipPath,
          expectedSourceCommit: fixture.sourceCommit,
          expectedBundleSha256: sha256(nonNormalizedGzip)
        }),
      /gzip header is not normalized/i
    );

    const nonNormalizedTar = gunzipSync(readFileSync(packaged.bundlePath));
    const firstSize = Number.parseInt(
      nonNormalizedTar.subarray(124, 136).toString('utf8').replace(/\0.*$/s, '').trim(),
      8
    );
    nonNormalizedTar[512 + firstSize] = 1;
    const nonNormalizedPadding = gzipSync(nonNormalizedTar, { level: 9, mtime: 0 });
    nonNormalizedPadding.writeUInt32LE(0, 4);
    nonNormalizedPadding[9] = 0xff;
    const nonNormalizedPaddingPath = path.join(scratchDir, 'non-normalized-padding.tar.gz');
    writeFileSync(nonNormalizedPaddingPath, nonNormalizedPadding);
    assert.throws(
      () =>
        verifyAutomationsProtocolBundle({
          bundlePath: nonNormalizedPaddingPath,
          expectedSourceCommit: fixture.sourceCommit,
          expectedBundleSha256: sha256(nonNormalizedPadding)
        }),
      /tar padding is not normalized/i
    );

    const extraTerminatorTar = Buffer.concat([
      gunzipSync(readFileSync(packaged.bundlePath)),
      Buffer.alloc(512, 0)
    ]);
    const extraTerminatorBundle = gzipSync(extraTerminatorTar, { level: 9, mtime: 0 });
    extraTerminatorBundle.writeUInt32LE(0, 4);
    extraTerminatorBundle[9] = 0xff;
    const extraTerminatorPath = path.join(scratchDir, 'extra-terminator.tar.gz');
    writeFileSync(extraTerminatorPath, extraTerminatorBundle);
    assert.throws(
      () =>
        verifyAutomationsProtocolBundle({
          bundlePath: extraTerminatorPath,
          expectedSourceCommit: fixture.sourceCommit,
          expectedBundleSha256: sha256(extraTerminatorBundle)
        }),
      /tar has an invalid terminator/i
    );

    const tamperedTar = gunzipSync(readFileSync(packaged.bundlePath));
    const original = Buffer.from('{"b":2}\n');
    const replacement = Buffer.from('{"b":3}\n');
    const index = tamperedTar.indexOf(original);
    assert.notEqual(index, -1);
    replacement.copy(tamperedTar, index);
    const tamperedBundle = gzipSync(tamperedTar, { level: 9, mtime: 0 });
    tamperedBundle.writeUInt32LE(0, 4);
    tamperedBundle[9] = 0xff;
    const tamperedPath = path.join(scratchDir, 'tampered.tar.gz');
    writeFileSync(tamperedPath, tamperedBundle);
    assert.throws(
      () =>
        verifyAutomationsProtocolBundle({
          bundlePath: tamperedPath,
          expectedSourceCommit: fixture.sourceCommit,
          expectedBundleSha256: sha256(tamperedBundle)
        }),
      /file SHA-256 mismatch for b\.json/i
    );
  });
});

test('pinned TypeScript declarations expose the implemented event page result', () => {
  const declaration = readFileSync(
    path.join(repositoryRoot, 'spec', 'coven-automations', 'v1', 'coven.automations.v1.d.ts'),
    'utf8'
  );
  assert.match(declaration, /export interface EventRef \{/);
  assert.match(declaration, /eventRef\?: EventRef;/);
  assert.match(declaration, /export interface EventPage \{/);
  assert.match(declaration, /events: EventEnvelope\[\];/);
  assert.match(declaration, /nextAfter: number \| null;/);
  assert.match(declaration, /checkpointExpiresAt: Timestamp;/);
  assert.match(
    declaration,
    /C extends "events\.read\.v1" \| "events\.subscribe\.v1" \? EventPage : Record<string, unknown>/
  );
});

test('verify CLI refuses missing required arguments with actionable usage', () => {
  const result = spawnSync(process.execPath, [scriptPath, 'verify'], {
    cwd: repositoryRoot,
    encoding: 'utf8'
  });
  assert.equal(result.status, 1);
  assert.match(
    result.stderr,
    /Usage: package-automations-protocol\.mjs verify --bundle <archive> --source-commit <sha> --sha256 <digest>/
  );
  assert.doesNotMatch(result.stderr, /undefined/);
});
