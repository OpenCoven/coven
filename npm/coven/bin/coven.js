#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { constants as osConstants } from 'node:os';

const require = createRequire(import.meta.url);

const PLATFORM_PACKAGES = {
  'darwin-arm64': '@opencoven/cli-macos',
  'darwin-x64': '@opencoven/cli-macos-x64',
  'linux-x64': '@opencoven/cli-linux-x64',
  'win32-x64': '@opencoven/cli-windows'
};

const binaryName = process.platform === 'win32' ? 'coven.exe' : 'coven';
const platformKey = `${process.platform}-${process.arch}`;
const packageName = PLATFORM_PACKAGES[platformKey];
const MEMORY_DASHBOARD_MIN_NODE_MAJOR = 24;

function resolveBinary() {
  if (!packageName) {
    throw new Error(
      `Unsupported platform ${platformKey}. Coven v0 publishes native npm packages for macOS Apple Silicon, Intel macOS x64, glibc-based Linux x64, and Windows x64.`
    );
  }

  try {
    return require.resolve(`${packageName}/bin/${binaryName}`);
  } catch (error) {
    throw new Error(
      `Could not find native Coven package ${packageName}. Reinstall @opencoven/cli so npm can install the optional dependency for ${platformKey}. Linux x64 support requires a glibc-based distribution; Alpine is not supported. Windows support requires x64 Windows. Original error: ${error.message}`
    );
  }
}

function isMemoryOpenInvocation(args) {
  if (args.includes('--json')) {
    return false;
  }
  const commandPath = [];
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === '--color') {
      index += 1;
      continue;
    }
    if (arg.startsWith('--color=')) {
      continue;
    }
    commandPath.push(arg);
    if (commandPath.length === 2) {
      break;
    }
  }
  return commandPath[0] === 'memory' && commandPath[1] === 'open';
}

function supportsMemoryDashboard(nodeVersion) {
  const major = Number.parseInt(nodeVersion.split('.')[0], 10);
  return Number.isInteger(major) && major >= MEMORY_DASHBOARD_MIN_NODE_MAJOR;
}

let binary;
try {
  binary = resolveBinary();
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

// Delegate every argument — including a lone --version/-V — to the native
// binary, which renders the full `coven vX (engine coven-code …, pinned …)`
// line. (The wrapper previously short-circuited --version to its own
// package.json version, which shadowed that output for npm installs.)
const args = process.argv.slice(2);
const childEnv = { ...process.env };
const opensMemoryDashboard = isMemoryOpenInvocation(args);
const requestsHelp = args.some((arg) => arg === '--help' || arg === '-h');
if (
  opensMemoryDashboard &&
  !requestsHelp &&
  !supportsMemoryDashboard(process.versions.node)
) {
  console.error(
    `coven memory open requires Node.js ${MEMORY_DASHBOARD_MIN_NODE_MAJOR} or newer; current Node.js is ${process.versions.node}. Upgrade Node.js and reinstall @opencoven/cli. Other Coven CLI commands continue to support Node.js 18 or newer.`
  );
  process.exit(1);
}
if (opensMemoryDashboard) {
  try {
    childEnv['COVEN_MEMORY_DASHBOARD_ENTRY'] = require.resolve(
      '@opencoven/coven-memory-dashboard/bin/coven-memory-dashboard.mjs'
    );
    childEnv['COVEN_MEMORY_DASHBOARD_NODE'] = process.execPath;
  } catch {
    // The native binary emits the single actionable installation error.
  }
}

const child = spawn(binary, args, {
  stdio: 'inherit',
  windowsHide: false,
  env: childEnv
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    if (!child.killed) {
      child.kill(signal);
    }
  });
}

child.on('error', (error) => {
  console.error(`Failed to launch Coven binary at ${binary}: ${error.message}`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    if (process.platform === 'win32') {
      const signalNumber = osConstants.signals[signal];
      process.exit(signalNumber === undefined ? 1 : 128 + signalNumber);
    }
    process.removeAllListeners(signal);
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
