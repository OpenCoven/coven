import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, symlinkSync } from 'node:fs';
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

// A CLI started before a top-level `const`, `let` or `class` runs synchronously
// until its first `await` while that binding is still uninitialized. v0.4.8's
// GitHub Release step failed exactly so (#1204). Every entrypoint must start
// its CLI after its last such declaration.
const cliStart = /^if \((?:isMainModule\(|process\.argv\[1\]|import\.meta\.url ===)/m;
// The binding is a name or an object or array destructuring pattern.
const topLevelBinding = /^(?:export\s+)?(?:const|let|class)\s+(\w+|\{[^}]*\}|\[[^\]]*\])/gm;

function lateBindings(source) {
  const start = source.search(cliStart);
  return start === -1 ? [] : [...source.slice(start).matchAll(topLevelBinding)].map((match) => match[1]);
}

test('the late-binding scan sees every top-level binding form', () => {
  const cli = 'if (isMainModule()) {\n  main();\n}\n';
  assert.deepEqual(lateBindings(
    `${cli}const plain = 1;\nlet counter = 0;\nclass Shape {}\nexport const exported = 2;\n` +
    'const { value, other: renamed } = source;\nlet [first, second] = pair;\nconst {\n  multi\n} = nested;\n'
  ), ['plain', 'counter', 'Shape', 'exported', '{ value, other: renamed }', '[first, second]', '{\n  multi\n}']);
  assert.deepEqual(lateBindings(`const { value } = source;\nconst [first] = pair;\n${cli}`), []);
  assert.deepEqual(lateBindings(`${cli}function hoisted() {}\n  const nested = 1;\n`), []);
});

test('entrypoint scripts start their CLI after every top-level binding', () => {
  const scriptsDirectory = path.join(repositoryRoot, 'scripts');
  const entrypoints = readdirSync(scriptsDirectory)
    .filter((name) => name.endsWith('.mjs'))
    .map((name) => ({ name, source: readFileSync(path.join(scriptsDirectory, name), 'utf8') }))
    .filter(({ source }) => cliStart.test(source));
  assert.ok(
    entrypoints.some(({ name }) => name === 'package-github-release.mjs'),
    'the entrypoint scan must find the release packager'
  );
  for (const { name, source } of entrypoints) {
    const late = lateBindings(source);
    assert.deepEqual(late, [], `${name} declares ${late.join(', ')} after starting its CLI`);
  }
});
