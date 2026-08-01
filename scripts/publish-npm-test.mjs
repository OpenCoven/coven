import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { defaultTargetName, isMainModule, isOidcContext, packageVersionPublished, publishArgs, publishEnv, releaseVersion, targetPackageName, validatePublishToken, validatePublishVersion, wrapperPackageDirName, wrapperPackageNameList, wrapperTextForPackage } from './publish-npm.mjs';
import { parseReleaseTag } from './release-npm-context.mjs';

const OIDC_ENV = {
  ACTIONS_ID_TOKEN_REQUEST_TOKEN: 'fake-oidc-token',
  ACTIONS_ID_TOKEN_REQUEST_URL: 'https://token.actions.githubusercontent.com/'
};

test('parseReleaseTag preserves stable releases', () => {
  assert.deepEqual(parseReleaseTag('v0.2.3'), {
    releaseMode: 'normal',
    releaseTag: 'v0.2.3',
    npmVersion: '0.2.3',
    recoveryAttempt: null
  });
});

test('parseReleaseTag derives the base version from signed recovery tags', () => {
  assert.deepEqual(parseReleaseTag('v0.2.3-recovery.1'), {
    releaseMode: 'recovery',
    releaseTag: 'v0.2.3',
    npmVersion: '0.2.3',
    recoveryAttempt: 1
  });
});

test('parseReleaseTag rejects malformed and unrelated prerelease tags', () => {
  for (const tag of [
    'v0.2',
    'v0.2.3-rc.1',
    'v0.2.3-recovery.0',
    'v01.2.3',
    'recovery-v0.2.3'
  ]) {
    assert.throws(
      () => parseReleaseTag(tag),
      /stable vX.Y.Z tag or vX.Y.Z-recovery.N/
    );
  }
});

const SIGNAL_TEST_PACKAGES = {
  'darwin-arm64': ['@opencoven/cli-macos', 'coven'],
  'linux-x64': ['@opencoven/cli-linux-x64', 'coven']
};

async function assertWrapperPreservesSignal(signal) {
  const fixture = mkdtempSync(path.join(tmpdir(), 'coven-wrapper-signal-'));
  let wrapperProcess;
  try {
    const wrapperDir = path.join(fixture, 'wrapper');
    const wrapperBinDir = path.join(wrapperDir, 'bin');
    const wrapperPath = path.join(wrapperBinDir, 'coven.js');
    mkdirSync(wrapperBinDir, { recursive: true });
    writeFileSync(
      path.join(wrapperDir, 'package.json'),
      JSON.stringify({ name: '@opencoven/cli-test', type: 'module' })
    );
    copyFileSync(
      fileURLToPath(new URL('../npm/coven/bin/coven.js', import.meta.url)),
      wrapperPath
    );

    const [packageName, binaryName] =
      SIGNAL_TEST_PACKAGES[`${process.platform}-${process.arch}`];
    const nativeDir = path.join(wrapperDir, 'node_modules', ...packageName.split('/'));
    const nativeBinDir = path.join(nativeDir, 'bin');
    mkdirSync(nativeBinDir, { recursive: true });
    writeFileSync(
      path.join(nativeDir, 'package.json'),
      JSON.stringify({ name: packageName, version: '0.0.0' })
    );
    symlinkSync(process.execPath, path.join(nativeBinDir, binaryName));

    let stdout = '';
    let stderr = '';
    wrapperProcess = spawn(
      process.execPath,
      [
        wrapperPath,
        '-e',
        'process.stdout.write("rea"); setTimeout(() => process.stdout.write("dy\\n"), 10); setTimeout(() => process.exit(2), 20_000);'
      ],
      { stdio: ['ignore', 'pipe', 'pipe'] }
    );
    wrapperProcess.stderr.setEncoding('utf8');
    wrapperProcess.stderr.on('data', (chunk) => {
      stderr += chunk;
    });

    await new Promise((resolve, reject) => {
      const timeout = setTimeout(
        () =>
          reject(
            new Error(
              `timed out waiting for fake native child; stdout:\n${stdout}\nstderr:\n${stderr}`
            )
          ),
        10_000
      );
      wrapperProcess.stdout.setEncoding('utf8');
      wrapperProcess.stdout.on('data', (chunk) => {
        stdout += chunk;
        if (stdout.includes('ready')) {
          clearTimeout(timeout);
          resolve();
        }
      });
      wrapperProcess.once('error', (error) => {
        clearTimeout(timeout);
        reject(error);
      });
      wrapperProcess.once('exit', (code, exitSignal) => {
        clearTimeout(timeout);
        reject(
          new Error(
            `wrapper exited before readiness: code=${code} signal=${exitSignal}; stdout:\n${stdout}\nstderr:\n${stderr}`
          )
        );
      });
    });

    const exit = once(wrapperProcess, 'exit');
    assert.equal(wrapperProcess.kill(signal), true);
    const [code, exitSignal] = await exit;
    assert.equal(code, null, `wrapper should not convert ${signal} into exit code ${code}`);
    assert.equal(exitSignal, signal, `wrapper should terminate with ${signal}`);
  } finally {
    if (
      wrapperProcess &&
      wrapperProcess.exitCode === null &&
      wrapperProcess.signalCode === null
    ) {
      wrapperProcess.kill('SIGKILL');
    }
    rmSync(fixture, { recursive: true, force: true });
  }
}

