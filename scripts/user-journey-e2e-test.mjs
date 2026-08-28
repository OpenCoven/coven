import assert from 'node:assert/strict';
import { chmodSync, statSync, existsSync, readFileSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';

import {
  assertSessionInspection,
  assertInvalidCwdFailure,
  buildJourneyEnv,
  createCommandRunner,
  createFakeCodexFixture,
  createGitShim,
  createJourneyLayout,
  createNodeShim,
  createScratchDir,
  initGitRepo,
  repoRoot,
  runPackagedUserJourney,
  spawnOptionsForCommand,
  windowsCommandInvocation
} from './user-journey-e2e.mjs';

function withScratch(label, fn) {
  const scratch = createScratchDir(path.join(repoRoot, 'target', 'script-test-scratch'), label);
  try {
    return fn(scratch);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

function makeResult({ status = 0, stdout = '', stderr = '' } = {}) {
  return { status, stdout, stderr };
}

function makeSessionInspectionArgs(events) {
  return {
    eventsOutput: JSON.stringify({ events }),
    logOutput: JSON.stringify([{ message: 'fake codex complete: E2E journey marker' }]),
    marker: 'E2E journey marker',
    sessionListOutput: JSON.stringify({
      sessions: [{ id: 'sess-1', harness: 'codex', status: 'completed', exit_code: 0 }]
    }),
    showOutput: JSON.stringify({
      id: 'sess-1',
      harness: 'codex',
      status: 'completed',
      exit_code: 0
    })
  };
}

function assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome) {
  assert.equal(options.cwd, layout.projectRoot);
  assert.equal(options.env.HOME, layout.userHome);
  assert.equal(options.env.USERPROFILE, layout.userHome);
  assert.equal(options.env.COVEN_HOME, layout.covenHome);
  assert.equal(options.env.GIT_CONFIG_GLOBAL, layout.gitGlobalConfigPath);
  assert.equal(options.env.GIT_CONFIG_NOSYSTEM, '1');
  assert.equal(options.env.GIT_TEMPLATE_DIR, layout.gitTemplateDir);
  assert.equal(options.env.PATH, [path.dirname(wrapperBin), layout.nodeShimDir].join(path.delimiter));
  assert.notEqual(options.env.HOME, maliciousHome);
  assert.notEqual(options.env.USERPROFILE, maliciousHome);
}

function scriptedJourneyBaseEnv(scratch, maliciousHome) {
  const systemBin = path.join(scratch, 'system-bin');
  mkdirSync(systemBin, { recursive: true });
  writeFileSync(path.join(systemBin, 'git'), 'fixture');
  return {
    ...process.env,
    HOME: maliciousHome,
    USERPROFILE: maliciousHome,
    GIT_CONFIG_GLOBAL: path.join(maliciousHome, '.gitconfig'),
    PATH: [systemBin, process.env.PATH ?? process.env.Path ?? ''].join(path.delimiter)
  };
}

function gitInitArgs(layout) {
  return [
    '-c',
    `core.hooksPath=${layout.gitHooksDir}`,
    '-c',
    `init.templateDir=${layout.gitTemplateDir}`,
    'init',
    '--initial-branch=main'
  ];
}

function gitCommitArgs(layout) {
  return [
    '-c',
    `core.hooksPath=${layout.gitHooksDir}`,
    '-c',
    'user.name=Coven User Journey',
    '-c',
    'user.email=user-journey@example.invalid',
    '-c',
    'commit.gpgsign=false',
    'commit',
    '--allow-empty',
    '-m',
    'init'
  ];
}

// `coven setup` is the one journey step expected to exit nonzero, so the
// scripted runner has to pin its options too: without `allowedExitCodes: [1]`
// the real journey aborts on the fail-closed path it is meant to assert.
function assertSetupStepOptions(options, layout) {
  assert.deepEqual(options.allowedExitCodes, [1]);
  assert.equal(options.cwd, layout.projectRoot);
  assert.equal(options.env.COVEN_HOME, layout.covenHome);
  assert.ok(
    options.env.PATH.split(path.delimiter).includes(layout.fixtureBinDir),
    'setup must run with the fake harness on PATH'
  );
}

function createScriptedRunner(script) {
  const queue = [...script];
  const calls = [];

  function take(method, command, args, options = {}) {
    const next = queue.shift();
    assert(next, `unexpected ${method} ${command} ${args.join(' ')}`);
    assert.equal(next.method, method, `expected ${next.method}, got ${method}`);
    if (next.command !== undefined) {
      assert.equal(command, next.command);
    }
    if (next.args !== undefined) {
      assert.deepEqual(args, next.args);
    }
    if (next.assertOptions) {
      next.assertOptions(options);
    }
    calls.push({ method, command, args, options });
    if (next.error) {
      throw next.error;
    }
    return next.result;
  }

  return {
    calls,
    runner: {
      platform: 'darwin',
      run() {
        throw new Error('run() should not be used by the scripted darwin runner');
      },
      runCapture(command, args, options) {
        return take('runCapture', command, args, options);
      },
      runDaemonStart(wrapperBin, env, options) {
        return take('runDaemonStart', wrapperBin, ['daemon', 'start'], { ...options, env });
      }
    },
    assertExhausted() {
      assert.deepEqual(queue, []);
    }
  };
}

test('createFakeCodexFixture writes executable Unix shims that log deterministically', { skip: process.platform === 'win32' }, () =>
  withScratch('fixture-unix', (scratch) => {
    const fixture = createFakeCodexFixture(path.join(scratch, 'bin'));
    const logPath = path.join(scratch, 'fixture-log.jsonl');
    const runner = createCommandRunner({ platform: process.platform });

    runner.runCapture(fixture.codexCommand, ['exec', '--', 'fixture marker'], {
      cwd: scratch,
      env: { ...process.env, COVEN_FAKE_FIXTURE_LOG: logPath }
    });
    const engine = runner.runCapture(fixture.engineCommand, ['auth', 'status', '--json'], {
      cwd: scratch,
      env: { ...process.env, COVEN_FAKE_FIXTURE_LOG: logPath }
    });

    const codexMode = statSync(fixture.codexCommand).mode & 0o777;
    const engineMode = statSync(fixture.engineCommand).mode & 0o777;
    assert.equal(codexMode, 0o755);
    assert.equal(engineMode, 0o755);
    assert.equal(engine.stdout, '{"loggedIn":true}\n');

    const entries = readFileSync(logPath, 'utf8')
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line));
    assert.deepEqual(entries.map((entry) => entry.kind), ['codex', 'coven-code']);
    assert.equal(entries[0].argv.at(-1), 'fixture marker');
  }));

