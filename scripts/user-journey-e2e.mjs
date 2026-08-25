#!/usr/bin/env node
// Hermetic packaged-binary user journey smoke for the npm-distributed Coven CLI.
//
// This script assumes the wrapper + native package are already installed. It:
//   1. isolates HOME/USERPROFILE/COVEN_HOME/PATH plus a temporary git repo,
//   2. verifies concise help and `help --all --json`,
//   3. verifies first-run doctor guidance with no harness on PATH,
//   4. injects a deterministic fake Codex + coven-code engine fixture,
//   5. exercises daemon lifecycle, a real packaged `coven run codex ...` turn,
//   6. inspects sessions/show/events/log, archive/summon/sacrifice, and
//   7. verifies bounded outside-root `--cwd` rejection plus daemon cleanup.

import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
export const repoRoot = path.resolve(__dirname, '..');
export const DEFAULT_COMMAND_TIMEOUT_MS = 60_000;

const CURATED_COMMANDS = ['doctor', 'run', 'sessions', 'attach', 'daemon', 'status', 'help'];
const REQUIRED_PUBLIC_COMMANDS = ['doctor', 'run', 'sessions', 'daemon', 'summon', 'archive', 'sacrifice'];
const HIDDEN_HELP_COMMANDS = ['chat', 'config', 'process-supervisor', 'daemon serve'];
const STRIP_ENV_PREFIXES = [
  'ANTHROPIC_',
  'AWS_',
  'AZURE_',
  'CLAUDE_CODE_',
  'COPILOT_',
  'COVEN_HARNESS_ADAPTER_',
  'GIT_',
  'GOOGLE_',
  'GROQ_',
  'NPM_CONFIG_',
  'OPENAI_',
  'OPENROUTER_',
  'XAI_'
];
const STRIP_ENV_EXACT = [
  'COVEN_ENGINE_BIN',
  'COVEN_HOME',
  'COVEN_SETTINGS_PATH',
  'GH_TOKEN',
  'GITHUB_TOKEN',
  'HOME',
  'HOMEDRIVE',
  'HOMEPATH',
  'NODE_AUTH_TOKEN',
  'NPM_TOKEN',
  'PATH',
  'Path',
  'USERPROFILE',
  'XDG_CONFIG_HOME'
];

export function isMainModule(importMetaUrl) {
  return Boolean(process.argv[1]) && path.resolve(fileURLToPath(importMetaUrl)) === path.resolve(process.argv[1]);
}

export function fail(message) {
  throw new Error(message);
}

export function createScratchDir(baseDir, label, options = {}) {
  const pid = options.pid ?? process.pid;
  const now = options.now ?? Date.now;
  mkdirSync(baseDir, { recursive: true });
  const seed = now();
  for (let counter = 0; counter < 100; counter += 1) {
    const candidate = path.join(baseDir, `${label}-${pid}-${seed}-${counter}`);
    try {
      mkdirSync(candidate);
      return candidate;
    } catch (error) {
      if (error?.code !== 'EEXIST') {
        throw error;
      }
    }
  }
  fail(`could not allocate scratch directory under ${baseDir}`);
}

export function spawnOptionsForCommand(options = {}, platform = process.platform) {
  return {
    shell: false,
    windowsHide: platform === 'win32',
    windowsVerbatimArguments: platform === 'win32',
    ...options.spawnOptions
  };
}

