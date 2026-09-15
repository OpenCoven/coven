import { realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export function isMainModule(moduleUrl) {
  if (!process.argv[1]) return false;
  try {
    // Node resolves symlinks in module URLs, but argv can retain the caller's alias.
    return realpathSync(fileURLToPath(moduleUrl)) === realpathSync(process.argv[1]);
  } catch (error) {
    if (error?.code !== 'ENOENT' && error?.code !== 'ENOTDIR') throw error;
    return false;
  }
}
