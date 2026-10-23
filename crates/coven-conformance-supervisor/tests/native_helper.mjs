// Independent native fixture driver, not release eligibility or runner support.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, existsSync, rmSync, mkdirSync, realpathSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
assert.equal(process.versions.node.split('.')[0], '24', 'native contract requires Node24');
const [helper, subject, cwd] = process.argv.slice(2);
const abi = 'coven.automations.windows-job-supervisor.v1';
const digest = createHash('sha256').update(readFileSync(subject)).digest('hex');
const marker = join(cwd, 'entered.marker');
// Written and closed before the marker is created; read only once the marker exists.
const enteredPid = join(cwd, 'entered.pid');
const body = Buffer.from([0, 255, 13, 10, 128, 42]);
// Failure context only: the fixture subject's own libtest panic site and the
// first line of its message, with any absolute path replaced. Never the
// subject payload, which this fixture does not echo on its panic path.
function subjectPanic(...buffers) {
  for (const buffer of buffers) {
    const text = buffer.toString('latin1');
    const match = /panicked at [^\r\n]*?([A-Za-z0-9_]+\.rs:\d+:\d+):\r?\n([^\r\n]{0,240})/.exec(text);
    if (match) return `${match[1]}: ${match[2].replace(/[A-Za-z]:\\[^\s'"]*/g, '<path>').replace(/\/[^\s'"]+/g, '<path>')}`;
  }
  return 'no subject panic site in captured output';
}
function checkedPid(value) {
  const pid = Number(value);
  assert.ok(Number.isSafeInteger(pid) && pid > 0, 'invalid fixture process identity');
  return pid;
}
async function run(mode, options = {}) {
  rmSync(marker, { force: true }); rmSync(enteredPid, { force: true });
  rmSync(join(cwd, 'descendant.pid'), { force: true });
  writeFileSync(join(cwd, 'mode'), mode);
  const nonce = 'ab'.repeat(16);
  const target = options.badImage ? join(cwd, 'invalid-image.exe') : subject;
  if (options.badImage) writeFileSync(target, 'not a PE image');
  const child = spawn(helper, [target, options.badCwd ? join(cwd, 'missing-directory') : cwd], { cwd, stdio: ['overlapped', 'overlapped', 'overlapped', 'overlapped'], windowsHide: true });
  const inputBody = options.blockInput ? Buffer.alloc(4 * 1024 * 1024, 0x41) : body;
  const frames = []; let pending = Buffer.alloc(0); let output = Buffer.alloc(0); let error = Buffer.alloc(0); let ack = false;
  let failure;
  const fail = (error) => { failure ??= error; child.kill(); };
  const guard = setTimeout(() => fail(new Error('native fixture hang guard')), 90000);
  child.on('error', fail);
  child.on('exit', () => { if (options.blockOutput) child.stdout.resume(); });
  child.stdin.on('error', () => {}); child.stdio[3].on('error', () => {});
  if (options.blockOutput) child.stdout.pause();
  child.stdout.on('data', chunk => {
    if (output.length + chunk.length > 1024 * 1024) return fail(new Error('stdout exceeded cap'));
    output = Buffer.concat([output, chunk]);
  });
  child.stderr.on('data', chunk => {
    if (error.length + chunk.length > 1024 * 1024) return fail(new Error('stderr exceeded cap'));
    error = Buffer.concat([error, chunk]);
  });
  child.stdio[3].on('data', chunk => {
    try {
      pending = Buffer.concat([pending, chunk]);
      for (;;) {
        const end = pending.indexOf(10);
        if (end < 0) { assert.ok(pending.length < 256); break; }
        assert.ok(end + 1 <= 256);
        const frame = JSON.parse(pending.subarray(0, end).toString('utf8'));
        pending = pending.subarray(end + 1);
        assert.equal(frame.schemaVersion, abi); assert.equal(frame.nonce, nonce);
        assert.ok(frames.length < 2);
        if (frame.kind === 'ready') {
          assert.equal(frames.length, 0); assert.deepEqual(Object.keys(frame).sort(), ['kind', 'nonce', 'schemaVersion']);
          assert.equal(existsSync(marker), false, 'subject entered before ACK');
          if (!options.noAck) child.stdio[3].write(`GO ${options.badAck ? 'ef'.repeat(16) : nonce}\n`, error => { if (error) return fail(error); ack = true; });
        } else if (frame.kind === 'exited') {
          assert.equal(frames[0]?.kind, 'ready');
          assert.deepEqual(Object.keys(frame).sort(), ['exitCode', 'kind', 'nonce', 'schemaVersion']);
        } else {
          assert.equal(frame.kind, 'refused');
          assert.deepEqual(Object.keys(frame).sort(), ['failure', 'kind', 'nonce', 'schemaVersion']);
        }
        frames.push(frame);
      }
    } catch (error) { fail(error); }
  });
  if (options.blockOutput) child.stdout.pause();
  let injected = false;
  const injectLoss = options.leaseLoss ? (async () => {
    while (!existsSync(marker) || !ack || (options.blockInput && !child.stdin.writableNeedDrain) || (options.blockOutput && child.stdout.readableLength < child.stdout.readableHighWaterMark)) {
      if (child.exitCode !== null || failure) return;
      await delay(10);
    }
    injected = true;
    child.stdio[3].write('x'); // Unexpected post-ACK data is controller loss.
  })() : Promise.resolve();
  const header = { schemaVersion: abi, kind: 'launch', nonce, subjectSha256: options.badDigest ? '0'.repeat(64) : createHash('sha256').update(readFileSync(target)).digest('hex'),
    args: ['--exact', 'subject', '--ignored', '--nocapture', '--test-threads=1'], inputBytes: inputBody.length, timeoutMs: 30000 };
  if (options.badHeader) header.schemaVersion = 'unsupported';
  // Control handlers are live before input writes; ACK never waits on body drain.
  if (options.headerStall) child.stdin.write('{');
  else child.stdin.end(Buffer.concat([Buffer.from(JSON.stringify(header) + '\n'), options.truncated ? inputBody.subarray(0, inputBody.length - 1) : inputBody, options.surplus ? Buffer.from([9]) : Buffer.alloc(0)]));
  const code = await new Promise(resolve => child.on('close', resolve));
  await injectLoss;
  clearTimeout(guard); if (failure) throw failure;
  try {
    assert.equal(pending.length, 0, 'partial terminal frame');
    if (options.badHeader || options.headerStall) {
      assert.equal(code, 2); assert.deepEqual(frames, []); assert.equal(existsSync(marker), false);
    } else if (options.badDigest || options.badImage || options.badCwd) {
      assert.equal(code, 2); assert.deepEqual(frames.map(frame => frame.kind), ['refused']);
      assert.equal(frames[0].failure, options.badCwd ? 'containment_unavailable' : 'launch_invalid'); assert.equal(existsSync(marker), false);
    } else if (options.badAck) {
      assert.equal(code, 2); assert.deepEqual(frames.map(frame => frame.kind), ['ready', 'refused']);
      assert.equal(frames[1].failure, 'controller_lost'); assert.equal(existsSync(marker), false);
    } else if (options.noAck || options.deadline) {
      assert.equal(code, 2);
      // The genuine total deadline also covers suspended preparation. Heavy host
      // load may refuse before ready; report that phase instead of fabricating
      // ACK-stall coverage. Success cases separately prove pre-ACK entry gating.
      assert.ok(frames.length === 1 || frames.length === 2);
      assert.deepEqual(frames.map(frame => frame.kind), frames.length === 2 ? ['ready', 'refused'] : ['refused']);
      assert.equal(frames.at(-1).failure, 'deadline_exceeded');
      if (options.noAck) assert.equal(existsSync(marker), false);
      console.log(JSON.stringify({ case: options.noAck ? 'no-ack-deadline' : 'running-deadline', readyObserved: frames.length === 2, targetEntered: existsSync(marker) }));
    } else if (options.leaseLoss) {
      assert.equal(injected, true, 'failure was not injected at the observed phase');
      assert.equal(code, 2); assert.deepEqual(frames.map(frame => frame.kind), ['ready', 'refused']);
      assert.equal(frames[1].failure, 'controller_lost');
      const pid = checkedPid(readFileSync(enteredPid, 'utf8'));
      assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' }, 'subject survived controller loss');
    } else if (options.surplus || options.truncated || mode === 'overflow' || mode === 'stderr-overflow') {
      assert.equal(code, 2); assert.deepEqual(frames.map(frame => frame.kind), ['ready', 'refused']);
      assert.equal(frames[1].failure, options.surplus || options.truncated ? 'launch_invalid' : 'output_limit_exceeded');
    } else {
      assert.equal(code, 0); assert.equal(ack, true);
      assert.deepEqual(frames.map(frame => frame.kind), ['ready', 'exited']);
      assert.equal(frames[1].exitCode, mode === 'nonzero' ? 23 : 0);
      if (mode === 'descendants') {
        const pid = checkedPid(readFileSync(join(cwd, 'descendant.pid'), 'utf8'));
        assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' }, 'descendant survived root exit cleanup');
      } else {
        assert.ok(output.includes(body), 'binary output lost'); assert.deepEqual(error, Buffer.from([0, 255, 7]));
      }
      assert.equal(existsSync(marker), true);
    }
  } catch (cause) {
    throw new Error(`native case ${mode} ${JSON.stringify(Object.keys(options).sort())}: helperExit=${code} ` +
      `frames=${JSON.stringify(frames)} subject=${subjectPanic(output, error)}`, { cause });
  }
  console.log(JSON.stringify({ case: mode, options: Object.keys(options).sort(), helperExit: code, frames: frames.map(frame => frame.kind) }));
}
async function controller() {
  rmSync(marker, { force: true }); rmSync(enteredPid, { force: true }); writeFileSync(join(cwd, 'mode'), 'hold');
  const nonce = 'cd'.repeat(16);
  const child = spawn(helper, [subject, cwd], { cwd, stdio: ['overlapped', 'overlapped', 'overlapped', 'overlapped'], windowsHide: true });
  process.send({ kind: 'spawned', helperPid: child.pid });
  child.stdout.resume(); child.stderr.resume(); child.stdin.on('error', () => {});
  child.stdio[3].on('error', () => {});
  let pending = '';
  child.stdio[3].on('data', chunk => {
    pending += chunk.toString('utf8');
    const end = pending.indexOf('\n');
    if (end >= 0 && JSON.parse(pending.slice(0, end)).kind === 'ready') {
      child.stdio[3].write(`GO ${nonce}\n`); pending = pending.slice(end + 1);
    }
  });
  child.stdin.end(JSON.stringify({ schemaVersion: abi, kind: 'launch', nonce, subjectSha256: digest,
    args: ['--exact', 'subject', '--ignored', '--nocapture'], inputBytes: 0, timeoutMs: 60000 }) + '\n');
  while (!existsSync(marker)) { assert.equal(child.exitCode, null); await delay(10); }
  process.send({ kind: 'entered', subjectPid: checkedPid(readFileSync(enteredPid, 'utf8')) });
  // Outer driver terminates this actual controller process while the target runs.
  await new Promise(() => {});
}
function alive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === 'ESRCH') return false; throw error; }
}
async function controllerDeath() {
  const child = spawn(process.execPath, [fileURLToPath(import.meta.url), helper, subject, cwd, 'controller'], {
    cwd, stdio: ['ignore', 'ignore', 'pipe', 'ipc'], windowsHide: true,
  });
  let helperPid; let subjectPid; let diagnostic = '';
  child.stderr.on('data', chunk => { diagnostic = (diagnostic + chunk).slice(-4096); });
  const closed = new Promise(resolve => child.once('close', resolve));
  const guard = setTimeout(() => child.kill(), 90000);
  try {
    await new Promise((resolve, reject) => {
      child.on('message', message => {
        if (message.kind === 'spawned') helperPid = checkedPid(message.helperPid);
        if (message.kind === 'entered') { subjectPid = checkedPid(message.subjectPid); resolve(); }
      });
      child.once('error', reject);
      child.once('exit', () => reject(new Error('controller exited before target readiness: ' + diagnostic)));
    });
    assert.ok(alive(helperPid)); assert.ok(alive(subjectPid));
    child.kill(); await closed;
    const started = Date.now();
    while (alive(helperPid) || alive(subjectPid)) {
      assert.ok(Date.now() - started < 60000, 'controller-death containment hang guard');
      await delay(10);
    }
    console.log(JSON.stringify({ case: 'actual-controller-death', helperExited: true, subjectExited: true }));
  } finally {
    clearTimeout(guard); child.kill();
    // Fixture-only backstop on assertion failure; this never counts as proof.
    for (const pid of [helperPid, subjectPid]) if (pid && alive(pid)) process.kill(pid);
    await closed;
  }
}
if (process.argv[5] === 'controller') {
  await controller();
} else {
await run('success'); await run('nonzero'); await run('success', { badHeader: true }); await run('success', { badDigest: true });
await run('success', { badImage: true }); await run('success', { badCwd: true });
await run('success', { headerStall: true }); await run('success', { truncated: true });
await run('descendants');
await run('hold', { blockInput: true, leaseLoss: true });
await run('overflow', { blockOutput: true, leaseLoss: true });
await run('success', { badAck: true });
await run('hold', { deadline: true });
await run('stderr-overflow');
await run('success', { noAck: true }); await run('success', { surplus: true }); await run('overflow');
await controllerDeath();
const runnerPath = process.argv[6];
const { invokeWindowsHelper, createWindowsRunnerArtifact, validateWindowsRunnerBinding, digestBytes, digestCanonical } = await import(pathToFileURL(runnerPath).href);
for (const mode of ['consumer', 'success']) {
  rmSync(marker, { force: true }); rmSync(enteredPid, { force: true }); writeFileSync(join(cwd, 'mode'), mode);
  const result = await invokeWindowsHelper({ helper, target: subject, directory: cwd, subjectSha256: digest,
    args: ['--exact', 'subject', '--ignored', '--nocapture', '--test-threads=1'], input: body, timeoutMs: 30000 });
  if (mode === 'consumer') {
    assert.equal(result.error, undefined); assert.equal(result.status, 0);
    // libtest writes its fixed preamble before the child entrypoint.
    assert.ok(result.stdout.endsWith('{"inputBytes":6}'));
  } else {
    assert.ok(result.error); assert.equal(result.status, null); assert.equal(result.stdout, '');
  }
  console.log(JSON.stringify({ case: 'production-consumer', mode, accepted: result.error === undefined }));
}
// Exercise real locked Cargo helper production from a private pinned Git clone.
// The parent workspace's active Cargo target must never be borrowed recursively.
const { buildWindowsHelper } = await import(pathToFileURL(join(dirname(runnerPath), 'audit.mjs')).href);
const repo = resolve(dirname(runnerPath), '../../..');
const gitEnv = { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_'))),
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_SYSTEM: '/dev/null', GIT_TERMINAL_PROMPT: '0' };
function git(args, directory = repo) {
  const result = spawnSync('git', ['-c', 'core.fsmonitor=false', ...args], { cwd: directory, env: gitEnv, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 });
  assert.equal(result.status, 0, 'native helper producer Git operation failed'); return result.stdout.trim();
}
const sourceCommit = git(['rev-parse', 'HEAD']);
const clone = join(cwd, 'producer-source');
git(['clone', '--local', '--no-hardlinks', '--no-checkout', '--quiet', '--config', 'core.hooksPath=/dev/null', '--config', 'core.autocrlf=false', '--', repo, clone]);
git(['checkout', '--quiet', '--detach', sourceCommit], clone);
const sourceRoot = realpathSync(clone);
assert.equal(git(['rev-parse', 'HEAD'], sourceRoot), sourceCommit);
assert.equal(git(['status', '--porcelain=v1', '--untracked-files=all', '--ignored=matching'], sourceRoot), '');
const producerOutput = join(cwd, 'producer-output'); mkdirSync(producerOutput);
const produced = buildWindowsHelper(sourceRoot, join(cwd, 'producer-target'), producerOutput, sourceCommit);
const runnerBytes = readFileSync(join(sourceRoot, 'conformance/automations/runner/conformance.mjs'));
assert.equal(digestBytes(runnerBytes), digestBytes(readFileSync(runnerPath)));
const helperBytes = readFileSync(produced.helperPath); const subjectBytes = readFileSync(subject);
const runnerArtifact = createWindowsRunnerArtifact(runnerBytes, helperBytes, sourceCommit);
const binding = { source: { commit: sourceCommit }, environment: { os: process.platform, arch: process.arch }, suites: [],
  runner: { name: 'coven-automations-conformance-runner', version: runnerArtifact.version, artifactSha256: runnerArtifact.sha256, vectorSetSha256: digestCanonical([]).value },
  subjectArtifact: { platform: { os: process.platform, arch: process.arch }, sha256: digest } };
assert.deepEqual(validateWindowsRunnerBinding(binding, { runnerBytes, helperBytes, subjectBytes }), runnerArtifact);
rmSync(marker, { force: true }); rmSync(enteredPid, { force: true }); writeFileSync(join(cwd, 'mode'), 'consumer');
const producedResult = await invokeWindowsHelper({ helper: produced.helperPath, target: subject, directory: cwd, subjectSha256: digest,
  args: ['--exact', 'subject', '--ignored', '--nocapture', '--test-threads=1'], input: body, timeoutMs: 30000 });
assert.equal(producedResult.error, undefined); assert.equal(producedResult.status, 0); assert.ok(producedResult.stdout.endsWith('{"inputBytes":6}'));
assert.equal(digestBytes(readFileSync(produced.helperPath)), produced.sha256);
assert.equal(git(['status', '--porcelain=v1', '--untracked-files=all', '--ignored=matching'], sourceRoot), '');
console.log(JSON.stringify({ case: 'native-pinned-helper-producer', sourceCommit, sourceTree: git(['rev-parse', 'HEAD^{tree}'], sourceRoot),
  cargoLockSha256: digestBytes(readFileSync(join(sourceRoot, 'Cargo.lock'))), helperSha256: produced.sha256, runnerArtifactSha256: runnerArtifact.sha256,
  scope: 'native_helper_build_binding_invocation_not_full_audit' }));
console.log(JSON.stringify({ schemaVersion: 'coven.automations.windows-supervisor-fixture.v1', node: process.versions.node, os: process.platform, arch: process.arch, helperSha256: createHash('sha256').update(readFileSync(helper)).digest('hex'), subjectSha256: digest }));
console.log('native-helper-matrix-passed');
}
