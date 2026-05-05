#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

const PLATFORM_PACKAGES = {
  'darwin-arm64': '@opencoven/cli-macos',
  'linux-x64': '@opencoven/cli-linux-x64'
};

const binaryName = process.platform === 'win32' ? 'coven.exe' : 'coven';
const platformKey = `${process.platform}-${process.arch}`;
const packageName = PLATFORM_PACKAGES[platformKey];

function wrapperVersion() {
  return require('../package.json').version;
}

function resolveBinary() {
  if (!packageName) {
    throw new Error(
      `Unsupported platform ${platformKey}. Coven v0 publishes native npm packages for macOS Apple Silicon and glibc-based Linux x64.`
    );
  }

  try {
    return require.resolve(`${packageName}/bin/${binaryName}`);
  } catch (error) {
    throw new Error(
      `Could not find native Coven package ${packageName}. Reinstall @opencoven/cli so npm can install the optional dependency for ${platformKey}. Linux x64 support requires a glibc-based distribution; Alpine is not supported. Original error: ${error.message}`
    );
  }
}

let binary;
try {
  binary = resolveBinary();
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

const args = process.argv.slice(2);
if (args.length === 1 && (args[0] === '--version' || args[0] === '-V')) {
  console.log(wrapperVersion());
  process.exit(0);
}

const child = spawn(binary, args, {
  stdio: 'inherit',
  windowsHide: false
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
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
