#!/usr/bin/env node

// Keeps every CI and release dependency that runs code on a reviewed,
// immutable reference (#805). A moving tag, branch or toolchain channel could
// otherwise change what PR gate and the release workflows run without any
// change to this repository.
//
//   check          offline: every `uses:` is a full commit SHA recorded in
//                  .github/actions-lock.json with a matching `# ref` comment;
//                  every Rust toolchain step installs rust-toolchain.toml's
//                  channel; every setup-node step reads .node-version, and
//                  every job that runs node installs it that way.
//   verify-remote  online: each locked SHA is still what its tag names, or
//                  is contained in its branch.
//   update         rewrites .github/actions-lock.json from the workflows
//                  after verifying every changed pin against GitHub, so an
//                  action bump (Dependabot's included) is one reviewed
//                  command rather than a hand-edited lock.

import { spawnSync } from 'node:child_process';
import { readFileSync, readdirSync, existsSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const LOCK_PATH = '.github/actions-lock.json';
const RUST_TOOLCHAIN_PATH = 'rust-toolchain.toml';
const NODE_VERSION_PATH = '.node-version';
const RUST_TOOLCHAIN_ACTION = 'dtolnay/rust-toolchain';
const SETUP_NODE_ACTION = 'actions/setup-node';
const USES_LINE = /^\s*(?:- )?uses:\s*(\S+)(?:\s+#\s*(.*?))?\s*$/;
const REMOTE_USES = /^([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)(\/[^@]+)?@(.+)$/;

function isMainModule(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  return Boolean(argv1) && pathToFileURL(argv1).href === moduleUrl;
}

export function readRepositoryInputs(root = repoRoot) {
  const workflowFiles = [];
  for (const dir of ['.github/workflows']) {
    for (const name of readdirSync(path.join(root, dir)).sort()) {
      if (/\.ya?ml$/.test(name)) {
        workflowFiles.push(`${dir}/${name}`);
      }
    }
  }
  const actionsDir = path.join(root, '.github/actions');
  if (existsSync(actionsDir)) {
    for (const name of readdirSync(actionsDir).sort()) {
      for (const file of ['action.yml', 'action.yaml']) {
        if (existsSync(path.join(actionsDir, name, file))) {
          workflowFiles.push(`.github/actions/${name}/${file}`);
        }
      }
    }
  }
  return {
    lock: JSON.parse(readFileSync(path.join(root, LOCK_PATH), 'utf8')),
    rustToolchain: readFileSync(path.join(root, RUST_TOOLCHAIN_PATH), 'utf8'),
    nodeVersion: readFileSync(path.join(root, NODE_VERSION_PATH), 'utf8'),
    files: workflowFiles.map((file) => ({ path: file, text: readFileSync(path.join(root, file), 'utf8') }))
  };
}

export function rustChannel(text) {
  const match = /^\s*channel\s*=\s*"([^"]+)"\s*$/m.exec(text);
  if (!match || !/^\d+\.\d+\.\d+$/.test(match[1])) {
    throw new Error(`${RUST_TOOLCHAIN_PATH} must pin channel to an exact x.y.z release, got ${JSON.stringify(match?.[1] ?? null)}.`);
  }
  return match[1];
}

export function nodeVersion(text) {
  const value = text.trim();
  if (!/^\d+\.\d+\.\d+$/.test(value)) {
    throw new Error(`${NODE_VERSION_PATH} must hold one exact x.y.z Node version, got ${JSON.stringify(value)}.`);
  }
  return value;
}

/// Splits a workflow into top-level jobs, each a list of steps with the lines
/// that belong to it. Text-based on purpose, like the other workflow guards:
/// no YAML dependency, and the layout here is the repository's own.
function workflowSteps(text) {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const steps = [];
  let job = null;
  let current = null;
  for (const [index, line] of lines.entries()) {
    const jobMatch = /^  ([A-Za-z0-9_-]+):\s*$/.exec(line);
    if (jobMatch) {
      job = jobMatch[1];
      current = null;
      continue;
    }
    const stepMatch = /^(\s*)- /.exec(line);
    if (stepMatch && /^\s*- (uses|name|run|id|if|env|with|shell|working-directory|continue-on-error|timeout-minutes):/.test(line)) {
      current = { job, indent: stepMatch[1].length, start: index + 1, lines: [line] };
      steps.push(current);
      continue;
    }
    if (current && (line.trim() === '' || line.startsWith(' '.repeat(current.indent + 2)))) {
      current.lines.push(line);
    } else if (current && line.trim() !== '') {
      current = null;
    }
  }
  return steps;
}

function withValue(step, key) {
  const pattern = new RegExp(`^\\s+${key}:\\s*(.+?)\\s*$`);
  for (const line of step.lines) {
    const match = pattern.exec(line);
    if (match) {
      return match[1].replace(/^['"]|['"]$/g, '');
    }
  }
  return null;
}

export function checkActionPins({ lock, rustToolchain, nodeVersion: nodeVersionText, files }) {
  const findings = [];
  if (lock?.schemaVersion !== 1 || lock?.kind !== 'coven.actions-lock' || typeof lock.actions !== 'object') {
    return [`${LOCK_PATH} must be coven.actions-lock schemaVersion 1.`];
  }
  for (const [name, entry] of Object.entries(lock.actions)) {
    if (!/^[0-9a-f]{40}$/.test(entry?.sha ?? '') || !entry?.ref || !['tag', 'branch'].includes(entry?.refKind)) {
      findings.push(`${LOCK_PATH} entry ${name} needs a 40-character sha, a ref, and refKind tag or branch.`);
    }
  }
  let channel;
  let node;
  try {
    channel = rustChannel(rustToolchain);
  } catch (error) {
    findings.push(error.message);
  }
  try {
    node = nodeVersion(nodeVersionText);
  } catch (error) {
    findings.push(error.message);
  }

  const used = new Set();
  for (const file of files) {
    const text = file.text.replace(/\r\n/g, '\n');
    text.split('\n').forEach((line, index) => {
      const where = `${file.path}:${index + 1}`;
      const match = USES_LINE.exec(line);
      if (!match || line.trimStart().startsWith('#')) {
        return;
      }
      const [, target, comment] = match;
      if (target.startsWith('./')) {
        return;
      }
      if (target.startsWith('docker://')) {
        if (!/@sha256:[0-9a-f]{64}$/.test(target)) {
          findings.push(`${where}: docker action ${target} must be pinned by @sha256 digest.`);
        }
        return;
      }
      const remote = REMOTE_USES.exec(target);
      if (!remote) {
        findings.push(`${where}: cannot parse action reference ${target}.`);
        return;
      }
      const [, name, , ref] = remote;
      const entry = lock.actions[name];
      if (!/^[0-9a-f]{40}$/.test(ref)) {
        findings.push(`${where}: ${name}@${ref} is not pinned to a full commit SHA.`);
        return;
      }
      if (!entry) {
        findings.push(`${where}: ${name} is not recorded in ${LOCK_PATH}.`);
        return;
      }
      used.add(name);
      if (ref !== entry.sha) {
        findings.push(`${where}: ${name}@${ref} differs from the locked ${entry.sha} (${entry.ref}).`);
      }
      if (comment !== entry.ref) {
        findings.push(`${where}: ${name} must carry the comment "# ${entry.ref}", got ${JSON.stringify(comment ?? null)}.`);
      }
    });

    const steps = workflowSteps(text);
    const jobsRunningNode = new Set();
    const jobsInstallingNode = new Set();
    for (const step of steps) {
      const stepText = step.lines.join('\n');
      const where = `${file.path}:${step.start}`;
      const usesMatch = step.lines.map((line) => USES_LINE.exec(line)).find(Boolean);
      const usesName = usesMatch ? REMOTE_USES.exec(usesMatch[1])?.[1] : null;
      if (usesName === RUST_TOOLCHAIN_ACTION) {
        const toolchain = withValue(step, 'toolchain');
        if (channel && toolchain !== channel) {
          findings.push(`${where}: ${RUST_TOOLCHAIN_ACTION} installs ${JSON.stringify(toolchain)}, ${RUST_TOOLCHAIN_PATH} pins ${channel}.`);
        }
      }
      if (usesName === SETUP_NODE_ACTION) {
        jobsInstallingNode.add(step.job);
        if (withValue(step, 'node-version') !== null) {
          findings.push(`${where}: ${SETUP_NODE_ACTION} must read ${NODE_VERSION_PATH} instead of a literal node-version.`);
        } else if (withValue(step, 'node-version-file') !== NODE_VERSION_PATH) {
          findings.push(`${where}: ${SETUP_NODE_ACTION} must set node-version-file: ${NODE_VERSION_PATH}.`);
        }
      }
      if (/(^|[\s;|&(])(node|npm|npx) /m.test(stepText.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n')) &&
        /^\s*(- )?run:/m.test(stepText)) {
        jobsRunningNode.add(step.job);
      }
    }
    for (const job of jobsRunningNode) {
      if (node && !jobsInstallingNode.has(job)) {
        findings.push(`${file.path}: job ${job} runs node without installing ${NODE_VERSION_PATH} through ${SETUP_NODE_ACTION}.`);
      }
    }
  }
  for (const name of Object.keys(lock.actions)) {
    if (!used.has(name)) {
      findings.push(`${LOCK_PATH} records ${name}, which no workflow uses.`);
    }
  }
  return findings;
}

function ghApiJson(endpoint) {
  const result = spawnSync('gh', ['api', endpoint], { encoding: 'utf8' });
  if (result.status !== 0) {
    throw new Error(`gh api ${endpoint} failed: ${String(result.stderr).trim()}`);
  }
  return JSON.parse(result.stdout);
}

/// Confirms each lock entry against GitHub: a tag must still resolve to the
/// locked commit, and a branch must still contain it.
export function verifyRemoteLock(lock, ghApi = ghApiJson) {
  const findings = [];
  for (const [name, entry] of Object.entries(lock.actions)) {
    try {
      if (entry.refKind === 'tag') {
        const ref = ghApi(`/repos/${name}/git/ref/tags/${encodeURIComponent(entry.ref)}`);
        let sha = ref?.object?.sha;
        if (ref?.object?.type === 'tag') {
          sha = ghApi(`/repos/${name}/git/tags/${sha}`)?.object?.sha;
        }
        if (sha !== entry.sha) {
          findings.push(`${name} tag ${entry.ref} now resolves to ${sha}, the lock pins ${entry.sha}.`);
        }
      } else {
        const comparison = ghApi(`/repos/${name}/compare/${entry.sha}...${encodeURIComponent(entry.ref)}`);
        if (!['identical', 'ahead'].includes(comparison?.status)) {
          findings.push(`${name} commit ${entry.sha} is no longer contained in branch ${entry.ref} (${comparison?.status}).`);
        }
      }
    } catch (error) {
      findings.push(`${name}: ${error.message}`);
    }
  }
  return findings;
}

/// The lock the workflows imply: one pin per action repository, named by its
/// `# ref` comment. A ref the lock already tracks as a branch stays a branch;
/// anything new is a tag, which is what version comments name.
export function proposeLock({ lock, files }) {
  const pins = new Map();
  const problems = [];
  for (const file of files) {
    file.text.replace(/\r\n/g, '\n').split('\n').forEach((line, index) => {
      const match = USES_LINE.exec(line);
      if (!match || line.trimStart().startsWith('#')) {
        return;
      }
      const [, target, comment] = match;
      const remote = REMOTE_USES.exec(target);
      if (!remote || target.startsWith('./') || target.startsWith('docker://')) {
        return;
      }
      const [, name, , sha] = remote;
      const where = `${file.path}:${index + 1}`;
      if (!/^[0-9a-f]{40}$/.test(sha) || !comment) {
        problems.push(`${where}: pin ${name} to a full commit SHA with a "# <ref>" comment before updating the lock.`);
        return;
      }
      const pin = pins.get(name);
      if (pin && (pin.sha !== sha || pin.ref !== comment)) {
        problems.push(`${where}: ${name}@${sha} # ${comment} disagrees with ${pin.where} (${pin.sha} # ${pin.ref}); use one pin per action.`);
        return;
      }
      pins.set(name, pin ?? { sha, ref: comment, where });
    });
  }
  const actions = {};
  // Code-point order: locale-independent, and the order the lock already uses.
  for (const name of [...pins.keys()].sort()) {
    const { sha, ref } = pins.get(name);
    const previous = lock.actions?.[name];
    const refKind = previous && previous.ref === ref ? previous.refKind : 'tag';
    actions[name] = { sha, ref, refKind };
  }
  const changed = Object.entries(actions)
    .filter(([name, entry]) => {
      const previous = lock.actions?.[name];
      return !previous || previous.sha !== entry.sha || previous.ref !== entry.ref || previous.refKind !== entry.refKind;
    })
    .map(([name]) => name);
  const removed = Object.keys(lock.actions ?? {}).filter((name) => !(name in actions));
  return { lock: { schemaVersion: 1, kind: 'coven.actions-lock', actions }, changed, removed, problems };
}

/// The lock file's own layout: one entry per line, so a bump is a one-line diff.
export function renderLock(lock) {
  const entries = Object.entries(lock.actions).map(
    ([name, entry]) => `    ${JSON.stringify(name)}: { "sha": ${JSON.stringify(entry.sha)}, "ref": ${JSON.stringify(entry.ref)}, "refKind": ${JSON.stringify(entry.refKind)} }`
  );
  return ['{', '  "schemaVersion": 1,', '  "kind": "coven.actions-lock",', '  "actions": {', entries.join(',\n'), '  }', '}', ''].join('\n');
}

/// Proposes the lock, verifies only what changed against GitHub, and writes
/// nothing unless every changed pin is what its tag or branch says it is.
export function updateLock(inputs, { ghApi = ghApiJson, write = (text) => writeFileSync(path.join(repoRoot, LOCK_PATH), text) } = {}) {
  const proposal = proposeLock(inputs);
  if (proposal.problems.length > 0) {
    return { findings: proposal.problems, proposal };
  }
  const changedLock = {
    actions: Object.fromEntries(proposal.changed.map((name) => [name, proposal.lock.actions[name]]))
  };
  const findings = verifyRemoteLock(changedLock, ghApi);
  if (findings.length > 0) {
    return { findings, proposal };
  }
  const updated = { ...inputs, lock: proposal.lock };
  const remaining = checkActionPins(updated);
  if (remaining.length > 0) {
    return { findings: remaining, proposal };
  }
  if (proposal.changed.length > 0 || proposal.removed.length > 0) {
    write(renderLock(proposal.lock));
  }
  return { findings: [], proposal };
}

if (isMainModule()) {
  const command = process.argv[2] ?? 'check';
  const inputs = readRepositoryInputs();
  if (command === 'update') {
    const { findings, proposal } = updateLock(inputs);
    for (const finding of findings) {
      console.error(finding);
    }
    if (findings.length > 0) {
      console.error(`${LOCK_PATH} was not changed.`);
      process.exit(1);
    }
    for (const name of proposal.changed) {
      const entry = proposal.lock.actions[name];
      console.log(`locked ${name}@${entry.sha} (${entry.refKind} ${entry.ref}), verified against GitHub`);
    }
    for (const name of proposal.removed) {
      console.log(`removed ${name}, which no workflow uses`);
    }
    if (proposal.changed.length === 0 && proposal.removed.length === 0) {
      console.log(`${LOCK_PATH} already matches the workflows.`);
    }
    process.exit(0);
  }
  const findings = command === 'verify-remote'
    ? verifyRemoteLock(inputs.lock)
    : command === 'check'
      ? checkActionPins(inputs)
      : [`Unknown command ${JSON.stringify(command)}; use check, verify-remote, or update.`];
  for (const finding of findings) {
    console.error(finding);
  }
  if (findings.length > 0) {
    process.exit(1);
  }
  console.log(`Action pins ${command === 'verify-remote' ? 'match GitHub' : 'are locked'} (${Object.keys(inputs.lock.actions).length} actions, ${inputs.files.length} workflow files).`);
}
