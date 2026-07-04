import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

function readRepoFile(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
}

test('public PR surfaces are open and no longer pre-July gate copy', () => {
  const paths = [
    '.github/pull_request_template.md',
    '.github/ISSUE_TEMPLATE/config.yml',
    'CONTRIBUTING.md',
    'README.md'
  ];

  for (const path of paths) {
    const text = readRepoFile(path);
    assert.doesNotMatch(text, /PRs? (?:are )?not (?:being )?accepted until July 2026/i, `${path} must not reject current PRs`);
    assert.doesNotMatch(text, /Please close this PR and open an Issue instead/i, `${path} must not instruct contributors to close PRs`);
    assert.doesNotMatch(text, /Until then,\s+we are accepting Issues and Bug Reports only/i, `${path} must not describe the pre-event issue-only gate`);
  }
});

test('pull request template requests a complete agent-ready readiness packet', () => {
  const template = readRepoFile('.github/pull_request_template.md');

  assert.match(template, /## Context/i);
  assert.match(template, /## Implementation/i);
  assert.match(template, /## Verification/i);
  assert.match(template, /## Risk and Rollback/i);
  assert.match(template, /## Agent Handoff/i);
  assert.match(template, /Closes #/);
  assert.match(template, /cargo fmt --check/);
  assert.match(template, /cargo clippy --workspace --all-targets -- -D warnings/);
  assert.match(template, /cargo test --workspace --locked/);
  assert.match(template, /python(?:3)? scripts\/check-secrets\.py/);
});

test('pull request template has no blank scaffold bullets', () => {
  const template = readRepoFile('.github/pull_request_template.md');

  assert.doesNotMatch(template, /^\s*-\s*$/m);
  assert.match(template, /- Summary:/);
  assert.match(template, /- Files changed:/);
});

test('pr-agent skill is Coven-native and encodes PR readiness gates', () => {
  const skill = readRepoFile('skills/pr-agent/SKILL.md');
  const readme = readRepoFile('skills/pr-agent/README.md');
  const agentConfig = readRepoFile('skills/pr-agent/agents/openai.yaml');
  const combined = `${skill}\n${readme}\n${agentConfig}`;

  assert.doesNotMatch(combined, /OpenClaw PR Agent/i);
  assert.doesNotMatch(combined, /\.agents\/skills\/PR_WORKFLOW\.md/);
  assert.doesNotMatch(combined, /scripts\/pr-(review|prepare|merge)/);

  assert.match(skill, /Coven PR Readiness Agent/);
  assert.match(skill, /Readiness Packet/);
  assert.match(skill, /Context Bundle/);
  assert.match(skill, /Template Assembly/);
  assert.match(skill, /Verification Matrix/);
  assert.match(skill, /Do not create or update a PR until/);
  assert.match(skill, /cargo fmt --check/);
  assert.match(skill, /cargo clippy --workspace --all-targets -- -D warnings/);
  assert.match(skill, /cargo test --workspace --locked/);
  assert.match(skill, /python(?:3)? scripts\/check-secrets\.py/);

  assert.match(readme, /Coven PR Readiness Agent/);
  assert.match(readme, /at-scale PR creation/i);
  assert.match(agentConfig, /Coven PR readiness/i);
  assert.match(agentConfig, /readiness packet/i);
});

test('openclaw skills manifest marks pr-agent as Coven-owned canonical context', () => {
  const manifest = JSON.parse(readRepoFile('skills/openclaw-skills-manifest.json'));
  const prAgent = manifest.skills.find((skill) => skill.name === 'pr-agent');

  assert.ok(prAgent, 'manifest must include pr-agent');
  assert.equal(prAgent.migrationStatus, 'represented-by-existing-coven-skill');
  assert.equal(prAgent.managedBySync, false);
  assert.match(prAgent.description, /Coven PR Readiness Agent/);
  assert.doesNotMatch(prAgent.description, /OpenClaw PR Agent/i);
});

test('npm prepublish smoke runs the PR readiness guardrail', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');

  assert.match(script, /scripts\/pr-readiness-test\.mjs/);
  assert.match(script, /onboarding, PR readiness, and publish guardrails/);
});

test('OpenClaw PR workflow skills explicitly redirect Coven PR creation to pr-agent', () => {
  const codeflow = readRepoFile('skills/codeflow-maintainer/SKILL.md');
  const openclawDev = readRepoFile('skills/openclaw-dev/SKILL.md');
  const manifest = JSON.parse(readRepoFile('skills/openclaw-skills-manifest.json'));

  for (const [name, text] of [
    ['codeflow-maintainer', codeflow],
    ['openclaw-dev', openclawDev]
  ]) {
    assert.match(text, /OpenClaw-only/i, `${name} must declare its OpenClaw-only boundary`);
    assert.match(text, /OpenCoven\/coven/i, `${name} must name the Coven repo boundary`);
    assert.match(text, /skills\/pr-agent/i, `${name} must route Coven PR creation to skills/pr-agent`);
    assert.match(text, /Coven PR Readiness Agent/i, `${name} must name the Coven readiness skill`);
  }

  for (const name of ['codeflow-maintainer', 'openclaw-dev']) {
    const entry = manifest.skills.find((skill) => skill.name === name);
    assert.ok(entry, `manifest must include ${name}`);
    assert.equal(entry.migrationStatus, 'represented-by-existing-coven-skill');
    assert.equal(entry.managedBySync, false);
    assert.match(entry.description, /OpenClaw-only/i);
    assert.match(entry.description, /Coven PR Readiness Agent/i);
  }
});
