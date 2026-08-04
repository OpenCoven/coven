#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const PRE_INTEL_NATIVE_PACKAGES = [
  '@opencoven/cli-macos',
  '@opencoven/cli-linux-x64',
  '@opencoven/cli-windows'
];
const POST_INTEL_NATIVE_PACKAGES = [
  '@opencoven/cli-macos',
  '@opencoven/cli-macos-x64',
  '@opencoven/cli-linux-x64',
  '@opencoven/cli-windows'
];

export function nativePackageSet(optionalDependencies) {
  const nativePackages = Object.keys(optionalDependencies ?? {})
    .filter((name) => name.startsWith('@opencoven/cli-'))
    .sort();
  const expectedSets = [
    ['pre-intel', PRE_INTEL_NATIVE_PACKAGES],
    ['post-intel', POST_INTEL_NATIVE_PACKAGES]
  ];

  for (const [name, expected] of expectedSets) {
    if (nativePackages.length === expected.length && nativePackages.every((value, index) => value === [...expected].sort()[index])) {
      return name;
    }
  }

  throw new Error(
    `Unsupported native package set: ${nativePackages.join(', ') || '(none)'}. ` +
      'Recovery only supports the complete pre-Intel or post-Intel package sets.'
  );
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

if (isMainModule()) {
  const baseCommit = process.argv[2];
  if (!baseCommit) {
    throw new Error('Usage: release-npm-recovery-contract.mjs <base-release-commit>');
  }
  const packageText = execFileSync('git', ['show', `${baseCommit}:npm/coven/package.json`], {
    encoding: 'utf8'
  });
  const packageJson = JSON.parse(packageText);
  const packageSet = nativePackageSet(packageJson.optionalDependencies);
  process.stdout.write(`native_package_set=${packageSet}\n`);
}