test(
  'npm wrapper preserves child signal termination',
  {
    skip:
      process.platform === 'win32' ||
      !SIGNAL_TEST_PACKAGES[`${process.platform}-${process.arch}`],
    timeout: 30_000
  },
  async () => {
    for (const signal of ['SIGINT', 'SIGTERM']) {
      await assertWrapperPreservesSignal(signal);
    }
  }
);

test('releaseVersion prefers explicit COVEN_NPM_VERSION and strips a leading v', () => {
  assert.equal(
    releaseVersion({ COVEN_NPM_VERSION: 'v1.2.3', GITHUB_REF_NAME: 'v9.9.9' }, '0.0.0'),
    '1.2.3'
  );
});

test('releaseVersion falls back to tag ref for tag-triggered dry runs', () => {
  assert.equal(releaseVersion({ GITHUB_REF_NAME: 'v2.0.1' }, '0.0.0'), '2.0.1');
});

test('releaseVersion falls back to package placeholder for local dry runs', () => {
  assert.equal(releaseVersion({}, '0.0.0'), '0.0.0');
});

test('validatePublishVersion rejects real publish with placeholder version', () => {
  assert.throws(() => validatePublishVersion('0.0.0', false), /Refusing real npm publish/);
});

test('validatePublishVersion allows dry-run with placeholder version', () => {
  assert.doesNotThrow(() => validatePublishVersion('0.0.0', true));
});

test('validatePublishVersion allows real publish with explicit release version', () => {
  assert.doesNotThrow(() => validatePublishVersion('1.2.3', false));
});

test('macOS target publishes under human-facing native package name', () => {
  assert.equal(targetPackageName('macos'), '@opencoven/cli-macos');
});

test('linux x64 target publishes under linux native package name', () => {
  assert.equal(targetPackageName('linux-x64'), '@opencoven/cli-linux-x64');
});

test('windows target publishes under windows native package name', () => {
  assert.equal(targetPackageName('windows'), '@opencoven/cli-windows');
});

test('defaultTargetName maps win32 x64 to windows', () => {
  assert.equal(defaultTargetName('win32', 'x64'), 'windows');
});

test('wrapper declares linux x64 native package as an optional dependency', () => {
  const packagePath = new URL(['..', 'npm', 'coven', 'package.json'].join('/'), import.meta.url);
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  assert.equal(packageJson.optionalDependencies['@opencoven/cli-linux-x64'], '0.0.0');
});

test('wrapper keeps the dashboard companion on its independent version', () => {
  const packagePath = new URL(['..', 'npm', 'coven', 'package.json'].join('/'), import.meta.url);
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  assert.equal(
    packageJson.optionalDependencies['@opencoven/coven-memory-dashboard'],
    '^0.1.0'
  );
});

test('wrapper declares the installed dashboard entrypoint handoff', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /COVEN_MEMORY_DASHBOARD_ENTRY/);
  assert.match(bin, /COVEN_MEMORY_DASHBOARD_NODE/);
});

