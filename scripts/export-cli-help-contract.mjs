#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HELP_ARGS = ['help', '--all', '--json'];
const HELP_SCHEMA_VERSION = 1;
const DOCS_ORIGIN = 'https://docs.opencoven.ai';
const DOCS_PATH_PREFIX = '/docs/';
const ANSI_ESCAPE_RE = /\u001b\[[0-?]*[ -/]*[@-~]/u;
const POSIX_MACHINE_PATH_RE =
  /(?:^|[\s"'`(])\/(?:Users|home|var|private|opt|etc|Volumes|mnt|srv|root)(?:\/[^\s"'`<>]+)+/u;
const WINDOWS_MACHINE_PATH_RE =
  /(?:^|[\s"'`(])[A-Za-z]:\\(?:[^\\\s"'`<>]+\\)*[^\\\s"'`<>]*/u;
const INTERNAL_COMMAND_NAMES = new Set(['process-supervisor', 'serve']);

function usage() {
  return 'Usage: node scripts/export-cli-help-contract.mjs --binary <command> [--binary-arg <arg>]... --output <path>';
}

function fail(message) {
  throw new Error(`${message}\n${usage()}`);
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function hasAnsi(value) {
  return ANSI_ESCAPE_RE.test(value);
}

function hasMachinePath(value) {
  return POSIX_MACHINE_PATH_RE.test(value) || WINDOWS_MACHINE_PATH_RE.test(value);
}

function ensureExactKeys(value, expectedKeys, label) {
  const actualKeys = Object.keys(value).sort();
  const wantedKeys = [...expectedKeys].sort();
  if (actualKeys.length !== wantedKeys.length || actualKeys.some((key, index) => key !== wantedKeys[index])) {
    throw new Error(`${label} must contain exactly these keys: ${expectedKeys.join(', ')}`);
  }
}

function ensureSafeString(value, label) {
  if (typeof value !== 'string' || value.trim().length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  if (hasAnsi(value)) {
    throw new Error(`${label} must not contain ANSI escape sequences`);
  }
  if (hasMachinePath(value)) {
    throw new Error(`${label} must not contain a machine-specific path`);
  }
  return value;
}

function normalizeDocsUrl(value, label) {
  const docsUrl = ensureSafeString(value, label);
  let parsed;
  try {
    parsed = new URL(docsUrl);
  } catch (error) {
    throw new Error(`${label} must be a valid URL: ${error.message}`);
  }
  if (parsed.origin !== DOCS_ORIGIN) {
    throw new Error(`${label} must use the stable ${DOCS_ORIGIN} origin`);
  }
  if (!parsed.pathname.startsWith(DOCS_PATH_PREFIX)) {
    throw new Error(`${label} must stay under ${DOCS_PATH_PREFIX}`);
  }
  if (parsed.search) {
    throw new Error(`${label} must not include query parameters`);
  }
  if (parsed.hash && !/^#[a-z0-9-]+$/u.test(parsed.hash)) {
    throw new Error(`${label} must use lowercase kebab-case fragment ids`);
  }
  return `${parsed.origin}${parsed.pathname}${parsed.hash}`;
}

export function parseArgs(argv) {
  const options = { binaryArgs: [] };
  let index = 0;

  while (index < argv.length) {
    const argument = argv[index];
    if (argument === '--help' || argument === '-h') {
      return { help: true };
    }

    if (argument.startsWith('--binary=')) {
      options.binary = argument.slice('--binary='.length);
      index += 1;
      continue;
    }
    if (argument === '--binary') {
      if (argv[index + 1] === undefined) {
        fail('missing value for --binary');
      }
      options.binary = argv[index + 1];
      index += 2;
      continue;
    }
    if (argument.startsWith('--binary-arg=')) {
      options.binaryArgs.push(argument.slice('--binary-arg='.length));
      index += 1;
      continue;
    }
    if (argument === '--binary-arg') {
      if (argv[index + 1] === undefined) {
        fail('missing value for --binary-arg');
      }
      options.binaryArgs.push(argv[index + 1]);
      index += 2;
      continue;
    }
    if (argument.startsWith('--output=')) {
      options.output = argument.slice('--output='.length);
      index += 1;
      continue;
    }
    if (argument === '--output') {
      if (argv[index + 1] === undefined) {
        fail('missing value for --output');
      }
      options.output = argv[index + 1];
      index += 2;
      continue;
    }

    fail(`unknown argument: ${argument}`);
  }

  if (!options.binary) {
    fail('missing required --binary path');
  }
  if (!options.output) {
    fail('missing required --output path');
  }

  return options;
}

export function normalizeCliHelpContract(payload) {
  if (!isPlainObject(payload)) {
    throw new Error('help contract must be a JSON object');
  }
  ensureExactKeys(payload, ['schemaVersion', 'groups'], 'help contract');

  if (payload.schemaVersion !== HELP_SCHEMA_VERSION) {
    throw new Error(`schemaVersion must be ${HELP_SCHEMA_VERSION}`);
  }
  if (!Array.isArray(payload.groups) || payload.groups.length === 0) {
    throw new Error('groups must be a non-empty array');
  }

  const seenGroupIds = new Set();
  const seenCommands = new Set();

  const groups = payload.groups.map((group, groupIndex) => {
    if (!isPlainObject(group)) {
      throw new Error(`group ${groupIndex} must be an object`);
    }
    ensureExactKeys(group, ['id', 'title', 'commands'], `group ${groupIndex}`);

    const id = ensureSafeString(group.id, `group ${groupIndex} id`);
    if (!/^[a-z0-9-]+$/u.test(id)) {
      throw new Error(`group ${groupIndex} id must be lowercase kebab-case`);
    }
    if (seenGroupIds.has(id)) {
      throw new Error(`duplicate group id: ${id}`);
    }
    seenGroupIds.add(id);

    const title = ensureSafeString(group.title, `group ${groupIndex} title`);
    if (!Array.isArray(group.commands) || group.commands.length === 0) {
      throw new Error(`group ${id} must contain at least one command`);
    }

    const commands = group.commands.map((command, commandIndex) => {
      if (!isPlainObject(command)) {
        throw new Error(`group ${id} command ${commandIndex} must be an object`);
      }
      ensureExactKeys(command, ['name', 'summary', 'docsUrl'], `group ${id} command ${commandIndex}`);

      const name = ensureSafeString(command.name, `group ${id} command ${commandIndex} name`);
      if (!/^[a-z0-9-]+$/u.test(name)) {
        throw new Error(`command ${name} must be lowercase kebab-case`);
      }
      if (INTERNAL_COMMAND_NAMES.has(name)) {
        throw new Error(`internal command leaked into public help: ${name}`);
      }
      if (seenCommands.has(name)) {
        throw new Error(`duplicate command name: ${name}`);
      }
      seenCommands.add(name);

      return {
        name,
        summary: ensureSafeString(command.summary, `command ${name} summary`),
        docsUrl: normalizeDocsUrl(command.docsUrl, `command ${name} docsUrl`),
      };
    });

    return { id, title, commands };
  });

  return {
    schemaVersion: HELP_SCHEMA_VERSION,
    groups,
  };
}

export function exportCliHelpContract({ binary, binaryArgs = [], output }) {
  const commandArgs = [...binaryArgs, ...HELP_ARGS];
  const result = spawnSync(binary, commandArgs, {
    encoding: 'utf8',
    maxBuffer: 10 * 1024 * 1024,
  });

  if (result.error) {
    throw new Error(`failed to execute ${binary}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      [
        `${binary} ${commandArgs.join(' ')} exited with ${result.status}`,
        result.stdout ? `stdout:\n${result.stdout}` : null,
        result.stderr ? `stderr:\n${result.stderr}` : null,
      ]
        .filter(Boolean)
        .join('\n\n'),
    );
  }
  if (!result.stdout || result.stdout.trim().length === 0) {
    throw new Error('CLI help contract command produced no stdout');
  }
  if (hasAnsi(result.stdout)) {
    throw new Error('CLI help contract stdout must not contain ANSI escape sequences');
  }
  if (hasMachinePath(result.stdout)) {
    throw new Error('CLI help contract stdout must not contain a machine-specific path');
  }

  let parsed;
  try {
    parsed = JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`CLI help contract stdout was not valid JSON: ${error.message}`);
  }

  const normalized = normalizeCliHelpContract(parsed);
  const pretty = `${JSON.stringify(normalized, null, 2)}\n`;

  mkdirSync(path.dirname(output), { recursive: true });
  writeFileSync(output, pretty);
  return { normalized, pretty };
}

function main() {
  const parsed = parseArgs(process.argv.slice(2));
  if (parsed.help) {
    console.log(usage());
    return;
  }
  exportCliHelpContract(parsed);
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
