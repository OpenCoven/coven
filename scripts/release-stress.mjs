#!/usr/bin/env node

import { appendFileSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_ITERATIONS = 10;
const DEFAULT_COMMAND_TIMEOUT_MS = 180_000;

const UNIX_COMMANDS = [
  {
    label: 'claim acquisition',
    args: ['test', '-p', 'coven-cli', '--test', 'parallel_protocol', '--locked']
  },
  {
    label: 'memory migration',
    args: [
      'test',
      '-p',
      'coven-cli',
      '--bin',
      'coven',
      'cockpit_sources::tests::opened_memory_record_rechecks_logical_restore_state',
      '--locked',
      '--',
      '--exact'
    ]
  },
  {
    label: 'process cleanup',
    args: [
      'test',
      '-p',
      'coven-cli',
      '--test',
      'smoke',
      'daemon_stop_terminates_live_piped_session_descendants',
      '--locked',
      '--',
      '--exact'
    ]
  },
  {
    label: 'PTY timeout',
    args: [
      'test',
      '-p',
      'coven-cli',
      '--bin',
      'coven',
      'pty_runner::tests::codex_json_runner_times_out_while_a_large_prompt_is_still_writing',
      '--locked',
      '--',
      '--exact'
    ]
  },
  {
    label: 'short socket homes',
    args: ['test', '-p', 'coven-client', '--test', 'health', '--locked']
  }
];

const WINDOWS_COMMANDS = [
  {
    label: 'Windows PTY timeout and process cleanup',
    args: [
      'test',
      '-p',
      'coven-cli',
      '--bin',
      'coven',
      'pty_runner::tests::windows_detached_pty_timeout_fails_and_kills_descendant',
      '--locked',
      '--',
      '--exact'
    ]
  }
];

export function buildStressPlan({
  suite,
  iterations = DEFAULT_ITERATIONS,
  commandTimeoutMs = DEFAULT_COMMAND_TIMEOUT_MS
}) {
  if (!Number.isInteger(iterations) || iterations < 1 || iterations > DEFAULT_ITERATIONS) {
    throw new Error(`iterations must be an integer from 1 through ${DEFAULT_ITERATIONS}`);
  }
  if (!Number.isInteger(commandTimeoutMs) || commandTimeoutMs < 1 || commandTimeoutMs > 300_000) {
    throw new Error('command timeout must be an integer from 1 through 300000 milliseconds');
  }
  const commands = suite === 'unix'
    ? UNIX_COMMANDS
    : suite === 'windows'
      ? WINDOWS_COMMANDS
      : null;
  if (!commands) {
    throw new Error("suite must be 'unix' or 'windows'");
  }

  return Array.from({ length: iterations }, (_, index) =>
    commands.map((command) => ({
      ...command,
      args: [...command.args],
      iteration: index + 1,
      timeoutMs: commandTimeoutMs
    }))
  ).flat();
}

export function sanitizeOutput(text, privatePaths) {
  let sanitized = String(text ?? '');
  for (const privatePath of privatePaths) {
    if (!privatePath) {
      continue;
    }
    sanitized = sanitized.split(String(privatePath)).join('<repo>');
  }
  return sanitized;
}

export function runStressPlan({
  plan,
  repoRoot: workingDirectory,
  runCommand = runCargoCommand,
  writeLog
}) {
  for (const entry of plan) {
    const header = `iteration=${entry.iteration} surface=${entry.label}\n`;
    writeLog(header);
    const result = runCommand(entry, workingDirectory);
    const output = sanitizeOutput(
      `${result.stdout ?? ''}${result.stderr ?? ''}`,
      [workingDirectory, process.env.HOME, process.env.USERPROFILE]
    );
    writeLog(output);
    if (output && !output.endsWith('\n')) {
      writeLog('\n');
    }

    if (result.error?.code === 'ETIMEDOUT') {
      throw new Error(
        `${entry.label} iteration ${entry.iteration} timed out after ${entry.timeoutMs}ms`
      );
    }
    if (result.error) {
      throw new Error(
        `${entry.label} iteration ${entry.iteration} failed to launch: ${result.error.message}`
      );
    }
    if (result.status !== 0) {
      throw new Error(
        `${entry.label} iteration ${entry.iteration} failed with exit ${result.status}`
      );
    }
  }
}

function runCargoCommand(entry, workingDirectory) {
  return spawnSync('cargo', entry.args, {
    cwd: workingDirectory,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: entry.timeoutMs
  });
}

function optionValue(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || index + 1 >= args.length) {
    throw new Error(`${name} is required`);
  }
  return args[index + 1];
}

function main() {
  const args = process.argv.slice(2);
  const suite = optionValue(args, '--suite');
  const iterations = Number(optionValue(args, '--iterations'));
  const commandTimeoutMs = Number(optionValue(args, '--command-timeout-ms'));
  const logPath = path.resolve(optionValue(args, '--log'));
  const plan = buildStressPlan({ suite, iterations, commandTimeoutMs });
  writeFileSync(logPath, `suite=${suite} iterations=${iterations}\n`);

  runStressPlan({
    plan,
    repoRoot,
    writeLog(text) {
      appendFileSync(logPath, text);
      process.stdout.write(text);
    }
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
