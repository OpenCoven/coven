import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const fixturesRoot = path.join(repoRoot, 'target', 'export-cli-help-contract-tests');
const scriptPath = path.join(repoRoot, 'scripts', 'export-cli-help-contract.mjs');

const VALID_FIXTURE = {
  schemaVersion: 1,
  groups: [
    {
      id: 'start-and-launch',
      title: 'Start and launch',
      commands: [
        {
          name: 'doctor',
          summary: 'Check local setup and print next steps',
          docsUrl: 'https://docs.opencoven.ai/docs/cli/doctor',
        },
        {
          name: 'help',
          summary: 'Show concise help, every public command, or help for one command',
          docsUrl: 'https://docs.opencoven.ai/docs/cli/interactive',
        },
      ],
    },
    {
      id: 'observe-your-coven',
      title: 'Observe your coven',
      commands: [
        {
          name: 'skills',
          summary: 'List installed skills from ~/.coven/skills/',
          docsUrl: 'https://docs.opencoven.ai/docs/cli/observe',
        },
      ],
    },
  ],
};

let counter = 0;

function testDir(name) {
  const safeName = name.replace(/[^a-z0-9-]+/gi, '-').toLowerCase();
  const directory = path.join(fixturesRoot, `${safeName}-${process.pid}-${counter++}`);
  rmSync(directory, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
  return directory;
}

function pushFlag(args, flagName, value, form) {
  if (form === 'equals') {
    args.push(`${flagName}=${value}`);
    return;
  }
  args.push(flagName, value);
}

function writeFakeBinary(directory) {
  const fixtureBinary = path.join(directory, 'fake coven help.mjs');
  writeFileSync(
    fixtureBinary,
    `import { readFileSync } from 'node:fs';

const args = JSON.stringify(process.argv.slice(2));
const expected = JSON.stringify(['help', '--all', '--json']);
if (args !== expected) {
  console.error(\`unexpected args: \${args}\`);
  process.exit(2);
}

const fixturePath = process.env.COVEN_FAKE_HELP_FIXTURE;
if (!fixturePath) {
  console.error('missing COVEN_FAKE_HELP_FIXTURE');
  process.exit(3);
}

process.stdout.write(readFileSync(fixturePath, 'utf8'));
if (process.env.COVEN_FAKE_HELP_STDERR) {
  process.stderr.write(process.env.COVEN_FAKE_HELP_STDERR);
}
process.exit(Number(process.env.COVEN_FAKE_HELP_EXIT ?? 0));
`,
  );
  return fixtureBinary;
}

function runExport({
  fixture,
  binaryForm = 'split',
  nodeFlagForm = 'equals',
  scriptArgForm = 'split',
  outputForm = 'split',
  name,
  env = {},
}) {
  const directory = testDir(name);
  const binaryPath = writeFakeBinary(directory);
  const fixturePath = path.join(directory, 'fixture.json');
  const outputPath = path.join(directory, 'out dir', 'coven cli help.json');
  writeFileSync(fixturePath, typeof fixture === 'string' ? fixture : JSON.stringify(fixture));

  const args = [];
  if (binaryForm === 'equals') {
    args.push(`--binary=${process.execPath}`);
  } else {
    args.push('--binary', process.execPath);
  }

  pushFlag(args, '--binary-arg', '--no-warnings', nodeFlagForm);
  pushFlag(args, '--binary-arg', binaryPath, scriptArgForm);

  pushFlag(args, '--output', outputPath, outputForm);

  const result = spawnSync(process.execPath, [scriptPath, ...args], {
    cwd: repoRoot,
    encoding: 'utf8',
    env: {
      ...process.env,
      COVEN_FAKE_HELP_FIXTURE: fixturePath,
      ...env,
    },
  });

  return { directory, outputPath, result };
}

test('exports a deterministic pretty contract with a trailing newline', () => {
  const first = runExport({
    fixture: {
      groups: VALID_FIXTURE.groups,
      schemaVersion: 1,
    },
    binaryForm: 'equals',
    nodeFlagForm: 'equals',
    scriptArgForm: 'split',
    outputForm: 'split',
    name: 'success first with spaces',
  });
  assert.equal(first.result.status, 0, first.result.stderr);
  assert.equal(
    readFileSync(first.outputPath, 'utf8'),
    `${JSON.stringify(VALID_FIXTURE, null, 2)}\n`,
  );

  const second = runExport({
    fixture: JSON.stringify({
      schemaVersion: 1,
      groups: VALID_FIXTURE.groups,
    }),
    binaryForm: 'split',
    nodeFlagForm: 'split',
    scriptArgForm: 'equals',
    outputForm: 'equals',
    name: 'success-second',
  });
  assert.equal(second.result.status, 0, second.result.stderr);
  assert.equal(readFileSync(second.outputPath, 'utf8'), readFileSync(first.outputPath, 'utf8'));
});

test('rejects duplicate command names', () => {
  const { result } = runExport({
    fixture: {
      schemaVersion: 1,
      groups: [
        {
          id: 'one',
          title: 'One',
          commands: [
            { name: 'doctor', summary: 'First', docsUrl: 'https://docs.opencoven.ai/docs/cli/doctor' },
          ],
        },
        {
          id: 'two',
          title: 'Two',
          commands: [
            { name: 'doctor', summary: 'Second', docsUrl: 'https://docs.opencoven.ai/docs/cli/run' },
          ],
        },
      ],
    },
    name: 'duplicate-command',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /duplicate command name: doctor/);
});

test('rejects unsupported schema versions', () => {
  const { result } = runExport({
    fixture: {
      schemaVersion: 2,
      groups: VALID_FIXTURE.groups,
    },
    name: 'invalid-schema',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /schemaVersion must be 1/);
});

test('rejects leaked internal commands', () => {
  const { result } = runExport({
    fixture: {
      schemaVersion: 1,
      groups: [
        {
          id: 'ops',
          title: 'Ops',
          commands: [
            {
              name: 'process-supervisor',
              summary: 'Internal only',
              docsUrl: 'https://docs.opencoven.ai/docs/cli/daemon',
            },
          ],
        },
      ],
    },
    name: 'internal-command',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /internal command leaked into public help/);
});

test('rejects unstable docs URLs', () => {
  const { result } = runExport({
    fixture: {
      schemaVersion: 1,
      groups: [
        {
          id: 'bad-url',
          title: 'Bad URL',
          commands: [
            {
              name: 'doctor',
              summary: 'Check local setup',
              docsUrl: 'https://example.com/docs/cli/doctor?preview=1',
            },
          ],
        },
      ],
    },
    name: 'bad-url',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /stable https:\/\/docs\.opencoven\.ai origin/);
});

test('rejects ANSI escape sequences', () => {
  const ansiFixture = JSON.stringify({
    schemaVersion: 1,
    groups: [
      {
        id: 'ansi',
        title: 'ANSI',
        commands: [
          {
            name: 'doctor',
            summary: '\u001b[31mCheck local setup\u001b[0m',
            docsUrl: 'https://docs.opencoven.ai/docs/cli/doctor',
          },
        ],
      },
    ],
  });
  const { result } = runExport({
    fixture: ansiFixture,
    name: 'ansi',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /ANSI escape sequences/);
});

test('rejects machine-specific path leakage', () => {
  const { result } = runExport({
    fixture: {
      schemaVersion: 1,
      groups: [
        {
          id: 'paths',
          title: 'Paths',
          commands: [
            {
              name: 'doctor',
              summary: 'Read /var/db/coven/private.sock before continuing',
              docsUrl: 'https://docs.opencoven.ai/docs/cli/doctor',
            },
          ],
        },
      ],
    },
    name: 'path-leak',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /machine-specific path/);
});

test('rejects Windows machine-specific path leakage', () => {
  const { result } = runExport({
    fixture: {
      schemaVersion: 1,
      groups: [
        {
          id: 'windows-paths',
          title: 'Windows paths',
          commands: [
            {
              name: 'doctor',
              summary: 'Read C:\\Users\\example\\coven\\private.sock before continuing',
              docsUrl: 'https://docs.opencoven.ai/docs/cli/doctor',
            },
          ],
        },
      ],
    },
    name: 'windows-path-leak',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /machine-specific path/);
});
