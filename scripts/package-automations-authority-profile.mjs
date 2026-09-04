#!/usr/bin/env node

import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import {
  packageContractProfile,
  verifyContractProfileBundle
} from './package-automations-protocol.mjs';

const AUTHORITY_PROFILE_CONFIG = {
  contractProfile: 'coven.automations.authority.v1',
  bundleSchemaVersion: 'coven.contract-profile.bundle.v1',
  specRelativeDir: 'spec/coven-automations/authority/v1',
  archiveRoot: 'coven-automations-authority-v1',
  bundlePrefix: 'coven-automations-authority-v1-contract',
  label: 'Automations authority profile'
};

export function packageAutomationsAuthorityProfile(options) {
  return packageContractProfile({
    ...options,
    config: AUTHORITY_PROFILE_CONFIG
  });
}

export function verifyAutomationsAuthorityProfileBundle(options) {
  return verifyContractProfileBundle({
    ...options,
    config: AUTHORITY_PROFILE_CONFIG
  });
}

function optionValue(args, name) {
  const exactIndex = args.indexOf(name);
  if (exactIndex !== -1) {
    const value = args[exactIndex + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`${name} requires a value`);
    }
    return value;
  }
  const prefix = `${name}=`;
  const inline = args.find((argument) => argument.startsWith(prefix));
  return inline?.slice(prefix.length);
}

function main() {
  const args = process.argv.slice(2);
  if (args[0] === 'verify') {
    const verifyArgs = args.slice(1);
    const bundlePath = optionValue(verifyArgs, '--bundle');
    const expectedSourceCommit = optionValue(verifyArgs, '--source-commit');
    const expectedBundleSha256 = optionValue(verifyArgs, '--sha256');
    if (!bundlePath || !expectedSourceCommit || !expectedBundleSha256) {
      throw new Error(
        'Usage: package-automations-authority-profile.mjs verify --bundle <archive> --source-commit <sha> --sha256 <digest>'
      );
    }
    const result = verifyAutomationsAuthorityProfileBundle({
      bundlePath,
      expectedSourceCommit,
      expectedBundleSha256
    });
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return;
  }

  const packageArgs = args[0] === 'package' ? args.slice(1) : args;
  const outputDir = optionValue(packageArgs, '--output');
  if (!outputDir) {
    throw new Error('Usage: package-automations-authority-profile.mjs --output <directory>');
  }
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const sourceCommit = process.env.SOURCE_COMMIT;
  if (!sourceCommit) {
    throw new Error('SOURCE_COMMIT is required');
  }
  const result = packageAutomationsAuthorityProfile({
    repoRoot,
    outputDir,
    sourceCommit
  });
  process.stdout.write(
    `${JSON.stringify({
      bundle: path.basename(result.bundlePath),
      bundleSha256: result.bundleSha256,
      contractContentSha256: result.contractContentSha256,
      sourceCommit
    })}\n`
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    main();
  } catch (error) {
    console.error(error?.message ?? String(error));
    process.exitCode = 1;
  }
}