test(
  'wrapper passes the dashboard handoff for every valid memory open option placement only',
  {
    skip:
      process.platform === 'win32' ||
      !SIGNAL_TEST_PACKAGES[`${process.platform}-${process.arch}`]
  },
  () => {
    const fixture = mkdtempSync(path.join(tmpdir(), 'coven-wrapper-dashboard-'));
    try {
      const wrapperDir = path.join(fixture, 'wrapper');
      const wrapperBinDir = path.join(wrapperDir, 'bin');
      const wrapperPath = path.join(wrapperBinDir, 'coven.js');
      mkdirSync(wrapperBinDir, { recursive: true });
      writeFileSync(
        path.join(wrapperDir, 'package.json'),
        JSON.stringify({ name: '@opencoven/cli-test', type: 'module' })
      );
      copyFileSync(
        fileURLToPath(new URL('../npm/coven/bin/coven.js', import.meta.url)),
        wrapperPath
      );

      const [packageName, binaryName] =
        SIGNAL_TEST_PACKAGES[`${process.platform}-${process.arch}`];
      const nativeDir = path.join(wrapperDir, 'node_modules', ...packageName.split('/'));
      const nativeBinDir = path.join(nativeDir, 'bin');
      const nativePath = path.join(nativeBinDir, binaryName);
      mkdirSync(nativeBinDir, { recursive: true });
      writeFileSync(
        path.join(nativeDir, 'package.json'),
        JSON.stringify({ name: packageName, version: '0.0.0' })
      );
      writeFileSync(
        nativePath,
        [
          '#!/bin/sh',
          'printf "%s\\n%s\\n" "$COVEN_MEMORY_DASHBOARD_ENTRY" "$COVEN_MEMORY_DASHBOARD_NODE"',
          ''
        ].join('\n')
      );
      chmodSync(nativePath, 0o755);

      const dashboardDir = path.join(
        wrapperDir,
        'node_modules',
        '@opencoven',
        'coven-memory-dashboard'
      );
      const dashboardBinDir = path.join(dashboardDir, 'bin');
      const dashboardEntry = path.join(
        dashboardBinDir,
        'coven-memory-dashboard.mjs'
      );
      mkdirSync(dashboardBinDir, { recursive: true });
      writeFileSync(
        path.join(dashboardDir, 'package.json'),
        JSON.stringify({
          name: '@opencoven/coven-memory-dashboard',
          version: '0.0.0',
          type: 'module'
        })
      );
      writeFileSync(dashboardEntry, '');

      const versionRunner = path.join(fixture, 'run-wrapper-with-node-version.mjs');
      writeFileSync(
        versionRunner,
        [
          "Object.defineProperty(process.versions, 'node', {",
          "  value: process.env.TEST_NODE_VERSION",
          '});',
          `process.argv = [process.execPath, ${JSON.stringify(wrapperPath)}, ...JSON.parse(process.env.TEST_WRAPPER_ARGS)];`,
          `await import(${JSON.stringify(pathToFileURL(wrapperPath).href)});`,
          ''
        ].join('\n')
      );

      for (const args of [
        ['memory', 'open'],
        ['--color=never', 'memory', 'open'],
        ['--color', 'never', 'memory', 'open'],
        ['memory', '--color=never', 'open'],
        ['memory', '--color', 'never', 'open'],
        ['memory', 'open', '--color=never'],
        ['memory', 'open', '--color', 'never']
      ]) {
        const result = spawnSync(process.execPath, [wrapperPath, ...args], {
          encoding: 'utf8',
          env: {
            ...process.env,
            COVEN_MEMORY_DASHBOARD_ENTRY: '',
            COVEN_MEMORY_DASHBOARD_NODE: ''
          }
        });

        assert.equal(result.status, 0, `${args.join(' ')}: ${result.stderr}`);
        assert.deepEqual(
          result.stdout.trim().split('\n'),
          [realpathSync(dashboardEntry), process.execPath],
          args.join(' ')
        );
      }

      for (const args of [
        ['memory'],
        ['memory', '--json'],
        ['memory', '--json', 'open'],
        ['memory', 'open', '--json'],
        ['--color=never', 'memory', '--json'],
        ['memory', '--color', 'never', '--json']
      ]) {
        const result = spawnSync(process.execPath, [wrapperPath, ...args], {
          encoding: 'utf8',
          env: {
            ...process.env,
            COVEN_MEMORY_DASHBOARD_ENTRY: '',
            COVEN_MEMORY_DASHBOARD_NODE: ''
          }
        });

        assert.equal(result.status, 0, `${args.join(' ')}: ${result.stderr}`);
        assert.equal(result.stdout, '\n\n', args.join(' '));
      }

      const unsupportedNode = spawnSync(process.execPath, [versionRunner], {
        encoding: 'utf8',
        env: {
          ...process.env,
          COVEN_MEMORY_DASHBOARD_ENTRY: '',
          COVEN_MEMORY_DASHBOARD_NODE: '',
          TEST_NODE_VERSION: '23.11.0',
          TEST_WRAPPER_ARGS: JSON.stringify(['memory', 'open'])
        }
      });
      assert.equal(unsupportedNode.status, 1);
      assert.equal(unsupportedNode.stdout, '');
      assert.match(
        unsupportedNode.stderr,
        /coven memory open requires Node\.js 24 or newer/
      );

      for (const args of [
        ['memory', 'open', '--help'],
        ['memory', '--json'],
        ['memory', '--json', 'open'],
        ['memory', 'open', '--json']
      ]) {
        const result = spawnSync(process.execPath, [versionRunner], {
          encoding: 'utf8',
          env: {
            ...process.env,
            COVEN_MEMORY_DASHBOARD_ENTRY: '',
            COVEN_MEMORY_DASHBOARD_NODE: '',
            TEST_NODE_VERSION: '23.11.0',
            TEST_WRAPPER_ARGS: JSON.stringify(args)
          }
        });
        assert.equal(result.status, 0, `${args.join(' ')}: ${result.stderr}`);
      }
    } finally {
      rmSync(fixture, { recursive: true, force: true });
    }
  }
);

