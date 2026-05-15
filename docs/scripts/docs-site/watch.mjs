#!/usr/bin/env node

/**
 * Rebuild the curated docs site when markdown, config, or docs assets change.
 */

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const rootDir = process.cwd();
let running = false;
let pending = false;

function runBuild() {
  if (running) {
    pending = true;
    return;
  }

  running = true;
  const child = spawn(process.execPath, ['scripts/docs-site/build.mjs'], {
    cwd: rootDir,
    stdio: 'inherit'
  });

  child.on('exit', (code) => {
    running = false;
    if (code !== 0) {
      console.error(`docs build failed with exit code ${code}`);
    }
    if (pending) {
      pending = false;
      runBuild();
    }
  });
}

function shouldSkip(relativePath) {
  return (
    relativePath === 'dist' ||
    relativePath.startsWith(`dist${path.sep}`) ||
    relativePath === 'node_modules' ||
    relativePath.startsWith(`node_modules${path.sep}`) ||
    relativePath === '.git' ||
    relativePath.startsWith(`.git${path.sep}`)
  );
}

function watchDirectory(directory) {
  const relativeDirectory = path.relative(rootDir, directory) || '.';
  if (shouldSkip(relativeDirectory)) return;

  fs.watch(directory, (_event, filename) => {
    if (filename) {
      const changed = path.join(relativeDirectory === '.' ? '' : relativeDirectory, String(filename));
      if (shouldSkip(changed)) return;
    }
    runBuild();
  });

  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    watchDirectory(path.join(directory, entry.name));
  }
}

console.log('Watching Coven docs...');
runBuild();

watchDirectory(rootDir);
