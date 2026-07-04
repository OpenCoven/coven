import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';

const criticalDocs = [
  'docs/help/harness-not-found.md',
  'docs/help/daemon-wont-start.md',
  'docs/help/diagnostics-bundle.md',
  'docs/help/paths.md',
  'docs/help/permissions.md',
  'docs/help/session-stuck.md',
  'docs/help/filing-issues.md',
  'docs/help/community.md',
  'docs/reference/cli-doctor.md',
  'docs/reference/cli-daemon.md',
  'docs/daemon/coven-home.md',
  'docs/daemon/health.md'
];

function readRepoFile(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
}

test('critical onboarding support docs are concrete, not stubs', () => {
  for (const path of criticalDocs) {
    const text = readRepoFile(path);
    assert.doesNotMatch(text, /^Stub -- fill in\.?$/m, `${path} must not be a stub`);
    assert.doesNotMatch(text, /^Stub . fill in\.?$/m, `${path} must not be a stub`);
    assert.match(text, /coven [a-z-]+/i, `${path} must include at least one concrete coven command`);
    assert.ok(text.length > 1200, `${path} must include actionable setup detail`);
  }
});

test('install docs link COVEN_HOME users to the daemon state guide', () => {
  const installDir = new URL('../docs/install/', import.meta.url);
  const installDocs = readdirSync(installDir)
    .filter((name) => name.endsWith('.md'))
    .map((name) => `docs/install/${name}`);

  for (const path of installDocs) {
    const text = readRepoFile(path);
    assert.doesNotMatch(text, /\/install\/coven-home\b/, `${path} must not link to missing install route`);
  }

  assert.match(readRepoFile('docs/install/index.md'), /\/daemon\/coven-home\b/);
});

test('ci runs npm onboarding smoke on every published platform', () => {
  const workflow = readRepoFile('.github/workflows/ci.yml');
  assert.match(workflow, /name:\s+npm onboarding smoke/i);
  assert.match(workflow, /ubuntu-latest/);
  assert.match(workflow, /macos-26/);
  assert.doesNotMatch(workflow, /os:\s+macos-latest/);
  assert.match(workflow, /windows-latest/);
  assert.match(workflow, /node scripts\/test-cli-prepublish\.mjs --target=\$\{\{ matrix\.npm-target \}\} --skip-build --skip-secrets-scan/);
  assert.match(workflow, /COVEN_NPM_DRY_RUN_VERSION:\s+999\.0\.0\s*(?:\n|$)/);
});

test('npm onboarding smoke exercises daemon lifecycle with isolated state', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /COVEN_HOME:\s*path\.join\(tempDir,\s*'coven-home'\)/);
  assert.match(script, /\['daemon',\s*'start'\]/);
  assert.match(script, /\['daemon',\s*'status'\]/);
  assert.match(script, /\['daemon',\s*'stop'\]/);
  assert.match(script, /\['sessions',\s*'--plain'\]/);
  assert.match(script, /status=running/);
  assert.match(script, /ok=true/);
});

test('npm onboarding smoke launches Windows cmd shims through a shell', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /function spawnOptionsForCommand\(/);
  assert.match(script, /shell:\s*platform === 'win32'/);
  assert.match(script, /spawnOptionsForCommand\(options/);
  assert.match(script, /spawnSync\(command,\s*args,\s*\{\s*\.\.\.spawnOptionsForCommand\(\)/);
  assert.match(script, /spawnSync\('npm',\s*\['view'[\s\S]*?\.\.\.spawnOptionsForCommand\(\)/);
  assert.match(script, /spawnSync\('npm',\s*\['pack'[\s\S]*?\.\.\.spawnOptionsForCommand\(\)/);
});

test('npm onboarding smoke verifies first-run missing-harness guidance deterministically', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /function firstRunSmokePath\(/);
  assert.match(script, /PATH:\s*firstRunSmokePath\(wrapperBin\)/);
  assert.match(script, /Install and authenticate at least one harness in this same shell/);
  assert.match(script, /npm install -g @openai\/codex && codex login/);
  assert.match(script, /npm install -g @anthropic-ai\/claude-code && claude doctor/);
});

test('npm onboarding smoke runs onboarding guardrails before packaging', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /scripts\/onboarding-docs-test\.mjs/);
  assert.match(script, /scripts\/publish-npm-test\.mjs/);
});

test('npm onboarding smoke avoids deprecated optional dependency install flag', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /'--omit=optional'/);
  assert.doesNotMatch(script, /'--no-optional'/);
});

test('npm onboarding smoke bounds subprocess hangs', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /DEFAULT_COMMAND_TIMEOUT_MS/);
  assert.match(script, /timeout:\s*options\.timeoutMs \?\? DEFAULT_COMMAND_TIMEOUT_MS/);
  assert.match(script, /ETIMEDOUT/);
});

test('npm onboarding smoke does not pipe-capture Windows daemon start', () => {
  const script = readRepoFile('scripts/test-cli-prepublish.mjs');
  assert.match(script, /function runDaemonStart\(/);
  assert.match(script, /process\.platform === 'win32'/);
  assert.match(script, /run\(wrapperBin, \['daemon', 'start'\]/);
  assert.match(script, /runCapture\(wrapperBin, \['daemon', 'status'\]/);
});