test('release publishes only the canonical @opencoven/cli wrapper package', () => {
  assert.deepEqual(wrapperPackageNameList(), ['@opencoven/cli']);
  assert.equal(wrapperPackageDirName('@opencoven/cli'), 'coven');
});

test('wrapperTextForPackage rewrites @opencoven/cli only when given a different target package name', () => {
  const source = '@opencoven/cli uses @opencoven/cli-macos and @opencoven/cli-linux-x64';
  // No-op when called with the primary package name.
  assert.equal(wrapperTextForPackage(source, '@opencoven/cli'), source);
});

test('wrapper declares windows native package as an optional dependency', () => {
  const packagePath = new URL(['..', 'npm', 'coven', 'package.json'].join('/'), import.meta.url);
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  assert.equal(packageJson.optionalDependencies['@opencoven/cli-windows'], '0.0.0');
});

test('wrapper binary maps linux x64 to linux native package and documents glibc requirement', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /'linux-x64': '@opencoven\/cli-linux-x64'/);
  assert.match(bin, /glibc-based Linux x64/);
});

test('wrapper binary maps windows x64 to windows native package and exe binary', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /'win32-x64': '@opencoven\/cli-windows'/);
  assert.match(bin, /process\.platform === 'win32' \? 'coven\.exe' : 'coven'/);
});

test('wrapper includes conventional Windows signal fallback', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const bin = readFileSync(binPath, 'utf8');
  assert.match(bin, /constants as osConstants/);
  assert.match(bin, /process\.platform === 'win32'/);
  assert.match(bin, /osConstants\.signals\[signal\]/);
  assert.match(bin, /128 \+ signalNumber/);
});

test('install docs do not claim macOS x64 support unless the wrapper maps darwin x64', () => {
  const binPath = new URL(['..', 'npm', 'coven', 'bin', 'coven.js'].join('/'), import.meta.url);
  const wrapperBin = readFileSync(binPath, 'utf8');
  const supportsDarwinX64 = wrapperBin.includes("'darwin-x64'");
  const docText = [
    ['..', 'README.md'],
    ['..', 'docs', 'install', 'index.md'],
    ['..', 'docs', 'install', 'npm.md'],
    ['..', 'docs', 'install', 'macos.md']
  ]
    .map((parts) => readFileSync(new URL(parts.join('/'), import.meta.url), 'utf8'))
    .join('\n');

  if (!supportsDarwinX64) {
    assert.doesNotMatch(docText, /macOS (?:arm64 or )?x64|macOS \(arm64 \+ x64\)|Intel Macs/i);
  }
});

test('release workflow builds and dry-runs linux x64 package', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /npm-target: linux-x64/);
  assert.match(workflow, /rust-target: x86_64-unknown-linux-gnu/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=linux-x64 --skip-build --dry-run --skip-wrapper/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=linux-x64 --skip-build --publish --skip-wrapper/);
});

test('release workflow builds macOS package on arm64 runner', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /npm-target: macos/);
  assert.match(workflow, /rust-target: aarch64-apple-darwin/);
  assert.match(workflow, /runner: macos-26/);
  assert.doesNotMatch(workflow, /runner: macos-latest/);
});