export function windowsCommandInvocation(command, commandArgs, baseEnv = {}) {
  const prefix = 'COVEN_JOURNEY_COMMAND_';
  const env = Object.fromEntries(
    Object.entries(baseEnv).filter(([name]) => !name.toUpperCase().startsWith(prefix))
  );
  const values = [command, ...commandArgs].map((value) => {
    const argument = String(value);
    if (/[\0\r\n"]/.test(argument)) {
      fail(`unsupported character in Windows command argument: ${JSON.stringify(argument)}`);
    }
    return argument;
  });
  const references = values.map((value, index) => {
    const name = `${prefix}${index}`;
    env[name] = value;
    return `"%${name}%"`;
  });
  return {
    command: env.ComSpec ?? env.COMSPEC ?? 'cmd.exe',
    commandArgs: ['/d', '/v:off', '/s', '/c', `"${references.join(' ')}"`],
    env
  };
}

export function createCommandRunner({
  repoRoot: commandRepoRoot = repoRoot,
  platform = process.platform,
  logger = console,
  spawnSyncImpl = spawnSync
} = {}) {
  function spawnInvocation(command, commandArgs, env) {
    if (platform !== 'win32') {
      return { command, commandArgs, env };
    }
    return windowsCommandInvocation(command, commandArgs, env);
  }

  function run(command, commandArgs, options = {}) {
    const printable = [command, ...commandArgs].join(' ');
    logger.log?.(`$ ${printable}`);
    const env = options.env ?? process.env;
    const invocation = spawnInvocation(command, commandArgs, env);
    const result = spawnSyncImpl(invocation.command, invocation.commandArgs, {
      ...spawnOptionsForCommand(options, platform),
      cwd: options.cwd ?? commandRepoRoot,
      env: invocation.env,
      stdio: 'inherit',
      timeout: options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS
    });
    handleSpawnResult(printable, result, options, { capture: false });
    return result;
  }

  function runCapture(command, commandArgs, options = {}) {
    const printable = [command, ...commandArgs].join(' ');
    logger.log?.(`$ ${printable}`);
    const env = options.env ?? process.env;
    const invocation = spawnInvocation(command, commandArgs, env);
    const result = spawnSyncImpl(invocation.command, invocation.commandArgs, {
      ...spawnOptionsForCommand(options, platform),
      cwd: options.cwd ?? commandRepoRoot,
      env: invocation.env,
      stdio: [
        options.spawnOptions?.input === undefined ? 'ignore' : 'pipe',
        'pipe',
        'pipe'
      ],
      encoding: 'utf8',
      timeout: options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS
    });
    handleSpawnResult(printable, result, options, { capture: true });
    return result;
  }

  function runDaemonStart(wrapperBin, env, options = {}) {
    if (platform === 'win32') {
      run(wrapperBin, ['daemon', 'start'], { ...options, env });
      return undefined;
    }
    return runCapture(wrapperBin, ['daemon', 'start'], { ...options, env });
  }

  return { platform, logger, run, runCapture, runDaemonStart };
}

function handleSpawnResult(printable, result, options, { capture }) {
  const stdout = textFromResult(result, 'stdout');
  const stderr = textFromResult(result, 'stderr');
  const timeoutMs = options.timeoutMs ?? DEFAULT_COMMAND_TIMEOUT_MS;
  if (result.error?.code === 'ETIMEDOUT') {
    fail(
      `${printable} timed out after ${timeoutMs}ms` +
        (capture ? `\nstdout:\n${stdout}\nstderr:\n${stderr}` : '')
    );
  }
  if (result.error) {
    fail(`${printable} failed: ${result.error.message}`);
  }
  if (result.status !== 0 && !(options.allowedExitCodes ?? []).includes(result.status)) {
    fail(
      `${printable} exited with ${result.status}` +
        (capture ? `\nstdout:\n${stdout}\nstderr:\n${stderr}` : '')
    );
  }
}

function textFromResult(result, field) {
  const value = result?.[field];
  if (typeof value === 'string') {
    return value;
  }
  if (Buffer.isBuffer(value)) {
    return value.toString('utf8');
  }
  return value ? String(value) : '';
}

export function createJourneyLayout(scratchRoot) {
  return {
    scratchRoot,
    covenHome: path.join(scratchRoot, 'h'),
    fixtureBinDir: path.join(scratchRoot, 'f'),
    fixtureLogPath: path.join(scratchRoot, 'l.jsonl'),
    gitGlobalConfigPath: path.join(scratchRoot, 'g'),
    gitHooksDir: path.join(scratchRoot, 'k'),
    gitTemplateDir: path.join(scratchRoot, 't'),
    nodeShimDir: path.join(scratchRoot, 'n'),
    outsideRoot: path.join(scratchRoot, 'o'),
    projectRoot: path.join(scratchRoot, 'p'),
    userHome: path.join(scratchRoot, 'u'),
    xdgConfigHome: path.join(scratchRoot, 'x')
  };
}

function createCompactJourneyScratchRoot(baseDir = repoRoot) {
  for (let counter = 0; counter < 100; counter += 1) {
    const candidate = path.join(baseDir, `.j${counter.toString(36)}`);
    try {
      mkdirSync(candidate);
      return candidate;
    } catch (error) {
      if (error?.code !== 'EEXIST') {
        throw error;
      }
    }
  }
  fail(`could not allocate compact journey scratch directory under ${baseDir}`);
}

function ensureJourneyLayout(layout) {
  for (const directory of [
    layout.covenHome,
    layout.fixtureBinDir,
    layout.gitHooksDir,
    layout.gitTemplateDir,
    layout.nodeShimDir,
    layout.outsideRoot,
    layout.projectRoot,
    layout.userHome,
    layout.xdgConfigHome
  ]) {
    mkdirSync(directory, { recursive: true });
  }
}

export function createNodeShim(nodeShimDir, { nodePath = process.execPath, platform = process.platform } = {}) {
  mkdirSync(nodeShimDir, { recursive: true });
  if (platform === 'win32') {
    const shim = path.join(nodeShimDir, 'node.cmd');
    writeFileSync(shim, windowsCommandShim(nodePath));
    return shim;
  }
  const shim = path.join(nodeShimDir, 'node');
  writeFileSync(shim, unixCommandShim(nodePath));
  chmodSync(shim, 0o755);
  return shim;
}

export function createGitShim(shimDir, { baseEnv = process.env, platform = process.platform } = {}) {
  const gitPath = resolveExecutableOnPath('git', { baseEnv, platform });
  if (platform === 'win32') {
    return gitPath;
  }
  const shim = path.join(shimDir, 'git');
  writeFileSync(shim, unixCommandShim(gitPath));
  chmodSync(shim, 0o755);
  return shim;
}

function posixSingleQuote(text) {
  return `'${String(text).replace(/'/g, `'\\''`)}'`;
}

function unixCommandShim(targetPath) {
  return `#!/bin/sh\nexec ${posixSingleQuote(targetPath)} "$@"\n`;
}

function windowsCommandShim(targetPath) {
  return `@"${targetPath.replaceAll('%', '%%')}" %*\r\n`;
}

export function createFakeCodexFixture(
  binDir,
  {
    fixtureScript = path.join(__dirname, 'fixtures', 'fake-codex.mjs'),
    nodePath = process.execPath,
    platform = process.platform,
    baseEnv = process.env,
    architecture = process.arch,
    windowsNativeFixture
  } = {}
) {
  mkdirSync(binDir, { recursive: true });
  if (platform === 'win32') {
    const target =
      architecture === 'arm64'
        ? {
            cpu: 'arm64',
            packageName: '@openai/codex-win32-arm64',
            triple: 'aarch64-pc-windows-msvc'
          }
        : {
            cpu: 'x64',
            packageName: '@openai/codex-win32-x64',
            triple: 'x86_64-pc-windows-msvc'
          };
    const packageRoot = path.join(binDir, 'node_modules', '@openai', 'codex');
    const packageBin = path.join(packageRoot, 'bin');
    const targetRoot = path.join(
      packageRoot,
      'node_modules',
      '@openai',
      target.packageName.slice('@openai/'.length)
    );
    const nativeBin = path.join(targetRoot, 'vendor', target.triple, 'bin');
    mkdirSync(packageBin, { recursive: true });
    mkdirSync(nativeBin, { recursive: true });
    const nativeCodex = path.join(nativeBin, 'codex.exe');
    if (windowsNativeFixture) {
      copyFileSync(windowsNativeFixture, nativeCodex);
    } else {
      const rustc = resolveExecutableOnPath('rustc', { baseEnv, platform });
      const source = path.join(__dirname, 'fixtures', 'fake-harness-windows.rs');
      const printable = `${rustc} --edition=2021 ${source} -o ${nativeCodex}`;
      const compile = spawnSync(rustc, ['--edition=2021', source, '-o', nativeCodex], {
        cwd: repoRoot,
        env: baseEnv,
        shell: false,
        stdio: 'inherit',
        timeout: DEFAULT_COMMAND_TIMEOUT_MS
      });
      handleSpawnResult(printable, compile, {}, { capture: false });
    }
    writeFileSync(path.join(packageBin, 'codex.js'), '// validated fixture entry\n');
    writeFileSync(
      path.join(packageRoot, 'package.json'),
      `${JSON.stringify({
        name: '@openai/codex',
        bin: { codex: 'bin/codex.js' },
        optionalDependencies: { [target.packageName]: '0.0.0' }
      })}\n`
    );
    writeFileSync(
      path.join(targetRoot, 'package.json'),
      `${JSON.stringify({
        name: '@openai/codex',
        os: ['win32'],
        cpu: [target.cpu]
      })}\n`
    );
    const codexCommand = path.join(binDir, 'codex.cmd');
    const engineCommand = path.join(binDir, 'coven-code.exe');
    writeFileSync(
      codexCommand,
      '@"%~dp0\\node_modules\\@openai\\codex\\bin\\codex.js" %*\r\n'
    );
    copyFileSync(nativeCodex, engineCommand);
    return {
      binDir,
      codexCommand,
      engineCommand,
      files: [codexCommand, engineCommand]
    };
  }

  const codexCommand = path.join(binDir, 'codex');
  const engineCommand = path.join(binDir, 'coven-code');
  writeFileSync(codexCommand, unixShim(nodePath, fixtureScript, 'codex'));
  writeFileSync(engineCommand, unixShim(nodePath, fixtureScript, 'coven-code'));
  chmodSync(codexCommand, 0o755);
  chmodSync(engineCommand, 0o755);
  return {
    binDir,
    codexCommand,
    engineCommand,
    files: [codexCommand, engineCommand]
  };
}

function unixShim(nodePath, fixtureScript, kind) {
  return `#!/bin/sh\nexec ${posixSingleQuote(nodePath)} ${posixSingleQuote(fixtureScript)} --fixture-kind ${posixSingleQuote(kind)} "$@"\n`;
}

function windowsShim(nodePath, fixtureScript, kind) {
  return `@echo off\r\n"${nodePath.replaceAll('%', '%%')}" "${fixtureScript.replaceAll('%', '%%')}" --fixture-kind ${kind} %*\r\n`;
}

function resolveExecutableOnPath(command, { baseEnv = process.env, platform = process.platform } = {}) {
  const pathValue = baseEnv.PATH ?? baseEnv.Path ?? '';
  const directories = pathValue.split(path.delimiter).filter(Boolean);
  const extensions =
    platform === 'win32'
      ? (baseEnv.PATHEXT ?? baseEnv.PathExt ?? '.COM;.EXE;.BAT;.CMD')
          .split(';')
          .filter(Boolean)
      : [''];
  for (const directory of directories) {
    if (platform === 'win32') {
      for (const extension of extensions) {
        const normalized = extension.startsWith('.') ? extension : `.${extension}`;
        const candidate = path.join(directory, `${command}${normalized}`);
        if (existsSync(candidate)) {
          return candidate;
        }
      }
      continue;
    }
    const candidate = path.join(directory, command);
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  fail(`required system command \`${command}\` was not found on PATH`);
}

function sanitizeBaseEnv(baseEnv = process.env, platform = process.platform) {
  const env = { ...baseEnv };
  for (const key of Object.keys(env)) {
    const upper = key.toUpperCase();
    const matchesPrefix = STRIP_ENV_PREFIXES.some((prefix) => upper.startsWith(prefix));
    const looksSensitive =
      upper.includes('TOKEN') || upper.includes('SECRET') || upper.includes('PASSWORD');
    if (STRIP_ENV_EXACT.includes(key) || STRIP_ENV_EXACT.includes(upper) || matchesPrefix || looksSensitive) {
      delete env[key];
    }
  }
  if (platform === 'win32' && env.Path !== undefined && env.PATH === undefined) {
    env.PATH = env.Path;
  }
  return env;
}

export function buildJourneyEnv({
  baseEnv = process.env,
  fixtureBinDir,
  gitBinDir,
  layout,
  platform = process.platform,
  wrapperBin
}) {
  const env = sanitizeBaseEnv(baseEnv, platform);
  const pathValue = [path.dirname(wrapperBin), layout.nodeShimDir, fixtureBinDir, gitBinDir]
    .filter(Boolean)
    .join(path.delimiter);
  env.COVEN_FAKE_FIXTURE_LOG = layout.fixtureLogPath;
  env.COVEN_HOME = layout.covenHome;
  env.HOME = layout.userHome;
  env.PATH = pathValue;
  env.USERPROFILE = layout.userHome;
  env.XDG_CONFIG_HOME = layout.xdgConfigHome;
  writeFileSync(layout.gitGlobalConfigPath, '', 'utf8');
  env.GIT_CONFIG_GLOBAL = layout.gitGlobalConfigPath;
  env.GIT_CONFIG_NOSYSTEM = '1';
  env.GIT_TEMPLATE_DIR = layout.gitTemplateDir;
  if (platform === 'win32') {
    env.Path = pathValue;
    const homeRoot = path.parse(layout.userHome).root;
    env.HOMEDRIVE = homeRoot.replace(/[\\/]$/, '');
    env.HOMEPATH = layout.userHome.slice(homeRoot.length - 1);
  }
  return env;
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

export function initGitRepo(runner, projectRoot, { env, layout } = {}) {
  if (!env) {
    fail('initGitRepo requires an isolated env');
  }
  if (!layout) {
    fail('initGitRepo requires a journey layout');
  }
  runner.runCapture('git', gitInitArgs(layout), { cwd: projectRoot, env });
  runner.runCapture(
    'git',
    gitCommitArgs(layout),
    { cwd: projectRoot, env }
  );
}

function listedCommands(helpText) {
  return helpText
    .split('\n')
    .map((line) => line.trimEnd())
    .filter((line) => line.startsWith('  '))
    .map((line) => line.trimStart().split(/\s+/)[0])
    .filter((name) => /^[a-z0-9-]+$/.test(name));
}

function assertVersionOutput(output) {
  if (!output.trim()) {
    fail('`coven --version` produced no output');
  }
  if (!output.toLowerCase().includes('coven')) {
    fail(`\`coven --version\` did not look like Coven output.\nstdout:\n${output}`);
  }
}

export function assertConciseDefaultHelp(output) {
  if (!output.toLowerCase().includes('usage')) {
    fail(`\`coven --help\` missing usage section.\nstdout:\n${output}`);
  }
  const commands = listedCommands(output);
  for (const command of CURATED_COMMANDS) {
    if (!commands.includes(command)) {
      fail(`\`coven --help\` is missing curated command ${command}.\nstdout:\n${output}`);
    }
  }
  for (const hidden of HIDDEN_HELP_COMMANDS) {
    if (output.includes(`\n  ${hidden}  `) || output.includes(hidden)) {
      fail(`\`coven --help\` should stay concise and must not surface ${hidden}.\nstdout:\n${output}`);
    }
  }
}

export function parseJsonOutput(label, output) {
  const stdout = typeof output === 'string' ? output : textFromResult(output, 'stdout');
  try {
    return JSON.parse(stdout);
  } catch (error) {
    fail(`${label} did not print valid JSON: ${error.message}\nstdout:\n${stdout}`);
  }
}

export function assertHelpCatalogJson(output) {
  const body = parseJsonOutput('`coven help --all --json`', output);
  if (body.schemaVersion !== 1) {
    fail(`unexpected help schemaVersion: ${body.schemaVersion}`);
  }
  const commands = (body.groups ?? [])
    .flatMap((group) => group.commands ?? [])
    .map((command) => command.name);
  for (const command of REQUIRED_PUBLIC_COMMANDS) {
    if (!commands.includes(command)) {
      fail(`\`coven help --all --json\` missing public command ${command}`);
    }
  }
  for (const hidden of ['process-supervisor', 'serve']) {
    if (commands.includes(hidden)) {
      fail(`\`coven help --all --json\` must not expose internal command ${hidden}`);
    }
  }
}

export function assertDoctorFailureGuidance(output, status) {
  if (status !== 1) {
    fail(`\`coven doctor\` on a bare runner should exit 1, got ${status}`);
  }
  for (const expected of [
    'Coven doctor',
    'Set up at least one harness in this same shell',
    'Codex: coven setup codex',
    'Claude Code: coven setup claude',
    'GitHub Copilot CLI: coven setup copilot',
    'Doctor found problems; review the failing checks above.'
  ]) {
    if (!output.includes(expected)) {
      fail(`\`coven doctor\` missing first-run guidance "${expected}".\nstdout:\n${output}`);
    }
  }
}

function assertDashboardHelp(output) {
  if (!output.includes('private local memory dashboard')) {
    fail(
      `\`coven memory open --help\` did not describe the private local dashboard.\nstdout:\n${output}`
    );
  }
}

export function assertDoctorPass(output) {
  if (!output.includes('Coven doctor')) {
    fail(`\`coven doctor\` did not print the expected banner.\nstdout:\n${output}`);
  }
  if (!output.includes('[OK] Codex') || !output.includes('[OK]') || !output.includes('coven-code')) {
    fail(`\`coven doctor\` did not report the fake Codex + coven-code environment.\nstdout:\n${output}`);
  }
  if (output.includes('[!!]')) {
    fail(`\`coven doctor\` should not report blocking failures after fixture install.\nstdout:\n${output}`);
  }
}

export function assertDoctorJsonPass(output) {
  const body = parseJsonOutput('`coven doctor --json`', output);
  if (body.ok !== true || body.blocking !== false) {
    fail(`\`coven doctor --json\` did not report a healthy environment.\nstdout:\n${textFromResult(output, 'stdout')}`);
  }
  const checks = body.checks ?? [];
  const harness = checks.find((check) => check.id === 'harness:codex');
  if (!harness || harness.status !== 'pass') {
    fail(`\`coven doctor --json\` must report harness:codex as pass.\nstdout:\n${textFromResult(output, 'stdout')}`);
  }
  const engine = checks.find((check) => check.id === 'engine');
  if (!engine || engine.status !== 'pass') {
    fail(`\`coven doctor --json\` must report engine as pass.\nstdout:\n${textFromResult(output, 'stdout')}`);
  }
  if (checks.some((check) => check.status === 'fail')) {
    fail(`\`coven doctor --json\` should have no failing checks once the fixture is installed.\nstdout:\n${textFromResult(output, 'stdout')}`);
  }
}

function assertDaemonRunningText(label, output) {
  if (!output.includes('Coven daemon: running')) {
    fail(`${label} did not report a running daemon.\nstdout:\n${output}`);
  }
}

export function assertDaemonStatusJson(output, expectedStatus, expectedOk) {
  const body = parseJsonOutput('daemon status --json', output);
  if (body.status !== expectedStatus || body.ok !== expectedOk) {
    fail(
      `unexpected daemon JSON health: expected ${expectedStatus}/${expectedOk}, got ${body.status}/${body.ok}\nstdout:\n${textFromResult(output, 'stdout')}`
    );
  }
  return body;
}

function sessionsFromJson(label, output) {
  const body = parseJsonOutput(label, output);
  if (!Array.isArray(body.sessions)) {
    fail(`${label} did not return a { sessions: [...] } envelope.`);
  }
  return body.sessions;
}

function eventPayloadStrings(events) {
  return events.map((event) => {
    if (typeof event.payload_json === 'string') {
      return event.payload_json;
    }
    return JSON.stringify(event);
  });
}

function parsedEventPayload(event) {
  if (event?.payload_json && typeof event.payload_json === 'object') {
    return event.payload_json;
  }
  if (typeof event?.payload_json !== 'string') {
    return null;
  }
  try {
    return JSON.parse(event.payload_json);
  } catch {
    return null;
  }
}

export function assertSessionInspection({
  eventsOutput,
  fixtureLogPath,
  logOutput,
  marker,
  sessionListOutput,
  showOutput
}) {
  const sessions = sessionsFromJson('`coven sessions --json`', sessionListOutput);
  if (sessions.length !== 1) {
    fail(`expected exactly one active session after the packaged journey, got ${sessions.length}`);
  }
  const listed = sessions[0];
  if (listed.harness !== 'codex' || listed.status !== 'completed') {
    fail(`unexpected session summary: ${JSON.stringify(listed)}`);
  }

  const session = parseJsonOutput('`coven sessions show --json`', showOutput);
  if (session.id !== listed.id) {
    fail(`session detail id ${session.id} did not match sessions list id ${listed.id}`);
  }
  if (session.harness !== 'codex' || session.status !== 'completed' || session.exit_code !== 0) {
    fail(`unexpected session detail: ${JSON.stringify(session)}`);
  }

  const eventsBody = parseJsonOutput('`coven sessions events --json`', eventsOutput);
  const events = eventsBody.events ?? [];
  if (events.length === 0) {
    fail('`coven sessions events --json` returned no events');
  }
  for (let index = 1; index < events.length; index += 1) {
    if ((events[index]?.seq ?? 0) <= (events[index - 1]?.seq ?? -1)) {
      fail('session events were not ordered by increasing sequence number');
    }
  }
  const payloads = eventPayloadStrings(events);
  const completionIndex = payloads.findIndex((payload) => payload.includes(`fake codex complete: ${marker}`));
  if (completionIndex === -1) {
    fail(`session events did not preserve the packaged Codex output for ${marker}.\npayloads:\n${payloads.join('\n')}`);
  }
  const terminalIndex = events.findIndex((event) => {
    const payload = parsedEventPayload(event);
    return payload?.status === 'completed' && payload?.exitCode === 0;
  });
  if (terminalIndex === -1) {
    fail(`session events did not record a completed terminal payload for ${marker}.\npayloads:\n${payloads.join('\n')}`);
  }
  if (terminalIndex <= completionIndex) {
    fail(`session events did not preserve output-before-terminal ordering for ${marker}.\npayloads:\n${payloads.join('\n')}`);
  }
  if (fixtureLogPath) {
    if (!existsSync(fixtureLogPath)) {
      fail(`fixture log was not created at ${fixtureLogPath}.`);
    }
    const fixtureEvents = readFileSync(fixtureLogPath, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    if (
      !fixtureEvents.some(
        (entry) =>
          entry.kind === 'codex' &&
          (entry.prompt === marker || entry.argv.includes(marker))
      )
    ) {
      fail(`fixture log did not record the Codex marker invocation for ${marker}.`);
    }
  }

  const logLines = parseJsonOutput('`coven sessions log --json`', logOutput);
  if (!Array.isArray(logLines) || logLines.length === 0) {
    fail('`coven sessions log --json` returned no log lines');
  }
  const messages = logLines.map((line) => String(line.message ?? ''));
  if (!messages.some((message) => message.includes(`fake codex complete: ${marker}`))) {
    fail(`session log did not preserve the fake Codex completion marker.\nmessages:\n${messages.join('\n')}`);
  }

  return session.id;
}

function assertArchiveAndSummonVisibility(activeOutput, allOutput, sessionId) {
  const activeSessions = sessionsFromJson('`coven sessions --json` after archive', activeOutput);
  if (activeSessions.some((session) => session.id === sessionId)) {
    fail(`archived session ${sessionId} should not appear in active sessions`);
  }
  const allSessions = sessionsFromJson('`coven sessions --all --json` after archive', allOutput);
  const archived = allSessions.find((session) => session.id === sessionId);
  if (!archived || !archived.archived_at) {
    fail(`archived session ${sessionId} should still appear in --all output`);
  }
}

function assertSummonOutput(output, sessionId, marker) {
  const stdout = textFromResult(output, 'stdout');
  if (!stdout.includes(marker)) {
    fail(`\`coven summon\` did not replay the archived session.\nstdout:\n${stdout}`);
  }
}

export function assertInvalidCwdFailure(output) {
  const stdout = textFromResult(output, 'stdout');
  const stderr = textFromResult(output, 'stderr');
  const combined = `${stdout}\n${stderr}`;
  if ((output.status ?? 0) === 0) {
    fail('outside-root `coven run --cwd` unexpectedly succeeded');
  }
  for (const expected of ['failed to resolve cwd', 'outside the Coven project root']) {
    if (!combined.includes(expected)) {
      fail(`outside-root rejection must mention "${expected}".\nstdout:\n${stdout}\nstderr:\n${stderr}`);
    }
  }
  if (combined.length > 2_000 || combined.includes('thread \'main\' panicked')) {
    fail(`outside-root rejection was not bounded/actionable.\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  }
}

function assertDaemonMetadataCleanup(layout, platform) {
  if (existsSync(path.join(layout.covenHome, 'daemon.json'))) {
    fail('daemon stop should remove daemon.json');
  }
  if (platform !== 'win32' && existsSync(path.join(layout.covenHome, 'coven.sock'))) {
    fail('daemon stop should remove the Unix socket file');
  }
}

export function runPackagedUserJourney({
  baseEnv = process.env,
  dashboardExpected = false,
  keepScratchDir = false,
  platform = process.platform,
  runner = createCommandRunner({ platform }),
  scratchRoot,
  sessionPrompt = 'E2E journey marker',
  wrapperBin
} = {}) {
  if (!wrapperBin) {
    fail('runPackagedUserJourney requires wrapperBin');
  }
  const resolvedWrapperBin = path.resolve(wrapperBin);
  if (!existsSync(resolvedWrapperBin)) {
    fail(`installed wrapper not found at ${resolvedWrapperBin}`);
  }

  const suppliedScratchRoot = scratchRoot !== undefined;
  const ownedScratchRoot = suppliedScratchRoot
    ? path.resolve(scratchRoot)
    : createCompactJourneyScratchRoot(repoRoot);
  if (suppliedScratchRoot) {
    mkdirSync(path.dirname(ownedScratchRoot), { recursive: true });
    try {
      mkdirSync(ownedScratchRoot);
    } catch (error) {
      if (error?.code === 'EEXIST') {
        fail(`scratch root already exists; refusing to delete caller-owned path: ${ownedScratchRoot}`);
      }
      throw error;
    }
  }
  const layout = createJourneyLayout(ownedScratchRoot);
  let activeEnv = undefined;
  let cleanupDaemon = false;
  let result;
  let primaryError;
  const daemonCleanupTimeoutMs = Math.min(DEFAULT_COMMAND_TIMEOUT_MS, 10_000);

  try {
    ensureJourneyLayout(layout);
    createNodeShim(layout.nodeShimDir, { platform });
    const gitCommand = createGitShim(layout.nodeShimDir, { baseEnv, platform });
    const gitBinDir = platform === 'win32' ? path.dirname(gitCommand) : undefined;

    activeEnv = buildJourneyEnv({
      baseEnv,
      gitBinDir,
      layout,
      platform,
      wrapperBin: resolvedWrapperBin
    });
    cleanupDaemon = true;
    initGitRepo(runner, layout.projectRoot, { env: activeEnv, layout });

    const version = runner.runCapture(resolvedWrapperBin, ['--version'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertVersionOutput(textFromResult(version, 'stdout'));

    const help = runner.runCapture(resolvedWrapperBin, ['--help'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertConciseDefaultHelp(textFromResult(help, 'stdout'));

    const helpJson = runner.runCapture(resolvedWrapperBin, ['help', '--all', '--json'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertHelpCatalogJson(helpJson);

    const firstDoctor = runner.runCapture(resolvedWrapperBin, ['doctor'], {
      allowedExitCodes: [1],
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertDoctorFailureGuidance(textFromResult(firstDoctor, 'stdout'), firstDoctor.status);

    if (dashboardExpected) {
      const memoryHelp = runner.runCapture(resolvedWrapperBin, ['memory', 'open', '--help'], {
        cwd: layout.projectRoot,
        env: activeEnv
      });
      assertDashboardHelp(textFromResult(memoryHelp, 'stdout'));
    }

    createFakeCodexFixture(layout.fixtureBinDir, { baseEnv, platform });
    activeEnv = buildJourneyEnv({
      baseEnv,
      fixtureBinDir: layout.fixtureBinDir,
      gitBinDir,
      layout,
      platform,
      wrapperBin: resolvedWrapperBin
    });

    const doctor = runner.runCapture(resolvedWrapperBin, ['doctor'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertDoctorPass(textFromResult(doctor, 'stdout'));

    const doctorJson = runner.runCapture(resolvedWrapperBin, ['doctor', '--json'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertDoctorJsonPass(doctorJson);

    const daemonStart = runner.runDaemonStart(resolvedWrapperBin, activeEnv, {
      cwd: layout.projectRoot
    });
    if (daemonStart !== undefined) {
      assertDaemonRunningText('`coven daemon start`', textFromResult(daemonStart, 'stdout'));
    }

    const daemonStatus = runner.runCapture(resolvedWrapperBin, ['daemon', 'status'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertDaemonRunningText('`coven daemon status`', textFromResult(daemonStatus, 'stdout'));

    const daemonStatusJson = runner.runCapture(
      resolvedWrapperBin,
      ['daemon', 'status', '--json'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    assertDaemonStatusJson(daemonStatusJson, 'running', true);

    const runOutput = runner.runCapture(resolvedWrapperBin, ['run', 'codex', sessionPrompt], {
      cwd: layout.projectRoot,
      env: activeEnv,
      timeoutMs: DEFAULT_COMMAND_TIMEOUT_MS
    });
    if (!textFromResult(runOutput, 'stdout').includes(sessionPrompt)) {
      fail(`\`coven run codex\` did not emit the marker prompt.\nstdout:\n${textFromResult(runOutput, 'stdout')}`);
    }

    const sessions = runner.runCapture(resolvedWrapperBin, ['sessions', '--json'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    const listedSessions = sessionsFromJson('`coven sessions --json`', sessions);
    if (listedSessions.length !== 1) {
      fail(`expected one active session after run, got ${listedSessions.length}`);
    }
    const sessionId = listedSessions[0].id;
    const show = runner.runCapture(
      resolvedWrapperBin,
      ['sessions', 'show', sessionId, '--json'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    const events = runner.runCapture(
      resolvedWrapperBin,
      ['sessions', 'events', sessionId, '--json'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    const log = runner.runCapture(resolvedWrapperBin, ['sessions', 'log', sessionId, '--json'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertSessionInspection({
      eventsOutput: events,
      fixtureLogPath: layout.fixtureLogPath,
      logOutput: log,
      marker: sessionPrompt,
      sessionListOutput: sessions,
      showOutput: show
    });

    const archive = runner.runCapture(resolvedWrapperBin, ['archive', sessionId], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    if (!textFromResult(archive, 'stdout').includes('archived session')) {
      fail(`\`coven archive\` did not confirm the archive action.\nstdout:\n${textFromResult(archive, 'stdout')}`);
    }

    const activeAfterArchive = runner.runCapture(resolvedWrapperBin, ['sessions', '--json'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    const allAfterArchive = runner.runCapture(
      resolvedWrapperBin,
      ['sessions', '--all', '--json'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    assertArchiveAndSummonVisibility(activeAfterArchive, allAfterArchive, sessionId);

    const summon = runner.runCapture(resolvedWrapperBin, ['summon', sessionId], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    assertSummonOutput(summon, sessionId, sessionPrompt);

    const activeAfterSummon = runner.runCapture(resolvedWrapperBin, ['sessions', '--json'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    const restored = sessionsFromJson('`coven sessions --json` after summon', activeAfterSummon)
      .find((session) => session.id === sessionId);
    if (!restored || restored.archived_at !== null) {
      fail(`summoned session ${sessionId} should be active again`);
    }

    const sacrifice = runner.runCapture(
      resolvedWrapperBin,
      ['sacrifice', sessionId, '--yes'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    if (!textFromResult(sacrifice, 'stdout').includes('sacrificed session')) {
      fail(`\`coven sacrifice --yes\` did not confirm the delete.\nstdout:\n${textFromResult(sacrifice, 'stdout')}`);
    }

    const allAfterSacrifice = runner.runCapture(
      resolvedWrapperBin,
      ['sessions', '--all', '--json'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    if (
      sessionsFromJson('`coven sessions --all --json` after sacrifice', allAfterSacrifice).some(
        (session) => session.id === sessionId
      )
    ) {
      fail(`sacrificed session ${sessionId} should not remain in --all output`);
    }

    const invalidCwd = runner.runCapture(
      resolvedWrapperBin,
      ['run', 'codex', 'outside root attempt', '--cwd', layout.outsideRoot],
      {
        allowedExitCodes: [1],
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    assertInvalidCwdFailure(invalidCwd);

    const stop = runner.runCapture(resolvedWrapperBin, ['daemon', 'stop'], {
      cwd: layout.projectRoot,
      env: activeEnv
    });
    if (!textFromResult(stop, 'stdout').includes('Coven daemon: stopped')) {
      fail(`\`coven daemon stop\` did not confirm shutdown.\nstdout:\n${textFromResult(stop, 'stdout')}`);
    }

    const stopped = runner.runCapture(
      resolvedWrapperBin,
      ['daemon', 'status', '--json'],
      {
        cwd: layout.projectRoot,
        env: activeEnv
      }
    );
    assertDaemonStatusJson(stopped, 'stopped', false);
    assertDaemonMetadataCleanup(layout, platform);
    cleanupDaemon = false;

    result = { scratchRoot: ownedScratchRoot, sessionId };
  } catch (error) {
    primaryError = error;
  } finally {
    let cleanupError;
    if (cleanupDaemon && activeEnv) {
      try {
        runner.runCapture(resolvedWrapperBin, ['daemon', 'stop'], {
          allowedExitCodes: [0, 1],
          cwd: layout.projectRoot,
          env: activeEnv,
          timeoutMs: daemonCleanupTimeoutMs
        });
        const stopped = runner.runCapture(
          resolvedWrapperBin,
          ['daemon', 'status', '--json'],
          {
            cwd: layout.projectRoot,
            env: activeEnv,
            timeoutMs: daemonCleanupTimeoutMs
          }
        );
        assertDaemonStatusJson(stopped, 'stopped', false);
        assertDaemonMetadataCleanup(layout, platform);
      } catch (error) {
        cleanupError = error;
      }
    }
    if (!keepScratchDir && !cleanupError) {
      rmSync(ownedScratchRoot, { recursive: true, force: true });
    }
    if (cleanupError) {
      const cleanupMessage =
        `daemon cleanup failed: ${cleanupError.message}; scratch preserved at ${ownedScratchRoot}`;
      primaryError = primaryError
        ? new AggregateError(
            [primaryError, cleanupError],
            `${primaryError.message}; ${cleanupMessage}`
          )
        : new Error(cleanupMessage, { cause: cleanupError });
    }
  }

  if (primaryError) {
    throw primaryError;
  }
  return result;
}

function parseArgs(argv) {
  const options = {};
  for (const arg of argv) {
    if (arg === '--keep-scratchdir') {
      options.keepScratchDir = true;
      continue;
    }
    if (arg === '--dashboard-installed') {
      options.dashboardExpected = true;
      continue;
    }
    if (arg.startsWith('--scratch-root=')) {
      options.scratchRoot = arg.slice('--scratch-root='.length);
      continue;
    }
    if (arg.startsWith('--wrapper-bin=')) {
      options.wrapperBin = arg.slice('--wrapper-bin='.length);
      continue;
    }
    fail(`unknown argument: ${arg}`);
  }
  if (!options.wrapperBin) {
    fail('usage: node scripts/user-journey-e2e.mjs --wrapper-bin=<installed-wrapper> [--scratch-root=<dir>] [--keep-scratchdir] [--dashboard-installed]');
  }
  return options;
}

if (isMainModule(import.meta.url)) {
  try {
    const options = parseArgs(process.argv.slice(2));
    const result = runPackagedUserJourney(options);
    console.log(`Packaged user journey passed (session ${result.sessionId}).`);
    if (options.keepScratchDir) {
      console.log(`Scratch preserved at ${result.scratchRoot}.`);
    }
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