test('fake Codex fixture reads a dash prompt from stdin', { skip: process.platform === 'win32' }, () =>
  withScratch('fixture-stdin', (scratch) => {
    const fixture = createFakeCodexFixture(path.join(scratch, 'bin'));
    const logPath = path.join(scratch, 'fixture-log.jsonl');
    const runner = createCommandRunner({ platform: process.platform });
    const result = runner.runCapture(fixture.codexCommand, ['exec', '-'], {
      cwd: scratch,
      env: { ...process.env, COVEN_FAKE_FIXTURE_LOG: logPath },
      spawnOptions: { input: 'stdin marker\n' }
    });

    assert.match(result.stdout, /fake codex complete: stdin marker/);
    const entry = JSON.parse(readFileSync(logPath, 'utf8').trim());
    assert.equal(entry.prompt, 'stdin marker');
  }));

test('createFakeCodexFixture writes Windows cmd shims without requiring a release package', () =>
  withScratch('fixture-win', (scratch) => {
    const nativeFixture = path.join(scratch, 'fixture.exe');
    writeFileSync(nativeFixture, 'fixture');
    const fixture = createFakeCodexFixture(path.join(scratch, 'bin'), {
      platform: 'win32',
      windowsNativeFixture: nativeFixture
    });
    assert.deepEqual(
      fixture.files.map((file) => path.basename(file)).sort(),
      ['codex.cmd', 'coven-code.exe']
    );
    assert.match(
      readFileSync(fixture.codexCommand, 'utf8'),
      /node_modules\\@openai\\codex\\bin\\codex\.js/
    );
    assert.equal(readFileSync(fixture.engineCommand, 'utf8'), 'fixture');
  }));

