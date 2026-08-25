#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { constants as osConstants } from 'node:os';
import path from 'node:path';

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
const WINDOWS_HIDE_NATIVE_WINDOW_ENV = 'COVEN_WINDOWS_HIDE_NATIVE_WINDOW';
// Wrapper-to-native handoff for `coven memory open`. The wrapper is the only
// legitimate source of these, so an inherited value is always stale or hostile.
const MEMORY_DASHBOARD_HANDOFF_ENV = [
  'COVEN_MEMORY_DASHBOARD_ENTRY',
  'COVEN_MEMORY_DASHBOARD_NODE',
  'COVEN_MEMORY_DASHBOARD_BIN'
];
const PRINT_NATIVE_BINARY_PATH_ARG = '--print-native-binary-path';

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
if (args.includes(PRINT_NATIVE_BINARY_PATH_ARG)) {
  if (args.length !== 1) {
    console.error(`${PRINT_NATIVE_BINARY_PATH_ARG} cannot be combined with other arguments.`);
    process.exit(1);
  }
  if (!path.isAbsolute(binary) || /[\r\n]/.test(binary)) {
    console.error('Resolved native Coven binary path is not a safe absolute path.');
    process.exit(1);
  }
  process.stdout.write(`${binary}\n`);
  process.exit(0);
}
const childEnv = { ...process.env };
// Desktop clients that intentionally own no console can opt into a hidden
// native child. Keep ordinary CLI launches unchanged so inherited output,
// terminal attachment, and Ctrl-C behavior retain their existing semantics.
const hideNativeWindowSignal = Object.entries(process.env).find(
  ([name]) => name.toUpperCase() === WINDOWS_HIDE_NATIVE_WINDOW_ENV
);
const hideNativeWindow =
  process.platform === 'win32' && hideNativeWindowSignal?.[1] === '1';
// This is a wrapper-boundary instruction, not ambient Coven configuration.
// Consume every casing of the name (Windows environment lookup is
// case-insensitive) so the native CLI and launched harnesses cannot inherit
// it and accidentally hide an unrelated nested wrapper invocation.
for (const name of Object.keys(childEnv)) {
  if (name.toUpperCase() === WINDOWS_HIDE_NATIVE_WINDOW_ENV) {
    delete childEnv[name];
  }
}
// Same reasoning for the dashboard handoff, and the same case-insensitive
// sweep. The native CLI launches whatever entrypoint these name, so an
// inherited value is arbitrary code selected by whoever set it. Clearing them
// first means the wrapper's own resolution below is the only thing that can
// populate them; when that resolution fails the native binary sees no handoff
// and emits its installation error instead of running a stale build.
for (const name of Object.keys(childEnv)) {
  if (MEMORY_DASHBOARD_HANDOFF_ENV.includes(name.toUpperCase())) {
    delete childEnv[name];
  }
}
const opensMemoryDashboard = isMemoryOpenInvocation(args);
const requestsHelp = args.some((arg) => arg === '--help' || arg === '-h');
if (
  opensMemoryDashboard &&
  !requestsHelp &&
  !supportsMemoryDashboard(process.versions.node)
) {
  console.error(
    `coven memory open requires Node.js ${MEMORY_DASHBOARD_MIN_NODE_MAJOR} or newer; current Node.js is ${process.versions.node}. Upgrade Node.js, then install the dashboard companion with: npm install -g @opencoven/coven-memory-dashboard. Other Coven CLI commands continue to support Node.js 18 or newer.`
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
    // The default for anyone who has not installed the companion. The
    // handoff variables stay cleared, so the native binary emits the single
    // actionable installation error.
  }
}

// libuv can apply CREATE_NO_WINDOW only when none of the child's stdio entries
// inherit Windows handles. The opt-in desktop path therefore uses pipes and
// transparently forwards them; ordinary terminal-owned CLI launches retain
// direct inherited stdio and their existing console/Ctrl-C behavior.
const child = spawn(binary, args, {
  stdio: hideNativeWindow ? ['pipe', 'pipe', 'pipe'] : 'inherit',
  windowsHide: hideNativeWindow,
  env: childEnv
});

let forwardingError = null;
if (hideNativeWindow) {
  const failForwarding = (label, error) => {
    if (
      label === 'stdin' &&
      (error?.code === 'EPIPE' || error?.code === 'ERR_STREAM_DESTROYED')
    ) {
      // A target may intentionally close stdin before it exits. This is not a
      // wrapper transport failure; its own exit status remains authoritative.
      return;
    }
    if (!forwardingError) {
      forwardingError = new Error(`${label} forwarding failed: ${error.message}`);
      process.exitCode = 1;
      try {
        process.stderr.write(`Coven wrapper: ${forwardingError.message}\n`);
      } catch {
        // The stderr destination itself may be the failed stream.
      }
      if (!child.killed) {
        child.kill('SIGTERM');
      }
    }
  };

  child.stdin.on('error', (error) => failForwarding('stdin', error));
  child.stdout.on('error', (error) => failForwarding('stdout source', error));
  child.stderr.on('error', (error) => failForwarding('stderr source', error));
  process.stdout.on('error', (error) => failForwarding('stdout', error));
  process.stderr.on('error', (error) => failForwarding('stderr', error));
  process.stdin.on('error', (error) => failForwarding('stdin source', error));

  process.stdin.pipe(child.stdin);
  child.stdout.pipe(process.stdout, { end: false });
  child.stderr.pipe(process.stderr, { end: false });
}

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

child.on(hideNativeWindow ? 'close' : 'exit', (code, signal) => {
  if (hideNativeWindow) {
    process.stdin.unpipe(child.stdin);
    process.stdin.pause();
  }
  if (forwardingError) {
    process.exitCode = 1;
    return;
  }
  if (signal) {
    if (process.platform === 'win32') {
      const signalNumber = osConstants.signals[signal];
      process.exitCode = signalNumber === undefined ? 1 : 128 + signalNumber;
      return;
    }
    process.removeAllListeners(signal);
    process.kill(process.pid, signal);
    return;
  }
  // Setting exitCode (rather than forcing process.exit) lets pipe-backed
  // stdout/stderr finish flushing without truncating the native CLI output.
  if (hideNativeWindow) {
    process.exitCode = code ?? 1;
  } else {
    process.exit(code ?? 1);
  }
});
