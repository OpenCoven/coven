#!/usr/bin/env node

import { nativeTargetNamesForPackageSet } from './publish-npm.mjs';
import { pathToFileURL } from 'node:url';

const TARGETS = {
  macos: {
    'npm-target': 'macos',
    'rust-target': 'aarch64-apple-darwin',
    runner: 'macos-26',
    binary: 'coven'
  },
  'macos-x64': {
    'npm-target': 'macos-x64',
    'rust-target': 'x86_64-apple-darwin',
    runner: 'macos-15-intel',
    binary: 'coven'
  },
  'linux-x64': {
    'npm-target': 'linux-x64',
    'rust-target': 'x86_64-unknown-linux-gnu',
    runner: 'ubuntu-latest',
    binary: 'coven'
  },
  windows: {
    'npm-target': 'windows',
    'rust-target': 'x86_64-pc-windows-msvc',
    runner: 'windows-latest',
    binary: 'coven.exe'
  }
};

export function platformMatrix(packageSet = 'post-intel') {
  return {
    include: nativeTargetNamesForPackageSet(packageSet).map((targetName) => TARGETS[targetName])
  };
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

if (isMainModule()) {
  process.stdout.write(`platform_matrix=${JSON.stringify(platformMatrix(process.argv[2] ?? 'post-intel'))}\n`);
}
