import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, realpathSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repositoryRoot = realpathSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..'));
const scripts = [
  { name: 'user-journey-e2e.mjs', args: [], error: /usage:.*--wrapper-bin=/ },
  { name: 'package-automations-protocol.mjs', args: ['verify'], error: /Usage:.*verify --bundle/ },
  { name: 'package-automations-authority-profile.mjs', args: ['verify'], error: /Usage:.*verify --bundle/ }
];

function scratchDirectory(t) {
  const directory = realpathSync(mkdtempSync(path.join(tmpdir(), 'coven-entrypoint-')));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

function assertArgumentFailure(entrypoint, script, cwd) {
  const result = spawnSync(process.execPath, [entrypoint, ...script.args], {
    cwd,
    encoding: 'utf8',
    timeout: 30_000
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, `${entrypoint} must execute its argument guard\n${result.stderr}`);
  assert.equal(result.stdout, '');
  assert.match(result.stderr, script.error);
}

for (const script of scripts) {
  const original = path.join(repositoryRoot, 'scripts', script.name);

  test(`${script.name}: canonical and relative CLI paths reject missing arguments`, () => {
    assertArgumentFailure(original, script, repositoryRoot);
    assertArgumentFailure(path.join('scripts', script.name), script, repositoryRoot);
  });

  test(`${script.name}: directory aliases execute the CLI`, (t) => {
    const directory = scratchDirectory(t);
    const alias = path.join(directory, 'checkout alias #1');
    symlinkSync(repositoryRoot, alias, 'junction');
    assertArgumentFailure(path.join(alias, 'scripts', script.name), script, directory);
  });

  test(`${script.name}: symlinked entrypoints execute the CLI`, {
    skip: process.platform === 'win32' ? 'file symlinks require Windows developer privileges' : false
  }, (t) => {
    const directory = scratchDirectory(t);
    const alias = path.join(directory, script.name);
    symlinkSync(original, alias);
    assertArgumentFailure(alias, script, directory);
  });

  test(`${script.name}: importing does not execute the CLI`, (t) => {
    const directory = scratchDirectory(t);
    for (const argv of [
      undefined,
      '-',
      path.join(directory, 'missing'),
      path.join(original, 'not-a-directory'),
      fileURLToPath(import.meta.url)
    ]) {
      const result = spawnSync(process.execPath, ['--input-type=module', '-e',
        `process.argv[1] = ${JSON.stringify(argv)};
         await import(${JSON.stringify(pathToFileURL(original).href)});
         console.log("import-safe");`
      ], { cwd: directory, encoding: 'utf8', timeout: 30_000 });
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, 'import-safe\n');
      assert.equal(result.stderr, '');
    }
  });
}