test('release workflow builds and dry-runs windows package', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /npm-target: windows/);
  assert.match(workflow, /rust-target: x86_64-pc-windows-msvc/);
  assert.match(workflow, /runner: windows-latest/);
  assert.match(workflow, /binary: coven\.exe/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=windows --skip-build --dry-run --skip-wrapper/);
  assert.match(workflow, /node scripts\/publish-npm\.mjs --target=windows --skip-build --publish --skip-wrapper/);
});

test('publishEnv preserves setup-node NODE_AUTH_TOKEN when NPM_TOKEN is absent', () => {
  assert.equal(publishEnv(false, { NODE_AUTH_TOKEN: 'from-setup-node', NPM_TOKEN: '' }).NODE_AUTH_TOKEN, 'from-setup-node');
});

test('publishEnv prefers explicit NPM_TOKEN when present', () => {
  assert.equal(publishEnv(false, { NODE_AUTH_TOKEN: 'from-setup-node', NPM_TOKEN: 'from-secret' }).NODE_AUTH_TOKEN, 'from-secret');
});

test('validatePublishToken allows real publish when only NODE_AUTH_TOKEN is set', () => {
  assert.doesNotThrow(() => validatePublishToken({ NODE_AUTH_TOKEN: 'from-setup-node' }, false));
});

test('validatePublishToken allows real publish when only NPM_TOKEN is set', () => {
  assert.doesNotThrow(() => validatePublishToken({ NPM_TOKEN: 'from-secret' }, false));
});

test('validatePublishToken rejects real publish when neither token nor OIDC is available', () => {
  assert.throws(() => validatePublishToken({}, false), /OIDC trusted publishing/);
});

test('validatePublishToken allows dry-run when no tokens are set', () => {
  assert.doesNotThrow(() => validatePublishToken({}, true));
});

test('isOidcContext requires both OIDC env vars to be present', () => {
  assert.equal(isOidcContext(OIDC_ENV), true);
  assert.equal(isOidcContext({ ACTIONS_ID_TOKEN_REQUEST_TOKEN: 'only-token' }), false);
  assert.equal(isOidcContext({ ACTIONS_ID_TOKEN_REQUEST_URL: 'only-url' }), false);
  assert.equal(isOidcContext({}), false);
});

test('validatePublishToken accepts OIDC context without any npm token', () => {
  assert.doesNotThrow(() => validatePublishToken(OIDC_ENV, false));
});

test('publishEnv leaves OIDC context untouched and does not smuggle in NODE_AUTH_TOKEN', () => {
  const result = publishEnv(false, { ...OIDC_ENV, NPM_TOKEN: 'should-not-be-used' });
  assert.equal(result.NODE_AUTH_TOKEN, undefined, 'OIDC publish must not inherit NODE_AUTH_TOKEN');
  assert.equal(result.NPM_TOKEN, 'should-not-be-used', 'unrelated env keys must pass through unchanged');
});

test('publishArgs adds --provenance and --access public only when OIDC is active for real publish', () => {
  assert.deepEqual(publishArgs(false, OIDC_ENV), ['publish', '--access', 'public', '--provenance']);
  assert.deepEqual(publishArgs(false, {}), ['publish', '--access', 'public']);
  assert.deepEqual(publishArgs(true, OIDC_ENV), ['publish', '--dry-run']);
  assert.deepEqual(publishArgs(true, {}), ['publish', '--dry-run']);
});

test('release workflow uses OIDC trusted publishing instead of a long-lived npm token', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(
    workflow,
    /npm-publish:[\s\S]*?permissions:[\s\S]*?id-token: write/,
    'npm-publish job must request id-token: write so npm publish --provenance can use OIDC'
  );
  // Build the legacy token reference at runtime so this test file itself does not
  // contain a literal NPM_TOKEN line that could be mistaken for a check that
  // expects the old behaviour.
  const legacyTokenRef = ['NPM', '_', 'TOKEN', ':'].join('');
  assert.equal(
    workflow.includes(legacyTokenRef),
    false,
    'release workflow must not reference NPM_TOKEN once OIDC trusted publishing is configured'
  );
  const legacySecretRef = ['secrets.', 'NPM', '_', 'ACCESS', '_', 'TOKEN'].join('');
  assert.equal(
    workflow.includes(legacySecretRef),
    false,
    'release workflow must not read the legacy NPM_ACCESS_TOKEN secret under OIDC'
  );
});

