#!/usr/bin/env node

// Builds the release's dependency inventory as a CycloneDX 1.5 SBOM (#805).
//
// The inventory is read from the exact tagged source: every package
// Cargo.lock resolves (registry crates with their checksums, git crates with
// their pinned commits, workspace members), the coven-code engine the CLI
// pins by per-asset digest, and the published npm packages themselves. Build
// tooling (locked workflow actions and the pinned toolchains) is listed with
// scope "excluded": it shaped the release but is not part of it.
//
// Cargo.lock resolves a superset of what any one native binary links
// (dev-dependencies and every target's dependencies are included). The
// inventory says so in its metadata rather than claiming per-binary precision.
// Output is deterministic for a given source, tag, and timestamp.

import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const REPOSITORY_VCS = 'git+https://github.com/OpenCoven/coven';
const NATIVE_PACKAGES = [
  '@opencoven/cli-linux-x64',
  '@opencoven/cli-macos',
  '@opencoven/cli-macos-x64',
  '@opencoven/cli-windows'
];
const SCOPE_NOTE =
  'Cargo.lock resolves a superset of what each native binary links: dev-dependencies and every target are included.';

/// Locale-independent ordering, so the same inputs give the same bytes on
/// every runner.
function byCodePoint(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

function parseOptions(args) {
  const options = new Map();
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg?.startsWith('--')) {
      throw new Error(`Unexpected argument ${JSON.stringify(arg)}.`);
    }
    const value = args[index + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`Missing value for ${arg}.`);
    }
    options.set(arg.slice(2), value);
    index += 1;
  }
  return options;
}

function requiredOption(options, key) {
  const value = options.get(key);
  if (!value) {
    throw new Error(`Missing required option --${key}.`);
  }
  return value;
}

function unquote(value, where) {
  const match = /^"((?:[^"\\]|\\.)*)"$/.exec(value.trim());
  if (!match) {
    throw new Error(`${where}: expected a quoted string, got ${value}.`);
  }
  return match[1];
}

/// Parses Cargo.lock's `[[package]]` tables. The lockfile format is generated
/// by Cargo and regular enough that a TOML dependency is not worth taking.
export function parseCargoLock(text) {
  const packages = [];
  let current = null;
  let inDependencies = false;
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  for (const [index, raw] of lines.entries()) {
    const line = raw.trim();
    const where = `Cargo.lock:${index + 1}`;
    if (line === '[[package]]') {
      current = { dependencies: [] };
      packages.push(current);
      inDependencies = false;
      continue;
    }
    if (line.startsWith('[')) {
      if (line !== '[metadata]' && !line.startsWith('[[')) {
        current = null;
      }
      continue;
    }
    if (!current || line === '' || line.startsWith('#')) {
      continue;
    }
    if (inDependencies) {
      if (line === ']') {
        inDependencies = false;
      } else {
        current.dependencies.push(unquote(line.replace(/,$/, ''), where));
      }
      continue;
    }
    const field = /^([a-z_-]+)\s*=\s*(.*)$/.exec(line);
    if (!field) {
      throw new Error(`${where}: cannot parse ${JSON.stringify(line)}.`);
    }
    const [, key, value] = field;
    if (key === 'dependencies') {
      if (value.trim() === '[]') {
        continue;
      }
      if (value.trim() !== '[') {
        throw new Error(`${where}: expected a multi-line dependencies array.`);
      }
      inDependencies = true;
    } else if (['name', 'version', 'source', 'checksum'].includes(key)) {
      current[key] = unquote(value, where);
    }
  }
  for (const entry of packages) {
    if (!entry.name || !entry.version) {
      throw new Error(`Cargo.lock has a package without a name and version: ${JSON.stringify(entry)}.`);
    }
  }
  return packages;
}

/// Reads crates/coven-cli/engine.lock: the engine version and the digest of
/// every per-platform archive the CLI will accept when installing it.
export function parseEngineLock(text) {
  const version = /^version\s*=\s*"([^"]+)"\s*$/m.exec(text)?.[1];
  const section = text.split(/^\[sha256\]\s*$/m)[1] ?? '';
  const assets = {};
  for (const match of section.matchAll(/^"([^"]+)"\s*=\s*"([0-9a-f]{64})"\s*$/gm)) {
    assets[match[1]] = match[2];
  }
  if (!version || Object.keys(assets).length === 0) {
    throw new Error('engine.lock must name a version and at least one sha256 asset digest.');
  }
  return { version, assets };
}

function encodePurlName(name) {
  return name.split('/').map(encodeURIComponent).join('/');
}