test('createNodeShim escapes percent characters in Windows batch files', () =>
  withScratch('node-shim-win', (scratch) => {
    const shim = createNodeShim(scratch, {
      nodePath: 'C:\\100% ready\\node.exe',
      platform: 'win32'
    });
    assert.equal(readFileSync(shim, 'utf8'), '@"C:\\100%% ready\\node.exe" %*\r\n');
  }));

test('Windows commands use an explicitly quoted cmd.exe invocation', () => {
  assert.equal(spawnOptionsForCommand({}, 'darwin').shell, false);
  assert.equal(spawnOptionsForCommand({}, 'win32').shell, false);
  assert.equal(spawnOptionsForCommand({}, 'win32').windowsHide, true);
  assert.equal(spawnOptionsForCommand({}, 'win32').windowsVerbatimArguments, true);

  const calls = [];
  const runner = createCommandRunner({
    logger: { log() {} },
    platform: 'win32',
    spawnSyncImpl(command, args, options) {
      calls.push({ command, args, options });
      return makeResult();
    }
  });

  const result = runner.runDaemonStart('C:\\wrapper path\\coven.cmd', { ComSpec: 'C:\\Windows\\cmd.exe', PATH: 'shim' }, {
    cwd: 'C:\\repo'
  });
  assert.equal(result, undefined);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, 'C:\\Windows\\cmd.exe');
  assert.deepEqual(calls[0].args.slice(0, 4), ['/d', '/v:off', '/s', '/c']);
  assert.equal(
    calls[0].args[4],
    '""%COVEN_JOURNEY_COMMAND_0%" "%COVEN_JOURNEY_COMMAND_1%" "%COVEN_JOURNEY_COMMAND_2%""'
  );
  assert.equal(calls[0].options.env.COVEN_JOURNEY_COMMAND_0, 'C:\\wrapper path\\coven.cmd');
  assert.equal(calls[0].options.env.COVEN_JOURNEY_COMMAND_1, 'daemon');
  assert.equal(calls[0].options.env.COVEN_JOURNEY_COMMAND_2, 'start');
  assert.equal(calls[0].options.shell, false);
  assert.equal(calls[0].options.windowsHide, true);
  assert.equal(calls[0].options.windowsVerbatimArguments, true);
  assert.equal(calls[0].options.stdio, 'inherit');
});

test('windowsCommandInvocation preserves percent characters outside shell text', () => {
  const invocation = windowsCommandInvocation(
    'C:\\100% ready\\coven.cmd',
    ['run', 'marker & %TEMP%'],
    { ComSpec: 'C:\\Windows\\cmd.exe' }
  );
  assert.equal(invocation.command, 'C:\\Windows\\cmd.exe');
  assert.equal(invocation.env.COVEN_JOURNEY_COMMAND_0, 'C:\\100% ready\\coven.cmd');
  assert.equal(invocation.env.COVEN_JOURNEY_COMMAND_2, 'marker & %TEMP%');
  assert.doesNotMatch(invocation.commandArgs.at(-1), /100%|TEMP/);
});

test('buildJourneyEnv strips inherited npm configuration', () =>
  withScratch('journey-env-isolation', (scratch) => {
    const wrapperBin = path.join(scratch, 'installed', 'coven');
    const layout = createJourneyLayout(path.join(scratch, 'journey-root'));
    mkdirSync(layout.scratchRoot, { recursive: true });
    const env = buildJourneyEnv({
      baseEnv: {
        NPM_CONFIG_CACHE: path.join(scratch, 'outside-cache'),
        PATH: process.env.PATH
      },
      layout,
      platform: process.platform,
      wrapperBin
    });

    assert.equal(env.NPM_CONFIG_CACHE, undefined);
  }));

test('runPackagedUserJourney rejects a pre-existing scratch root', () =>
  withScratch('journey-owned-root', (scratch) => {
    const wrapperBin = path.join(scratch, 'installed', 'coven');
    const journeyRoot = path.join(scratch, 'journey-root');
    const unrelatedFile = path.join(journeyRoot, 'keep.txt');
    mkdirSync(path.dirname(wrapperBin), { recursive: true });
    mkdirSync(journeyRoot, { recursive: true });
    writeFileSync(wrapperBin, '#!/bin/sh\n');
    writeFileSync(unrelatedFile, 'keep');

    assert.throws(
      () => runPackagedUserJourney({ scratchRoot: journeyRoot, wrapperBin }),
      /scratch root already exists/
    );
    assert.equal(readFileSync(unrelatedFile, 'utf8'), 'keep');
  }));

