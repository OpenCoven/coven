#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync
} from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { gunzipSync, gzipSync } from 'node:zlib';

const BASE_PROFILE_CONFIG = {
  contractProfile: 'coven.automations.v1',
  bundleSchemaVersion: 'coven.automations.bundle.v1',
  specRelativeDir: 'spec/coven-automations/v1',
  archiveRoot: 'coven-automations-v1',
  bundlePrefix: 'coven-automations-v1-contract',
  label: 'Automations protocol'
};

const BASE_V1 = {
  commit: '8a796807b37d4ad33eaeca37498debf1ca55dd49',
  bundle: '512460db71d4257d7a4d33ea306578e66d9ac499d9384eb9c2b8e2b4e2e32363',
  content: '3c145eb92a93426ed64631f6487a8cd12903b0a49a6e752269f594ac50a779f5',
  files: 17
};

function compareLexically(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function runGit(repoRoot, args) {
  const result = spawnSync('git', args, {
    cwd: repoRoot,
    encoding: 'utf8'
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(
      `git ${args.join(' ')} failed: ${String(result.stderr || result.stdout).trim()}`
    );
  }
  return result.stdout.trim();
}

function runGitBytes(repoRoot, args) {
  const result = spawnSync('git', args, {
    cwd: repoRoot,
    encoding: null
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(
      `git ${args.join(' ')} failed: ${String(result.stderr || result.stdout).trim()}`
    );
  }
  return result.stdout;
}

function listContractFiles(specDir, relativeDir = '') {
  const absoluteDir = path.join(specDir, relativeDir);
  const files = [];
  for (const entry of readdirSync(absoluteDir, { withFileTypes: true })) {
    const relativePath = path.posix.join(relativeDir.split(path.sep).join(path.posix.sep), entry.name);
    const absolutePath = path.join(specDir, ...relativePath.split('/'));
    const stat = lstatSync(absolutePath);
    if (stat.isSymbolicLink() || (!stat.isDirectory() && !stat.isFile())) {
      throw new Error(
        `Automations protocol input tree must contain only regular files and directories: ${relativePath}`
      );
    }
    if (stat.isDirectory()) {
      files.push(...listContractFiles(specDir, relativePath));
    } else {
      files.push(relativePath);
    }
  }
  return files.sort(compareLexically);
}

function writeString(buffer, offset, length, value) {
  const bytes = Buffer.from(value);
  if (bytes.length > length) {
    throw new Error(`Tar header value exceeds ${length} bytes: ${value}`);
  }
  bytes.copy(buffer, offset);
}

function writeOctal(buffer, offset, length, value) {
  const text = value.toString(8).padStart(length - 1, '0');
  if (text.length > length - 1) {
    throw new Error(`Tar header numeric value exceeds ${length} bytes: ${value}`);
  }
  writeString(buffer, offset, length, `${text}\0`);
}

function splitTarPath(name) {
  if (Buffer.byteLength(name) <= 100) {
    return { name, prefix: '' };
  }
  for (let index = name.lastIndexOf('/'); index > 0; index = name.lastIndexOf('/', index - 1)) {
    const prefix = name.slice(0, index);
    const basename = name.slice(index + 1);
    if (Buffer.byteLength(prefix) <= 155 && Buffer.byteLength(basename) <= 100) {
      return { name: basename, prefix };
    }
  }
  throw new Error(`Tar entry path exceeds the ustar limit: ${name}`);
}

function createTarHeader(name, size) {
  const header = Buffer.alloc(512, 0);
  const split = splitTarPath(name);
  writeString(header, 0, 100, split.name);
  writeOctal(header, 100, 8, 0o644);
  writeOctal(header, 108, 8, 0);
  writeOctal(header, 116, 8, 0);
  writeOctal(header, 124, 12, size);
  writeOctal(header, 136, 12, 0);
  header.fill(0x20, 148, 156);
  writeString(header, 156, 1, '0');
  writeString(header, 257, 6, 'ustar');
  writeString(header, 263, 2, '00');
  writeString(header, 345, 155, split.prefix);
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  writeString(header, 148, 8, `${checksum.toString(8).padStart(6, '0')}\0 `);
  return header;
}

function createTarGz(entries) {
  const parts = [];
  for (const entry of entries) {
    parts.push(createTarHeader(entry.name, entry.data.length));
    parts.push(entry.data);
    const remainder = entry.data.length % 512;
    if (remainder !== 0) {
      parts.push(Buffer.alloc(512 - remainder, 0));
    }
  }
  parts.push(Buffer.alloc(1024, 0));
  const gzip = gzipSync(Buffer.concat(parts), { level: 9, mtime: 0 });
  gzip.writeUInt32LE(0, 4);
  gzip[9] = 0xff;
  return gzip;
}

function assertCleanSource(repoRoot, sourceCommit, label) {
  if (!/^[0-9a-f]{40}$/.test(sourceCommit)) {
    throw new Error(`${label} source commit must be a lowercase 40-character Git SHA: ${sourceCommit}`);
  }
  const head = runGit(repoRoot, ['rev-parse', 'HEAD']);
  if (head !== sourceCommit) {
    throw new Error(`${label} source commit ${sourceCommit} does not match HEAD ${head}`);
  }
  const status = runGit(repoRoot, ['status', '--porcelain=v1', '--untracked-files=all']);
  if (status !== '') {
    throw new Error(`${label} input tree is dirty:\n${status}`);
  }
}

function trackedContractFiles(repoRoot, sourceCommit, specRelativeDir, label) {
  const tree = runGitBytes(repoRoot, [
    'ls-tree',
    '-r',
    '-z',
    '--full-tree',
    sourceCommit,
    '--',
    specRelativeDir
  ]).toString('utf8');
  const prefix = `${specRelativeDir}/`;
  return tree
    .split('\0')
    .filter(Boolean)
    .map((record) => {
      const separator = record.indexOf('\t');
      if (separator === -1) {
        throw new Error(`Invalid git ls-tree record for ${label}: ${record}`);
      }
      const [mode, type, object] = record.slice(0, separator).split(' ');
      const sourcePath = record.slice(separator + 1);
      if (
        type !== 'blob' ||
        !['100644', '100755'].includes(mode) ||
        !sourcePath.startsWith(prefix)
      ) {
        throw new Error(
          `${label} input tree must contain only regular files: ${sourcePath}`
        );
      }
      return {
        path: sourcePath.slice(prefix.length),
        object
      };
    })
    .sort((left, right) => compareLexically(left.path, right.path));
}

function normalizeProfileConfig(config) {
  const normalized = {
    contractProfile: String(config.contractProfile),
    bundleSchemaVersion: String(config.bundleSchemaVersion),
    specRelativeDir: String(config.specRelativeDir),
    archiveRoot: String(config.archiveRoot),
    bundlePrefix: String(config.bundlePrefix),
    label: String(config.label)
  };
  for (const [key, value] of Object.entries(normalized)) {
    if (!value) {
      throw new Error(`Contract profile packaging config ${key} is required`);
    }
  }
  return normalized;
}

export function packageContractProfile({ repoRoot, outputDir, sourceCommit, config }) {
  const profile = normalizeProfileConfig(config);
  const normalizedRepoRoot = path.resolve(String(repoRoot));
  const normalizedOutputDir = path.resolve(String(outputDir));
  const normalizedSourceCommit = String(sourceCommit).trim();
  assertCleanSource(normalizedRepoRoot, normalizedSourceCommit, profile.label);

  const specDir = path.join(normalizedRepoRoot, ...profile.specRelativeDir.split('/'));
  const trackedFiles = trackedContractFiles(
    normalizedRepoRoot,
    normalizedSourceCommit,
    profile.specRelativeDir,
    profile.label
  );
  const filesystemFiles = listContractFiles(specDir);
  if (
    filesystemFiles.join('\n') !== trackedFiles.map((file) => file.path).join('\n')
  ) {
    throw new Error(
      `${profile.label} input tree does not exactly match tracked source files`
    );
  }
  const artifact = buildContractProfileArtifact(
    normalizedRepoRoot,
    normalizedSourceCommit,
    profile,
    trackedFiles
  );

  mkdirSync(normalizedOutputDir, { recursive: true });
  const bundleName = `${profile.bundlePrefix}-${normalizedSourceCommit}.tar.gz`;
  const bundlePath = path.join(normalizedOutputDir, bundleName);
  const manifestPath = path.join(normalizedOutputDir, 'manifest.json');
  writeFileSync(bundlePath, artifact.bundleBytes);
  writeFileSync(manifestPath, artifact.manifestBytes);

  return {
    bundlePath,
    manifestPath,
    bundleSha256: artifact.bundleSha256,
    contractContentSha256: artifact.contractContentSha256
  };
}

function buildContractProfileArtifact(repoRoot, sourceCommit, profile, trackedFiles) {
  const files = trackedFiles.map(({ path: relativePath, object }) => {
    const bytes = runGitBytes(repoRoot, ['cat-file', 'blob', object]);
    return {
      path: relativePath,
      sha256: sha256(bytes),
      size: bytes.length,
      bytes
    };
  });
  if (files.length === 0) {
    throw new Error(`${profile.label} input tree is empty: ${specDir}`);
  }

  const contractContentSha256 = sha256(
    Buffer.from(files.map((file) => `${file.path}\0${file.sha256}\n`).join(''))
  );
  const manifest = {
    schemaVersion: profile.bundleSchemaVersion,
    contractProfile: profile.contractProfile,
    sourceCommit,
    contractContentSha256,
    files: files.map(({ path: relativePath, sha256: digest, size }) => ({
      path: relativePath,
      sha256: digest,
      size
    }))
  };
  const manifestBytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
  const archiveEntries = [
    ...files.map((file) => ({
      name: `${profile.archiveRoot}/${file.path}`,
      data: file.bytes
    })),
    {
      name: 'manifest.json',
      data: manifestBytes
    }
  ].sort((left, right) => compareLexically(left.name, right.name));
  const bundleBytes = createTarGz(archiveEntries);

  return {
    bundleBytes,
    manifestBytes,
    bundleSha256: sha256(bundleBytes),
    contractContentSha256
  };
}

export function packageAutomationsProtocol(options) {
  return packageContractProfile({
    ...options,
    config: BASE_PROFILE_CONFIG
  });
}

export function reproduceHistoricalAutomationsProtocolArtifact(repoRoot) {
  const normalizedRepoRoot = path.resolve(String(repoRoot));
  const profile = normalizeProfileConfig(BASE_PROFILE_CONFIG);
  const trackedFiles = trackedContractFiles(
    normalizedRepoRoot,
    BASE_V1.commit,
    profile.specRelativeDir,
    profile.label
  );
  const artifact = buildContractProfileArtifact(
    normalizedRepoRoot,
    BASE_V1.commit,
    profile,
    trackedFiles
  );
  const reproduced = {
    sourceCommit: BASE_V1.commit,
    bundleSha256: artifact.bundleSha256,
    contractContentSha256: artifact.contractContentSha256,
    fileCount: trackedFiles.length
  };
  if (
    reproduced.bundleSha256 !== BASE_V1.bundle ||
    reproduced.contractContentSha256 !== BASE_V1.content ||
    reproduced.fileCount !== BASE_V1.files
  ) {
    throw new Error(
      `Historical Automations v1 artifact reproduction mismatch: ${JSON.stringify(reproduced)}`
    );
  }
  return reproduced;
}

function tarString(header, offset, length) {
  return header
    .subarray(offset, offset + length)
    .toString('utf8')
    .replace(/\0.*$/s, '');
}

function tarOctal(header, offset, length) {
  const value = tarString(header, offset, length).trim();
  if (!/^[0-7]+$/.test(value)) {
    throw new Error(`Invalid tar octal field: ${JSON.stringify(value)}`);
  }
  return Number.parseInt(value, 8);
}

function parseTarGz(bundleBytes, label) {
  const canonicalGzipHeader = Buffer.from([
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff
  ]);
  if (
    bundleBytes.length < canonicalGzipHeader.length ||
    !bundleBytes.subarray(0, canonicalGzipHeader.length).equals(canonicalGzipHeader)
  ) {
    throw new Error(`${label} bundle gzip header is not normalized`);
  }
  let tar;
  try {
    tar = gunzipSync(bundleBytes);
  } catch (error) {
    throw new Error(`${label} bundle gzip is invalid: ${error?.message ?? String(error)}`);
  }
  const entries = [];
  let offset = 0;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      if (
        offset + 1024 !== tar.length ||
        !tar.subarray(offset, offset + 1024).every((byte) => byte === 0)
      ) {
        throw new Error(`${label} tar has an invalid terminator`);
      }
      return entries;
    }
    const expectedChecksum = tarOctal(header, 148, 8);
    const checksumHeader = Buffer.from(header);
    checksumHeader.fill(0x20, 148, 156);
    const actualChecksum = checksumHeader.reduce((sum, byte) => sum + byte, 0);
    if (actualChecksum !== expectedChecksum) {
      throw new Error(`${label} tar header checksum mismatch`);
    }
    const basename = tarString(header, 0, 100);
    const prefix = tarString(header, 345, 155);
    const name = prefix ? `${prefix}/${basename}` : basename;
    const type = tarString(header, 156, 1);
    const mode = tarOctal(header, 100, 8);
    const uid = tarOctal(header, 108, 8);
    const gid = tarOctal(header, 116, 8);
    const size = tarOctal(header, 124, 12);
    const mtime = tarOctal(header, 136, 12);
    if (
      type !== '0' ||
      mode !== 0o644 ||
      uid !== 0 ||
      gid !== 0 ||
      mtime !== 0 ||
      !header.equals(createTarHeader(name, size))
    ) {
      throw new Error(`${label} tar metadata is not normalized for ${name}`);
    }
    if (
      name === '' ||
      name.startsWith('/') ||
      name.includes('\\') ||
      name.split('/').some((component) => component === '' || component === '.' || component === '..')
    ) {
      throw new Error(`${label} tar contains unsafe path ${JSON.stringify(name)}`);
    }
    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    if (dataEnd > tar.length) {
      throw new Error(`${label} tar entry exceeds archive bounds: ${name}`);
    }
    const paddedEnd = dataStart + Math.ceil(size / 512) * 512;
    if (!tar.subarray(dataEnd, paddedEnd).every((byte) => byte === 0)) {
      throw new Error(`${label} tar padding is not normalized for ${name}`);
    }
    entries.push({ name, data: tar.subarray(dataStart, dataEnd) });
    offset = paddedEnd;
  }
  throw new Error(`${label} tar is missing its terminator`);
}

export function verifyContractProfileBundle({
  bundlePath,
  expectedSourceCommit,
  expectedBundleSha256,
  config
}) {
  const profile = normalizeProfileConfig(config);
  const normalizedSourceCommit = String(expectedSourceCommit).trim();
  if (!/^[0-9a-f]{40}$/.test(normalizedSourceCommit)) {
    throw new Error(`Expected source commit must be a lowercase 40-character Git SHA: ${normalizedSourceCommit}`);
  }
  const normalizedExpectedBundleSha256 = String(expectedBundleSha256).trim();
  if (!/^[0-9a-f]{64}$/.test(normalizedExpectedBundleSha256)) {
    throw new Error('Expected bundle SHA-256 must be 64 lowercase hexadecimal characters');
  }
  const bundleBytes = readFileSync(path.resolve(String(bundlePath)));
  const actualBundleSha256 = sha256(bundleBytes);
  if (actualBundleSha256 !== normalizedExpectedBundleSha256) {
    throw new Error(
      `Automations protocol bundle SHA-256 mismatch: expected ${normalizedExpectedBundleSha256}, got ${actualBundleSha256}`
    );
  }
  const entries = parseTarGz(bundleBytes, profile.label);
  const names = entries.map((entry) => entry.name);
  if (new Set(names).size !== names.length) {
    throw new Error('Automations protocol bundle contains duplicate archive entries');
  }
  const sortedNames = [...names].sort(compareLexically);
  if (names.join('\n') !== sortedNames.join('\n')) {
    throw new Error('Automations protocol bundle entries are not lexically ordered');
  }
  const manifestEntry = entries.find((entry) => entry.name === 'manifest.json');
  if (!manifestEntry) {
    throw new Error('Automations protocol bundle is missing manifest.json');
  }
  let manifest;
  try {
    manifest = JSON.parse(manifestEntry.data.toString('utf8'));
  } catch (error) {
    throw new Error(`Automations protocol manifest is invalid JSON: ${error?.message ?? String(error)}`);
  }
  if (
    manifest.schemaVersion !== profile.bundleSchemaVersion ||
    manifest.contractProfile !== profile.contractProfile
  ) {
    throw new Error('Automations protocol manifest profile is invalid');
  }
  if (manifest.sourceCommit !== normalizedSourceCommit) {
    throw new Error(
      `Automations protocol source commit mismatch: expected ${normalizedSourceCommit}, got ${manifest.sourceCommit}`
    );
  }
  if (!Array.isArray(manifest.files) || manifest.files.length === 0) {
    throw new Error('Automations protocol manifest must list at least one contract file');
  }
  const contractEntries = new Map(
    entries
      .filter((entry) => entry.name !== 'manifest.json')
      .map((entry) => {
        const prefix = `${profile.archiveRoot}/`;
        if (!entry.name.startsWith(prefix)) {
          throw new Error(`Unexpected Automations protocol archive entry: ${entry.name}`);
        }
        return [entry.name.slice(prefix.length), entry.data];
      })
  );
  const manifestPaths = manifest.files.map((file) => file.path);
  if (
    new Set(manifestPaths).size !== manifestPaths.length ||
    manifestPaths.join('\n') !== [...manifestPaths].sort(compareLexically).join('\n') ||
    manifestPaths.join('\n') !== [...contractEntries.keys()].sort(compareLexically).join('\n')
  ) {
    throw new Error('Automations protocol manifest paths do not exactly match archive entries');
  }
  for (const file of manifest.files) {
    if (
      typeof file.path !== 'string' ||
      !/^[0-9a-f]{64}$/.test(file.sha256) ||
      !Number.isSafeInteger(file.size) ||
      file.size < 0
    ) {
      throw new Error(`Automations protocol manifest file record is invalid: ${JSON.stringify(file)}`);
    }
    const bytes = contractEntries.get(file.path);
    if (bytes.length !== file.size) {
      throw new Error(`Automations protocol file size mismatch for ${file.path}`);
    }
    const actualFileSha256 = sha256(bytes);
    if (actualFileSha256 !== file.sha256) {
      throw new Error(`Automations protocol file SHA-256 mismatch for ${file.path}`);
    }
  }
  const actualContentSha256 = sha256(
    Buffer.from(manifest.files.map((file) => `${file.path}\0${file.sha256}\n`).join(''))
  );
  if (actualContentSha256 !== manifest.contractContentSha256) {
    throw new Error(
      `Automations protocol content SHA-256 mismatch: expected ${manifest.contractContentSha256}, got ${actualContentSha256}`
    );
  }
  return {
    sourceCommit: manifest.sourceCommit,
    bundleSha256: actualBundleSha256,
    contractContentSha256: actualContentSha256,
    fileCount: manifest.files.length
  };
}

export function verifyAutomationsProtocolBundle(options) {
  return verifyContractProfileBundle({
    ...options,
    config: BASE_PROFILE_CONFIG
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
        'Usage: package-automations-protocol.mjs verify --bundle <archive> --source-commit <sha> --sha256 <digest>'
      );
    }
    const result = verifyAutomationsProtocolBundle({
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
    throw new Error('Usage: package-automations-protocol.mjs --output <directory>');
  }
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const sourceCommit = process.env.SOURCE_COMMIT;
  if (!sourceCommit) {
    throw new Error('SOURCE_COMMIT is required');
  }
  const result = packageAutomationsProtocol({
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