test('release workflow verifies the signed release tag before building or publishing', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /verify-tag:/, 'workflow must declare a verify-tag job');
  assert.match(
    workflow,
    /build-platform:[\s\S]*?needs:[\s\S]*?verify-tag/,
    'build-platform must depend on verify-tag so unsigned tags never reach the build matrix'
  );
  assert.match(
    workflow,
    /\.verification\.verified/,
    'verify-tag must consult GitHub`s signature verification API'
  );
  assert.match(
    workflow,
    /lightweight tag/,
    'verify-tag must explicitly reject lightweight (unsigned) tags'
  );
  assert.match(
    workflow,
    /git merge-base --is-ancestor .* origin\/main/,
    'verify-tag must ensure the tagged commit is contained in origin/main'
  );
  assert.match(
    workflow,
    /vars\.NPM_RELEASE_ALLOWED_SIGNERS/,
    'verify-tag must require an explicit SSH allowed-signers allowlist via NPM_RELEASE_ALLOWED_SIGNERS'
  );
  assert.match(
    workflow,
    /git verify-tag "\$TAG_NAME"/,
    'verify-tag must locally verify the tag against the SSH allowed signers file'
  );
  assert.match(
    workflow,
    /gpg\.format ssh[\s\S]*git tag -s/,
    'release instructions must show SSH signing configuration before the signed tag command'
  );
  assert.doesNotMatch(
    workflow,
    /Trigger: pushing an SSH-signed annotated tag/,
    'release instructions must not imply the GitHub push trigger itself rejects unsigned tags'
  );
  assert.match(
    workflow,
    /Trigger:[^\n]*any `v\*` tag push[\s\S]*Gate:[^\n]*requires an SSH-signed annotated tag/,
    'release instructions must separate the broad tag trigger from the signed-tag gate'
  );
  assert.match(
    workflow,
    /empty line/,
    'verify-tag must reject empty allowed signer entries before authorization checks'
  );
  assert.match(
    workflow,
    /gpg\.ssh\.allowedSignersFile/,
    'verify-tag must configure git with the release SSH allowed signers file'
  );
  assert.doesNotMatch(
    workflow,
    /tagger_email_lc|tagger_email=\$\(jq -r '\.tagger\.email|NPM_RELEASE_SIGNERS/,
    'verify-tag must not authorize releases by mutable tagger email allowlist'
  );
  assert.doesNotMatch(
    workflow,
    /\.signature\s*\{/,
    'verify-tag must not query the removed GitHub GraphQL Tag.signature field'
  );
  assert.match(
    workflow,
    /tag_object_type=\$\(jq -r '\.object\.type/,
    'verify-tag must inspect the annotated tag target type and SHA'
  );
  assert.match(
    workflow,
    /tag_object_type.*commit/s,
    'verify-tag must reject annotated tags that do not target commits'
  );
});

test('release workflow fail-closes signed recovery tags', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');

  assert.match(
    workflow,
    /node scripts\/release-npm-context\.mjs "\$GITHUB_REF_NAME"/,
    'workflow must derive stable release context from the pushed signed tag'
  );
  assert.match(
    workflow,
    /outputs:[\s\S]*release_mode:[\s\S]*release_tag:[\s\S]*npm_version:/,
    'verify-tag must expose validated release context to downstream jobs'
  );
  assert.match(
    workflow,
    /git verify-tag "\$RELEASE_TAG"/,
    'recovery must locally verify the original stable release tag'
  );
  assert.match(
    workflow,
    /BASE_TAG_PAYLOAD=.*gh api/,
    'recovery must query GitHub verification for the original release tag'
  );
  assert.match(
    workflow,
    /BASE_OBJECT_TYPE.*commit/s,
    'recovery must reject a base tag that does not target a commit'
  );
  assert.match(
    workflow,
    /git merge-base --is-ancestor "\$BASE_COMMIT_SHA" "\$TAGGED_COMMIT_SHA"/,
    'recovery tag must descend from the stable release tag'
  );
  assert.match(
    workflow,
    /git diff --name-only "\$BASE_COMMIT_SHA\.\.\$TAGGED_COMMIT_SHA"/,
    'recovery must inspect every changed path after the stable release'
  );
  assert.match(
    workflow,
    /Refusing recovery: changed path/,
    'recovery must fail closed on product or package drift'
  );
});

