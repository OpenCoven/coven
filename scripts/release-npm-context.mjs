#!/usr/bin/env node

import { pathToFileURL } from 'node:url';

const VERSION = '(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)';
const STABLE_TAG = new RegExp(`^v(${VERSION})$`);

export function parseReleaseTag(tag) {
  const stable = STABLE_TAG.exec(tag);
  if (stable) {
    return {
      releaseTag: tag,
      npmVersion: stable[1]
    };
  }

  throw new Error(`Release tag ${JSON.stringify(tag)} must be a stable vX.Y.Z tag.`);
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && moduleUrl === pathToFileURL(argv1).href;
}

if (isMainModule()) {
  const context = parseReleaseTag(process.argv[2] ?? '');
  process.stdout.write(
    [`release_tag=${context.releaseTag}`, `npm_version=${context.npmVersion}`, ''].join('\n')
  );
}