test(
  'initGitRepo ignores malicious caller HOME hooks and templates',
  { skip: process.platform === 'win32' },
  () =>
    withScratch('git-home-isolation', (scratch) => {
      const maliciousHome = path.join(scratch, 'malicious-home');
      const maliciousHooksDir = path.join(scratch, 'malicious-hooks');
      const maliciousTemplateDir = path.join(scratch, 'malicious-template');
      const leakMarker = path.join(scratch, 'hook-ran');
      const wrapperBin = path.join(scratch, 'installed', 'coven');
      const layout = createJourneyLayout(path.join(scratch, 'journey-root'));
      const baseEnv = {
        ...process.env,
        HOME: maliciousHome,
        USERPROFILE: maliciousHome
      };

      for (const directory of [
        maliciousHome,
        maliciousHooksDir,
        path.join(maliciousTemplateDir, 'hooks'),
        path.dirname(wrapperBin),
        layout.covenHome,
        layout.gitHooksDir,
        layout.gitTemplateDir,
        layout.nodeShimDir,
        layout.projectRoot,
        layout.userHome,
        layout.xdgConfigHome
      ]) {
        mkdirSync(directory, { recursive: true });
      }
      writeFileSync(wrapperBin, '#!/bin/sh\n');
      writeFileSync(
        path.join(maliciousHome, '.gitconfig'),
        `[core]\n\thooksPath = ${maliciousHooksDir}\n[init]\n\ttemplateDir = ${maliciousTemplateDir}\n`,
        'utf8'
      );
      for (const hookPath of [
        path.join(maliciousHooksDir, 'pre-commit'),
        path.join(maliciousTemplateDir, 'hooks', 'pre-commit')
      ]) {
        writeFileSync(hookPath, `#!/bin/sh\necho leaked > ${JSON.stringify(leakMarker)}\nexit 1\n`);
        chmodSync(hookPath, 0o755);
      }

      createNodeShim(layout.nodeShimDir, { platform: process.platform });
      const gitCommand = createGitShim(layout.nodeShimDir, {
        baseEnv,
        platform: process.platform
      });
      const env = buildJourneyEnv({
        baseEnv,
        gitBinDir: process.platform === 'win32' ? path.dirname(gitCommand) : undefined,
        layout,
        platform: process.platform,
        wrapperBin
      });
      const runner = createCommandRunner({ logger: { log() {} }, platform: process.platform });

      initGitRepo(runner, layout.projectRoot, { env, layout });

      assert.equal(existsSync(leakMarker), false);
      const head = runner.runCapture('git', ['rev-parse', 'HEAD'], {
        cwd: layout.projectRoot,
        env
      });
      assert.match(head.stdout, /^[0-9a-f]{40}\n$/);
    })
);

test(
  'createNodeShim preserves special Unix path characters in shell shims',
  { skip: process.platform === 'win32' },
  () =>
    withScratch('node-shim-quoting', (scratch) => {
      const weirdComponent = "special $paths `ticks` 'quotes' and spaces";
      const weirdDir = path.join(scratch, weirdComponent);
      const targetPath = path.join(weirdDir, 'shim target');
      mkdirSync(weirdDir, { recursive: true });
      writeFileSync(targetPath, "#!/bin/sh\nprintf 'shim-target:%s\\n' \"$1\"\n");
      chmodSync(targetPath, 0o755);

      const shim = createNodeShim(path.join(scratch, 'shim-bin'), {
        nodePath: targetPath,
        platform: process.platform
      });
      const runner = createCommandRunner({ logger: { log() {} }, platform: process.platform });
      const result = runner.runCapture(shim, ['quoted marker'], { cwd: scratch });

      assert.equal(result.stdout, 'shim-target:quoted marker\n');
    })
);

