#!/usr/bin/env node

import { pathToFileURL } from 'node:url';

const VERSION = '(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)';
const STABLE_TAG = new RegExp(`^v(${VERSION})$`);
const RECOVERY_TAG = new RegExp(`^v(${VERSION})-recovery\\.([1-9]\\d*)$`);

export function parseReleaseTag(tag) {
  const stable = STABLE_TAG.exec(tag);
  if (stable) {
    return {
      releaseMode: 'normal',
      releaseTag: tag,
      npmVersion: stable[1],
      recoveryAttempt: null
    };
  }

  const recovery = RECOVERY_TAG.exec(tag);
  if (recovery) {
    return {
      releaseMode: 'recovery',
      releaseTag: `v${recovery[1]}`,
      npmVersion: recovery[1],
      recoveryAttempt: Number.parseInt(recovery[5], 10)
    };
  }

  throw new Error(
    `Release tag ${JSON.stringify(tag)} must be a stable vX.Y.Z tag or vX.Y.Z-recovery.N tag.`
  );
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && moduleUrl === pathToFileURL(argv1).href;
}

if (isMainModule()) {
  const context = parseReleaseTag(process.argv[2] ?? '');
  process.stdout.write(
    [
      `release_mode=${context.releaseMode}`,
      `release_tag=${context.releaseTag}`,
      `npm_version=${context.npmVersion}`,
      `recovery_attempt=${context.recoveryAttempt ?? ''}`,
      ''
    ].join('\n')
  );
}