function cratePurl(entry) {
  const base = `pkg:cargo/${encodeURIComponent(entry.name)}@${encodeURIComponent(entry.version)}`;
  return entry.gitCommit ? `${base}?vcs_url=${encodeURIComponent(`git+${entry.gitUrl}@${entry.gitCommit}`)}` : base;
}

/// Classifies one lockfile package as a workspace member, a registry crate
/// (which must carry a checksum), or a git crate (which must be pinned to a
/// commit). Anything else is refused rather than listed without provenance.
function classifyCrate(entry) {
  if (entry.source === undefined) {
    return { ...entry, kind: 'workspace' };
  }
  if (entry.source === 'registry+https://github.com/rust-lang/crates.io-index') {
    if (!/^[0-9a-f]{64}$/.test(entry.checksum ?? '')) {
      throw new Error(`Cargo.lock registry crate ${entry.name} ${entry.version} has no sha256 checksum.`);
    }
    return { ...entry, kind: 'registry' };
  }
  const git = /^git\+(https:\/\/[^?#]+)(?:\?[^#]*)?#([0-9a-f]{40})$/.exec(entry.source);
  if (git) {
    return { ...entry, kind: 'git', gitUrl: git[1], gitCommit: git[2] };
  }
  throw new Error(`Cargo.lock crate ${entry.name} ${entry.version} has an unsupported source ${JSON.stringify(entry.source)}.`);
}

/// Resolves a lockfile dependency string ("name", "name version", or
/// "name version (source)") to exactly one package.
function resolveDependency(reference, crates, from) {
  const [name, version] = reference.split(' ');
  const matches = crates.filter((entry) => entry.name === name && (version === undefined || entry.version === version));
  if (matches.length !== 1) {
    throw new Error(`Cargo.lock dependency ${JSON.stringify(reference)} of ${from} resolves to ${matches.length} packages.`);
  }
  return matches[0];
}

function serialNumber(releaseTag, sourceCommit) {
  const hex = createHash('sha256').update(`coven-release-inventory\0${releaseTag}\0${sourceCommit}`).digest('hex');
  const variant = ((parseInt(hex[16], 16) & 0x3) | 0x8).toString(16);
  return `urn:uuid:${hex.slice(0, 8)}-${hex.slice(8, 12)}-5${hex.slice(13, 16)}-${variant}${hex.slice(17, 20)}-${hex.slice(20, 32)}`;
}

export function buildDependencyInventory({
  releaseTag,
  npmVersion,
  sourceCommit,
  sourceDateEpoch,
  cargoLock,
  engineLock,
  actionsLock = null,
  rustToolchain = null,
  nodeVersion = null
}) {
  if (!/^[0-9a-f]{40}$/.test(String(sourceCommit ?? ''))) {
    throw new Error(`Inventory source commit must be a 40-character SHA, got ${JSON.stringify(sourceCommit)}.`);
  }
  if (!Number.isSafeInteger(sourceDateEpoch) || sourceDateEpoch < 0) {
    throw new Error(`Inventory SOURCE_DATE_EPOCH must be a non-negative integer, got ${JSON.stringify(sourceDateEpoch)}.`);
  }
  const crates = parseCargoLock(cargoLock).map(classifyCrate);
  const refs = new Map();
  const components = [];
  const add = (component) => {
    if (refs.has(component['bom-ref'])) {
      throw new Error(`Inventory has two components named ${component['bom-ref']}.`);
    }
    refs.set(component['bom-ref'], component);
    components.push(component);
  };

  for (const entry of crates) {
    const purl = cratePurl(entry);
    const component = { type: 'library', 'bom-ref': purl, name: entry.name, version: entry.version, purl };
    if (entry.kind === 'registry') {
      component.hashes = [{ alg: 'SHA-256', content: entry.checksum }];
    } else if (entry.kind === 'git') {
      component.externalReferences = [{ type: 'vcs', url: `${entry.gitUrl}#${entry.gitCommit}` }];
    } else {
      component.properties = [{ name: 'coven:workspace-member', value: 'true' }];
    }
    entry.bomRef = purl;
    add(component);
  }

  const engine = parseEngineLock(engineLock);
  add({
    type: 'application',
    'bom-ref': `pkg:github/OpenCoven/coven-code@v${engine.version}`,
    name: 'coven-code',
    version: engine.version,
    purl: `pkg:github/OpenCoven/coven-code@v${engine.version}`,
    scope: 'optional',
    properties: Object.entries(engine.assets)
      .sort(([left], [right]) => byCodePoint(left, right))
      .map(([asset, digest]) => ({ name: `coven:engine-asset-sha256:${asset}`, value: digest }))
  });
  for (const packageName of NATIVE_PACKAGES) {
    const purl = `pkg:npm/${encodePurlName(packageName)}@${npmVersion}`;
    add({ type: 'application', 'bom-ref': purl, name: packageName, version: npmVersion, purl, scope: 'optional' });
  }
  for (const [name, entry] of Object.entries(actionsLock?.actions ?? {})) {
    const purl = `pkg:githubactions/${name}@${entry.sha}`;
    add({
      type: 'application',
      'bom-ref': purl,
      name,
      version: entry.ref,
      purl,
      scope: 'excluded',
      properties: [{ name: 'coven:build-tool', value: 'workflow action' }]
    });
  }

  const dependencies = crates
    .map((entry) => ({
      ref: entry.bomRef,
      dependsOn: [...new Set(entry.dependencies.map((reference) => resolveDependency(reference, crates, entry.name).bomRef))].sort()
    }))
    .sort((left, right) => byCodePoint(left.ref, right.ref));

  const wrapperPurl = `pkg:npm/${encodePurlName('@opencoven/cli')}@${npmVersion}`;
  const properties = [
    { name: 'coven:release-tag', value: releaseTag },
    { name: 'coven:source-commit', value: sourceCommit },
    { name: 'coven:inventory-scope', value: SCOPE_NOTE }
  ];
  if (rustToolchain) {
    properties.push({ name: 'coven:rust-toolchain', value: rustToolchain });
  }
  if (nodeVersion) {
    properties.push({ name: 'coven:node', value: nodeVersion });
  }
  return {
    bomFormat: 'CycloneDX',
    specVersion: '1.5',
    serialNumber: serialNumber(releaseTag, sourceCommit),
    version: 1,
    metadata: {
      timestamp: new Date(sourceDateEpoch * 1000).toISOString().replace(/\.000Z$/, 'Z'),
      tools: { components: [{ type: 'application', name: 'coven release-dependency-inventory', version: '1' }] },
      component: {
        type: 'application',
        'bom-ref': wrapperPurl,
        name: '@opencoven/cli',
        version: npmVersion,
        purl: wrapperPurl,
        externalReferences: [{ type: 'vcs', url: `${REPOSITORY_VCS}#${sourceCommit}` }]
      },
      properties
    },
    components: components.sort((left, right) => byCodePoint(left['bom-ref'], right['bom-ref'])),
    dependencies
  };
}

export function inventoryAssetName(npmVersion) {
  return `coven-v${npmVersion}-sbom.cdx.json`;
}

function main() {
  const options = parseOptions(process.argv.slice(2));
  const sourceDir = path.resolve(requiredOption(options, 'source-dir'));
  const readOptional = (relativePath) => {
    const filePath = path.join(sourceDir, relativePath);
    return existsSync(filePath) ? readFileSync(filePath, 'utf8') : null;
  };
  const releaseTag = requiredOption(options, 'release-tag');
  const npmVersion = releaseTag.replace(/^v/, '');
  const rustToolchainText = readOptional('rust-toolchain.toml');
  const actionsLockText = readOptional('.github/actions-lock.json');
  const inventory = buildDependencyInventory({
    releaseTag,
    npmVersion,
    sourceCommit: requiredOption(options, 'source-commit'),
    sourceDateEpoch: Number(requiredOption(options, 'source-date-epoch')),
    cargoLock: readFileSync(path.join(sourceDir, 'Cargo.lock'), 'utf8'),
    engineLock: readFileSync(path.join(sourceDir, 'crates', 'coven-cli', 'engine.lock'), 'utf8'),
    actionsLock: actionsLockText ? JSON.parse(actionsLockText) : null,
    rustToolchain: rustToolchainText ? /^\s*channel\s*=\s*"([^"]+)"/m.exec(rustToolchainText)?.[1] ?? null : null,
    nodeVersion: readOptional('.node-version')?.trim() || null
  });
  const outputDir = path.resolve(requiredOption(options, 'output-dir'));
  mkdirSync(outputDir, { recursive: true });
  const outputPath = path.join(outputDir, inventoryAssetName(npmVersion));
  writeFileSync(outputPath, `${JSON.stringify(inventory, null, 2)}\n`);
  console.log(`${outputPath}: ${inventory.components.length} components`);
}

if (isMainModule()) {
  try {
    main();
  } catch (error) {
    console.error(error?.message ?? String(error));
    process.exit(1);
  }
}
