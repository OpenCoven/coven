#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const packagedBinary = path.resolve(
  __dirname,
  '..',
  'native',
  process.platform,
  process.arch,
  process.platform === 'win32' ? 'coven.exe' : 'coven'
);
const repoDebugBinary = path.resolve(
  __dirname,
  '..',
  '..',
  '..',
  'target',
  'debug',
  process.platform === 'win32' ? 'coven.exe' : 'coven'
);
const binary = existsSync(packagedBinary) ? packagedBinary : repoDebugBinary;

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(`Failed to launch Coven binary at ${binary}: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
