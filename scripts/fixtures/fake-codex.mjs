#!/usr/bin/env node

import { appendFileSync } from 'node:fs';

const args = process.argv.slice(2);
const fixtureFlagIndex = args.indexOf('--fixture-kind');
if (fixtureFlagIndex === -1 || fixtureFlagIndex === args.length - 1) {
  console.error('fake-codex fixture requires --fixture-kind <codex|coven-code>');
  process.exit(2);
}

const fixtureKind = args[fixtureFlagIndex + 1];
const passthroughArgs = [
  ...args.slice(0, fixtureFlagIndex),
  ...args.slice(fixtureFlagIndex + 2),
];

logInvocation(fixtureKind, passthroughArgs);

switch (fixtureKind) {
  case 'codex':
    runCodexFixture(passthroughArgs);
    break;
  case 'coven-code':
    runCovenCodeFixture(passthroughArgs);
    break;
  default:
    console.error(`unsupported fake fixture kind: ${fixtureKind}`);
    process.exit(2);
}

function runCodexFixture(argv) {
  if (argv[0] === '--version') {
    process.stdout.write('codex 0.0.0-fake\n');
    return;
  }
  if (argv[0] === 'login') {
    process.stdout.write('fake codex login ok\n');
    return;
  }

  const prompt = promptText(argv);
  process.stdout.write('fake codex harness=codex\n');
  process.stdout.write(`fake codex complete: ${prompt}\n`);
}

function runCovenCodeFixture(argv) {
  if (argv.length === 1 && argv[0] === '--version') {
    process.stdout.write('coven-code 0.6.1\n');
    return;
  }
  if (
    argv.length === 3 &&
    argv[0] === 'auth' &&
    argv[1] === 'status' &&
    argv[2] === '--json'
  ) {
    process.stdout.write('{"loggedIn":true}\n');
    return;
  }

  process.stdout.write('fake coven-code ready\n');
}

function promptText(argv) {
  const separatorIndex = argv.indexOf('--');
  const promptArgs =
    separatorIndex === -1 ? argv.filter((arg) => !arg.startsWith('-')) : argv.slice(separatorIndex + 1);
  return promptArgs.join(' ').trim() || '<empty prompt>';
}

function logInvocation(kind, argv) {
  const logPath = process.env.COVEN_FAKE_FIXTURE_LOG;
  if (!logPath) {
    return;
  }
  appendFileSync(
    logPath,
    `${JSON.stringify({ kind, argv, cwd: process.cwd() })}\n`,
    'utf8'
  );
}