test('release workflow publishes only missing packages during recovery', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  const dryRunStart = workflow.indexOf('  npm-dry-run:');
  const publishStart = workflow.indexOf('  npm-publish:');
  assert.ok(dryRunStart !== -1, 'npm-dry-run job must exist in release workflow');
  assert.ok(publishStart !== -1, 'npm-publish job must exist in release workflow');
  assert.ok(dryRunStart < publishStart, 'npm-dry-run must appear before npm-publish in release workflow');
  const dryRun = workflow.slice(dryRunStart, publishStart);
  const publish = workflow.slice(publishStart);

  assert.match(workflow, /npm-dry-run:[\s\S]*?needs: \[build-platform, verify-tag\]/);
  assert.match(
    publish,
    /needs: \[build-platform, npm-dry-run, verify-tag\]/,
    'publish job must consume verified release context'
  );
  assert.match(
    publish,
    /name: Confirm expected partial npm state[\s\S]*@opencoven\/cli-linux-x64[\s\S]*@opencoven\/cli-windows[\s\S]*@opencoven\/cli-macos[\s\S]*@opencoven\/cli/,
    'recovery must prove the exact two-published, two-missing package state'
  );
  assert.match(
    publish,
    /Could not prove \$package_name@\$NPM_VERSION is absent/,
    'registry errors other than E404 must fail closed'
  );
  assert.match(
    dryRun,
    /--target=linux-x64 --skip-build --dry-run --skip-wrapper\s*\n\s*if: needs\.verify-tag\.outputs\.release_mode == 'normal'/,
    'Linux dry-run must skip an already-published version during recovery'
  );
  assert.match(
    dryRun,
    /--target=windows --skip-build --dry-run --skip-wrapper\s*\n\s*if: needs\.verify-tag\.outputs\.release_mode == 'normal'/,
    'Windows dry-run must skip an already-published version during recovery'
  );
  assert.match(
    dryRun,
    /--target=macos --skip-build --dry-run\s*\n\s*env:/,
    'macOS plus wrapper dry-run must run in normal and recovery modes'
  );
  assert.match(
    publish,
    /--target=linux-x64 --skip-build --publish --skip-wrapper\s*\n\s*if: needs\.verify-tag\.outputs\.release_mode == 'normal'/,
    'Linux publication must run only for normal releases'
  );
  assert.match(
    publish,
    /--target=windows --skip-build --publish --skip-wrapper\s*\n\s*if: needs\.verify-tag\.outputs\.release_mode == 'normal'/,
    'Windows publication must run only for normal releases'
  );
  assert.match(
    publish,
    /--target=macos --skip-build --publish\s*\n\s*env:/,
    'macOS plus wrapper publication must run in normal and recovery modes'
  );
  assert.doesNotMatch(
    workflow,
    /COVEN_NPM_VERSION: \$\{\{ github\.ref_name \}\}/,
    'publication must use the base npm version derived from verified tag context'
  );
  assert.match(
    workflow,
    /COVEN_NPM_VERSION: \$\{\{ needs\.verify-tag\.outputs\.npm_version \}\}/
  );
});

test('release workflow triggers only on signed v* tag pushes (no workflow_dispatch fallback)', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /on:\s*\n\s*push:\s*\n\s*tags:\s*\n\s*- 'v\*'/);
  assert.equal(
    /workflow_dispatch:/.test(workflow),
    false,
    'workflow_dispatch trigger must be removed so manual unsigned publishes are impossible'
  );
});