test(
  'createFakeCodexFixture preserves special Unix path characters in fixture shims',
  { skip: process.platform === 'win32' },
  () =>
    withScratch('fixture-shim-quoting', (scratch) => {
      const weirdComponent = "special $paths `ticks` 'quotes' and spaces";
      const weirdDir = path.join(scratch, weirdComponent);
      const fixtureScript = path.join(weirdDir, 'fake codex fixture.mjs');
      mkdirSync(weirdDir, { recursive: true });
      writeFileSync(
        fixtureScript,
        readFileSync(path.join(repoRoot, 'scripts', 'fixtures', 'fake-codex.mjs'), 'utf8'),
        'utf8'
      );

      const fixture = createFakeCodexFixture(path.join(scratch, 'bin'), {
        fixtureScript,
        platform: process.platform
      });
      const runner = createCommandRunner({ logger: { log() {} }, platform: process.platform });
      const result = runner.runCapture(fixture.codexCommand, ['exec', '--', 'quoted marker'], {
        cwd: scratch
      });

      assert.match(result.stdout, /fake codex complete: quoted marker/);
    })
);

test('runPackagedUserJourney sequences installed-wrapper commands and full lifecycle checks', () =>
  withScratch('journey-sequence', (scratch) => {
    const wrapperBin = path.join(scratch, 'installed', 'coven');
    const journeyRoot = path.join(scratch, 'journey-root');
    const layout = createJourneyLayout(journeyRoot);
    const maliciousHome = path.join(scratch, 'malicious-home');
    mkdirSync(path.dirname(wrapperBin), { recursive: true });
    writeFileSync(wrapperBin, '#!/bin/sh\n');

    const sessionEnvelope = {
      sessions: [
        {
          id: 'sess-1',
          harness: 'codex',
          status: 'completed',
          exit_code: 0,
          archived_at: null
        }
      ]
    };
    const archivedEnvelope = {
      sessions: [
        {
          id: 'sess-1',
          harness: 'codex',
          status: 'completed',
          exit_code: 0,
          archived_at: '2026-01-01T00:00:00Z'
        }
      ]
    };
    const script = createScriptedRunner([
      {
        method: 'runCapture',
        command: 'git',
        args: gitInitArgs(layout),
        assertOptions(options) {
          assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome);
        },
        result: makeResult()
      },
      {
        method: 'runCapture',
        command: 'git',
        args: gitCommitArgs(layout),
        assertOptions(options) {
          assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome);
        },
        result: makeResult()
      },
      { method: 'runCapture', command: wrapperBin, args: ['--version'], result: makeResult({ stdout: 'coven v1.2.3\n' }) },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['--help'],
        result: makeResult({
          stdout: ['Usage: coven', '', ...['doctor', 'run', 'sessions', 'attach', 'daemon', 'status', 'help'].map((name) => `  ${name}  summary`), ''].join('\n')
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['help', '--all', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            schemaVersion: 1,
            groups: [
              {
                id: 'start-and-launch',
                commands: [
                  ...['doctor', 'run', 'sessions', 'daemon', 'summon', 'archive', 'sacrifice'].map(
                    (name) => ({ name })
                  )
                ]
              }
            ]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor'],
        result: makeResult({
          status: 1,
          stdout: [
            'Coven doctor',
            'Set up at least one harness in this same shell',
            'Codex: coven setup codex',
            'Claude Code: coven setup claude',
            'GitHub Copilot CLI: coven setup copilot',
            'Doctor found problems; review the failing checks above.'
          ].join('\n')
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor'],
        result: makeResult({
          stdout: 'Coven doctor\n[OK] Codex\n[OK] /fake/coven-code\n'
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            ok: true,
            blocking: false,
            checks: [
              { id: 'harness:codex', status: 'pass' },
              { id: 'engine', status: 'pass' }
            ]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['setup', 'codex'],
        assertOptions(options) {
          assertSetupStepOptions(options, layout);
        },
        result: makeResult({ status: 1, stdout: 'Codex: non_tty\n' })
      },
      {
        method: 'runDaemonStart',
        command: wrapperBin,
        result: makeResult({ stdout: 'Coven daemon: running\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'status'],
        result: makeResult({ stdout: 'Coven daemon: running\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'status', '--json'],
        result: makeResult({
          stdout: JSON.stringify({ status: 'running', ok: true, pid: 123, socket: 'sock', started_at: 'now' })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['run', 'codex', 'E2E journey marker'],
        assertOptions(options) {
          writeFileSync(
            options.env.COVEN_FAKE_FIXTURE_LOG,
            `${JSON.stringify({
              kind: 'codex',
              argv: ['exec', '-'],
              cwd: options.cwd,
              prompt: 'E2E journey marker'
            })}\n`
          );
        },
        result: makeResult({ stdout: 'fake codex complete: E2E journey marker\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', '--json'],
        result: makeResult({ stdout: JSON.stringify(sessionEnvelope) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', 'show', 'sess-1', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            id: 'sess-1',
            harness: 'codex',
            status: 'completed',
            exit_code: 0
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', 'events', 'sess-1', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            events: [
              { seq: 1, payload_json: '{"text":"E2E journey marker"}' },
              { seq: 2, payload_json: '{"text":"fake codex complete: E2E journey marker"}' },
              { seq: 3, kind: 'exit', payload_json: '{"status":"completed","exitCode":0}' }
            ]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', 'log', 'sess-1', '--json'],
        result: makeResult({
          stdout: JSON.stringify([{ message: 'fake codex complete: E2E journey marker' }])
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['archive', 'sess-1'],
        result: makeResult({ stdout: 'archived session\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', '--json'],
        result: makeResult({ stdout: JSON.stringify({ sessions: [] }) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', '--all', '--json'],
        result: makeResult({ stdout: JSON.stringify(archivedEnvelope) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['summon', 'sess-1'],
        result: makeResult({ stdout: 'sess-1\nfake codex complete: E2E journey marker\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', '--json'],
        result: makeResult({ stdout: JSON.stringify(sessionEnvelope) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sacrifice', 'sess-1', '--yes'],
        result: makeResult({ stdout: 'sacrificed session\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', '--all', '--json'],
        result: makeResult({ stdout: JSON.stringify({ sessions: [] }) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['run', 'codex', 'outside root attempt', '--cwd', path.join(scratch, 'journey-root', 'o')],
        result: makeResult({ status: 1, stderr: 'failed to resolve cwd: cwd is outside the Coven project root' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'stop'],
        result: makeResult({ stdout: 'Coven daemon: stopped\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'status', '--json'],
        result: makeResult({ stdout: JSON.stringify({ status: 'stopped', ok: false }) })
      }
    ]);

    const result = runPackagedUserJourney({
      baseEnv: scriptedJourneyBaseEnv(scratch, maliciousHome),
      keepScratchDir: false,
      platform: 'darwin',
      runner: script.runner,
      scratchRoot: journeyRoot,
      wrapperBin
    });

    assert.equal(result.sessionId, 'sess-1');
    assert.equal(existsSync(journeyRoot), false);
    script.assertExhausted();
  }));

test('assertSessionInspection rejects missing terminal events after the completion marker', () => {
  assert.throws(
    () =>
      assertSessionInspection(
        makeSessionInspectionArgs([
          { seq: 1, payload_json: '{"text":"E2E journey marker"}' },
          { seq: 2, payload_json: '{"text":"fake codex complete: E2E journey marker"}' }
        ])
      ),
    /did not record a completed terminal payload/
  );
});

test('assertSessionInspection requires the supplied fixture log', () =>
  withScratch('fixture-log-required', (scratch) => {
    assert.throws(
      () =>
        assertSessionInspection({
          ...makeSessionInspectionArgs([
            { seq: 1, payload_json: '{"text":"fake codex complete: E2E journey marker"}' },
            { seq: 2, kind: 'exit', payload_json: '{"status":"completed","exitCode":0}' }
          ]),
          fixtureLogPath: path.join(scratch, 'missing.jsonl')
        }),
      /fixture log was not created/
    );
  }));

test('assertSessionInspection rejects terminal events that arrive before completion output', () => {
  assert.throws(
    () =>
      assertSessionInspection(
        makeSessionInspectionArgs([
          { seq: 1, kind: 'exit', payload_json: '{"status":"completed","exitCode":0}' },
          { seq: 2, payload_json: '{"text":"fake codex complete: E2E journey marker"}' }
        ])
      ),
    /output-before-terminal ordering/
  );
});

test('runPackagedUserJourney attempts bounded daemon stop after start failure', () =>
  withScratch('journey-daemon-start-failure', (scratch) => {
    const wrapperBin = path.join(scratch, 'installed', 'coven');
    const journeyRoot = path.join(scratch, 'journey-root');
    const layout = createJourneyLayout(journeyRoot);
    const maliciousHome = path.join(scratch, 'malicious-home');
    const requiredPublicCommands = ['doctor', 'run', 'sessions', 'daemon', 'summon', 'archive', 'sacrifice'];

    mkdirSync(path.dirname(wrapperBin), { recursive: true });
    writeFileSync(wrapperBin, '#!/bin/sh\n');

    const script = createScriptedRunner([
      {
        method: 'runCapture',
        command: 'git',
        args: gitInitArgs(layout),
        assertOptions(options) {
          assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome);
        },
        result: makeResult()
      },
      {
        method: 'runCapture',
        command: 'git',
        args: gitCommitArgs(layout),
        assertOptions(options) {
          assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome);
        },
        result: makeResult()
      },
      { method: 'runCapture', command: wrapperBin, args: ['--version'], result: makeResult({ stdout: 'coven v1.2.3\n' }) },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['--help'],
        result: makeResult({
          stdout: ['Usage: coven', '', ...['doctor', 'run', 'sessions', 'attach', 'daemon', 'status', 'help'].map((name) => `  ${name}  summary`), ''].join('\n')
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['help', '--all', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            schemaVersion: 1,
            groups: [{ commands: requiredPublicCommands.map((name) => ({ name })) }]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor'],
        result: makeResult({
          status: 1,
          stdout: [
            'Coven doctor',
            'Set up at least one harness in this same shell',
            'Codex: coven setup codex',
            'Claude Code: coven setup claude',
            'GitHub Copilot CLI: coven setup copilot',
            'Doctor found problems; review the failing checks above.'
          ].join('\n')
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor'],
        result: makeResult({ stdout: 'Coven doctor\n[OK] Codex\n[OK] /fake/coven-code\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            ok: true,
            blocking: false,
            checks: [
              { id: 'harness:codex', status: 'pass' },
              { id: 'engine', status: 'pass' }
            ]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['setup', 'codex'],
        assertOptions(options) {
          assertSetupStepOptions(options, layout);
        },
        result: makeResult({ status: 1, stdout: 'Codex: non_tty\n' })
      },
      {
        method: 'runDaemonStart',
        command: wrapperBin,
        error: new Error('daemon start timed out after 60000ms')
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'stop'],
        assertOptions(options) {
          assert.equal(options.cwd, layout.projectRoot);
          assert.equal(options.env.COVEN_HOME, layout.covenHome);
          assert.deepEqual(options.allowedExitCodes, [0, 1]);
          assert.equal(options.timeoutMs, 10_000);
        },
        error: new Error('cleanup stop timed out')
      }
    ]);

    assert.throws(
      () =>
        runPackagedUserJourney({
          baseEnv: scriptedJourneyBaseEnv(scratch, maliciousHome),
          platform: 'darwin',
          runner: script.runner,
          scratchRoot: journeyRoot,
          wrapperBin
        }),
      /daemon start timed out after 60000ms; daemon cleanup failed: cleanup stop timed out; scratch preserved at/
    );
    assert.equal(
      script.calls.filter((call) => call.command === wrapperBin && call.args[0] === 'daemon' && call.args[1] === 'stop')
        .length,
      1
    );
    assert.equal(existsSync(journeyRoot), true);
    script.assertExhausted();
  }));

test('runPackagedUserJourney stops the daemon and removes scratch after mid-journey failure', () =>
  withScratch('journey-cleanup', (scratch) => {
    const wrapperBin = path.join(scratch, 'installed', 'coven');
    const journeyRoot = path.join(scratch, 'journey-root');
    const layout = createJourneyLayout(journeyRoot);
    const maliciousHome = path.join(scratch, 'malicious-home');
    mkdirSync(path.dirname(wrapperBin), { recursive: true });
    writeFileSync(wrapperBin, '#!/bin/sh\n');
    const requiredPublicCommands = ['doctor', 'run', 'sessions', 'daemon', 'summon', 'archive', 'sacrifice'];

    const script = createScriptedRunner([
      {
        method: 'runCapture',
        command: 'git',
        args: gitInitArgs(layout),
        assertOptions(options) {
          assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome);
        },
        result: makeResult()
      },
      {
        method: 'runCapture',
        command: 'git',
        args: gitCommitArgs(layout),
        assertOptions(options) {
          assertHermeticGitOptions(options, layout, wrapperBin, maliciousHome);
        },
        result: makeResult()
      },
      { method: 'runCapture', command: wrapperBin, args: ['--version'], result: makeResult({ stdout: 'coven v1.2.3\n' }) },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['--help'],
        result: makeResult({
          stdout: ['Usage: coven', '', ...['doctor', 'run', 'sessions', 'attach', 'daemon', 'status', 'help'].map((name) => `  ${name}  summary`), ''].join('\n')
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['help', '--all', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            schemaVersion: 1,
            groups: [{ commands: requiredPublicCommands.map((name) => ({ name })) }]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor'],
        result: makeResult({
          status: 1,
          stdout: [
            'Coven doctor',
            'Set up at least one harness in this same shell',
            'Codex: coven setup codex',
            'Claude Code: coven setup claude',
            'GitHub Copilot CLI: coven setup copilot',
            'Doctor found problems; review the failing checks above.'
          ].join('\n')
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor'],
        result: makeResult({ stdout: 'Coven doctor\n[OK] Codex\n[OK] /fake/coven-code\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['doctor', '--json'],
        result: makeResult({
          stdout: JSON.stringify({
            ok: true,
            blocking: false,
            checks: [
              { id: 'harness:codex', status: 'pass' },
              { id: 'engine', status: 'pass' }
            ]
          })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['setup', 'codex'],
        assertOptions(options) {
          assertSetupStepOptions(options, layout);
        },
        result: makeResult({ status: 1, stdout: 'Codex: non_tty\n' })
      },
      {
        method: 'runDaemonStart',
        command: wrapperBin,
        result: makeResult({ stdout: 'Coven daemon: running\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'status'],
        result: makeResult({ stdout: 'Coven daemon: running\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'status', '--json'],
        result: makeResult({
          stdout: JSON.stringify({ status: 'running', ok: true, pid: 123, socket: 'sock', started_at: 'now' })
        })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['run', 'codex', 'E2E journey marker'],
        result: makeResult({ stdout: 'fake codex complete: E2E journey marker\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', '--json'],
        result: makeResult({ stdout: JSON.stringify({ sessions: [{ id: 'sess-1', harness: 'codex', status: 'completed', exit_code: 0, archived_at: null }] }) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', 'show', 'sess-1', '--json'],
        result: makeResult({ stdout: JSON.stringify({ id: 'sess-1', harness: 'codex', status: 'completed', exit_code: 0 }) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', 'events', 'sess-1', '--json'],
        result: makeResult({ stdout: JSON.stringify({ events: [] }) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['sessions', 'log', 'sess-1', '--json'],
        result: makeResult({ stdout: JSON.stringify([]) })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'stop'],
        result: makeResult({ stdout: 'Coven daemon: stopped\n' })
      },
      {
        method: 'runCapture',
        command: wrapperBin,
        args: ['daemon', 'status', '--json'],
        assertOptions(options) {
          assert.equal(options.timeoutMs, 10_000);
        },
        result: makeResult({
          stdout: JSON.stringify({ status: 'stopped', ok: false })
        })
      }
    ]);

    assert.throws(
      () =>
        runPackagedUserJourney({
          baseEnv: scriptedJourneyBaseEnv(scratch, maliciousHome),
          platform: 'darwin',
          runner: script.runner,
          scratchRoot: journeyRoot,
          wrapperBin
        }),
      /returned no events/
    );
    assert.equal(existsSync(journeyRoot), false);
    assert.equal(
      script.calls.filter((call) => call.command === wrapperBin && call.args[0] === 'daemon' && call.args[1] === 'stop')
        .length,
      1
    );
    script.assertExhausted();
  }));

test('assertInvalidCwdFailure rejects unbounded or non-actionable errors', () => {
  assert.doesNotThrow(() =>
    assertInvalidCwdFailure(
      makeResult({
        status: 1,
        stderr: 'failed to resolve cwd: cwd is outside the Coven project root'
      })
    )
  );
  assert.throws(
    () => assertInvalidCwdFailure(makeResult({ status: 1, stderr: 'bad cwd' })),
    /failed to resolve cwd/
  );
});