test('release workflow pins all third-party actions to immutable commit SHAs', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  const usesLines = workflow.split('\n').filter((line) => /^\s*-\s*uses:\s/.test(line));
  assert.ok(usesLines.length > 0, 'expected at least one `uses:` line in the release workflow');
  for (const line of usesLines) {
    const match = line.match(/uses:\s*([^@]+)@([^\s#]+)/);
    assert.ok(match, `could not parse uses line: ${line}`);
    const ref = match[2];
    assert.match(
      ref,
      /^[0-9a-f]{40}$/,
      `action ${match[1]} must be pinned to a 40-char commit SHA, found "${ref}" on line: ${line}`
    );
  }
});

test('release workflow concurrency keeps overlapping releases from interleaving', () => {
  const workflowPath = new URL(
    ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
    import.meta.url
  );
  const workflow = readFileSync(workflowPath, 'utf8');
  assert.match(workflow, /^concurrency:\s*\n\s*group:\s*release-npm/m);
  assert.match(workflow, /cancel-in-progress:\s*false/);
});

test('releasing guide documents signed partial-publish recovery', () => {
  const guide = readFileSync(
    new URL(['..', 'docs', 'reference', 'releasing.md'].join('/'), import.meta.url),
    'utf8'
  );

  assert.match(guide, /vX\.Y\.Z-recovery\.N/);
  assert.match(guide, /new signed recovery tag/i);
  assert.match(guide, /original release tag is an ancestor/i);
  assert.match(guide, /never move or reuse/i);
  assert.match(guide, /@opencoven\/cli-linux-x64[\s\S]*@opencoven\/cli-windows/);
  assert.match(guide, /@opencoven\/cli-macos[\s\S]*@opencoven\/cli/);
  assert.match(guide, /npm trust github/);
});

test('secret guard unit tests run in local and tag-driven release gates', () => {
  const prepublish = readFileSync(
    new URL('test-cli-prepublish.mjs', import.meta.url),
    'utf8'
  );
  const workflow = readFileSync(
    new URL(
      ['..', '.github', 'workflows', 'release-npm.yml'].join('/'),
      import.meta.url
    ),
    'utf8'
  );

  assert.match(
    prepublish,
    /run\('python3', \['scripts\/check-secrets-test\.py'\]\)/,
    'local prepublish must prove the scanner regression suite before scanning'
  );
  assert.match(
    workflow,
    /python3 scripts\/check-secrets-test\.py/,
    'tag-driven release gates must prove the scanner regression suite before publishing'
  );
});

test('prepublish smoke has explicit dry-run version override and registry failure message', () => {
  const scriptPath = new URL('test-cli-prepublish.mjs', import.meta.url);
  const script = readFileSync(scriptPath, 'utf8');

  assert.match(script, /COVEN_NPM_DRY_RUN_VERSION/);
  assert.match(script, /Could not read current \$\{packageName\} version/);
});

test('prepublish smoke rejects a missing dashboard tarball before running gates', () => {
  const fixture = mkdtempSync(path.join(tmpdir(), 'coven-dashboard-tarball-'));
  try {
    const scriptPath = fileURLToPath(new URL('test-cli-prepublish.mjs', import.meta.url));
    const missing = path.join(fixture, 'missing-dashboard.tgz');
    const childEnv = { ...process.env };
    delete childEnv.NODE_TEST_CONTEXT;
    const result = spawnSync(
      process.execPath,
      [
        scriptPath,
        '--target=unsupported-test-target',
        `--dashboard-tarball=${missing}`
      ],
      { encoding: 'utf8', env: childEnv }
    );

    assert.notEqual(result.status, 0);
    assert.match(
      `${result.stdout}\n${result.stderr}`,
      /dashboard tarball not found/
    );
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test('publish-npm entrypoint detection works with filesystem paths', () => {
  const scriptPath = fileURLToPath(new URL('publish-npm.mjs', import.meta.url));
  assert.equal(isMainModule(scriptPath, pathToFileURL(scriptPath).href), true);
  assert.equal(isMainModule(scriptPath, import.meta.url), false);
});

test('publish-npm uses Windows shell resolution for command shims', () => {
  const scriptPath = new URL('publish-npm.mjs', import.meta.url);
  const script = readFileSync(scriptPath, 'utf8');

  assert.match(script, /function spawnOptionsForCommand\(/);
  assert.match(script, /shell:\s*platform === 'win32'/);
  assert.match(script, /spawnSync\(command,\s*args,\s*\{\s*\.\.\.spawnOptionsForCommand\(\)/);
  assert.match(script, /spawnSync\('npm',\s*\['view'[\s\S]*?\.\.\.spawnOptionsForCommand\(\)/);
});

test('packageVersionPublished returns true when npm view exits 0 (version exists on registry)', () => {
  const result = packageVersionPublished('@opencoven/cli', '0.0.49', () => ({ status: 0 }));
  assert.equal(result, true);
});

test('packageVersionPublished returns false when npm view exits non-zero (E404, not yet published)', () => {
  const result = packageVersionPublished('@opencoven/cli', '99.99.99', () => ({ status: 1 }));
  assert.equal(result, false);
});

test('publish-npm.mjs fails closed when a package version already exists', () => {
  const scriptPath = new URL('publish-npm.mjs', import.meta.url);
  const script = readFileSync(scriptPath, 'utf8');

  assert.match(script, /function publishPackage\(/);
  assert.match(script, /Refusing to publish because this package version already exists on npm/);
  assert.doesNotMatch(script, /Refusing to publish wrappers/);
  assert.match(script, /publishPackage\(target\.packageName/);
  assert.match(script, /publishPackage\(packageName/);
  assert.doesNotMatch(script, /Skipping \$\{packageName\}@\$\{version\}: already published/);
});
