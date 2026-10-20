#!/usr/bin/env python3
"""Transparent local verification primitives; CI remains merge authority."""
from pathlib import Path
import shlex
import subprocess


def execute(commands: list[tuple[str, list[str]]], root: Path) -> int:
    """Stream each check and retain the first failure while running later checks.

    Commands must be independent: a caller must not enqueue a consumer of an
    artifact whose producer might fail. Arguments are passed literally, never
    evaluated by a shell. This function does not provision tools or privileges.
    """
    first_failure = 0
    for name, argv in commands:
        print(f"RUN {name}: {shlex.join(argv)}", flush=True)
        try:
            result = subprocess.run(argv, cwd=root, check=False)
            status = result.returncode
            if status < 0:
                status = 128 - status
        except FileNotFoundError:
            print(f"missing executable: {argv[0]}", flush=True)
            status = 127
        except OSError as error:
            print(f"cannot execute {argv[0]}: {error.strerror}", flush=True)
            status = 126
        if status:
            print(f"FAIL {name} (exit {status})", flush=True)
            if not first_failure:
                first_failure = status
        else:
            print(f"PASS {name}", flush=True)
    return first_failure


def run_checks(checks, root):
    """Run a dependency-ordered plan. Skipped dependents never erase a failure."""
    results = {}
    first_failure = 0
    for check in checks:
        blocked = [name for name in check['needs'] if results.get(name) != 0]
        if blocked:
            print(f"SKIP {check['name']}: prerequisite failed or skipped: {', '.join(blocked)}", flush=True)
            results[check['name']] = 1
            first_failure = first_failure or 1
            continue
        status = execute([(check['name'], check['argv'])], root / check['cwd'])
        results[check['name']] = status
        first_failure = first_failure or status
    return first_failure


def validate_inventory(root):
    """Fail closed when CI's policy contract changes, including multiline steps."""
    import hashlib
    import json
    inventory = json.loads((root / 'scripts/dev-verify-policy.json').read_text())
    workflow = (root / '.github/workflows/ci.yml').read_text()
    try:
        policy = workflow.split('  policy-guard:\n', 1)[1].split('\n  automations-protocol-bundle:', 1)[0]
    except IndexError as error:
        raise ValueError('policy inventory drift: CI policy job not found') from error
    inline = []
    for line in policy.splitlines():
        if line.startswith('      - run: '):
            argv = shlex.split(line.split('run: ', 1)[1])
            if argv[0] == 'python':
                argv[0] = 'python3'
            if argv[0] == 'scripts/check-workflows.sh':
                argv.insert(0, 'bash')
            inline.append(argv)
    if (hashlib.sha256(policy.encode()).hexdigest() != inventory['policy_sha256']
            or inline != inventory['commands']):
        raise ValueError('policy inventory drift: review ci.yml and update the mapped commands and digest')
    return inventory['commands']


def load_script(root, name):
    import importlib.util
    import sys
    # Planning must not create ignored files before the exact-checkout audit.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), root / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def plan(tier, revision_range, changed, root, system):
    checks = []
    skips = []

    def add(name, argv, cwd='.', needs=()):
        checks.append(dict(name=name, argv=argv, cwd=cwd, needs=list(needs)))

    internal = ['python3', 'scripts/dev_verify.py', '--internal']
    add('rust-pins', internal + ['rust-pins'])
    if tier != 'quick':
        add('node-pin', internal + ['node-pin'])
        add('policy-inventory', internal + ['policy-inventory'])
    if tier == 'full':
        # Audit before package installs or tests create ignored checkout files.
        add('native-audit', internal + ['native-audit'], needs=['rust-pins', 'node-pin'])
    add('format', ['cargo', 'fmt', '--check'], needs=['rust-pins'])
    add('check', ['cargo', 'check', '--workspace', '--locked'], needs=['rust-pins'])
    add('secrets', ['python3', 'scripts/check-secrets.py'])
    add('privacy-range', ['python3', 'scripts/check-coven-privacy.py', '--range', revision_range])
    add('privacy-staged', ['python3', 'scripts/check-coven-privacy.py', '--staged'])
    add('working-policy', internal + ['working-policy'])
    if tier == 'quick':
        skips.append('PR/full checks: outside quick scope (clippy, tests, full policy, packages, dependency audit and feature suites)')
        return checks, skips

    add('clippy', ['cargo', 'clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'], needs=['rust-pins'])
    add('workspace-tests', ['cargo', 'test', '--workspace', '--locked'], needs=['rust-pins'])
    add('docs-range', ['python3', 'scripts/check-docs-ownership.py', '--range', revision_range])
    # Validate at plan construction as well as in the visible execution plan.
    browser_tools_setup = None
    for index, argv in enumerate(validate_inventory(root)):
        if argv == ['python3', 'scripts/check-secrets.py']:
            continue  # Already mandatory in every tier.
        needs = ['policy-inventory']
        if argv[0] in ('node', 'npm'):
            needs.append('node-pin')
        if argv[:4] == ['npm', 'ci', '--prefix', 'scripts/browser']:
            browser_tools_setup = f'policy-{index + 1}'
        if argv == ['node', '--test', 'scripts/browser-capability.test.mjs']:
            if browser_tools_setup is None:
                raise ValueError('browser contract tests require preceding locked tooling setup')
            needs.append(browser_tools_setup)
        add(f'policy-{index + 1}', argv, needs=needs)
    add('dependency-audit', ['cargo', 'deny', '--all-features', 'check', 'advisories', 'licenses', 'bans', 'sources'], needs=['rust-pins'])

    categories = load_script(root, 'classify-ci-changes').classify(changed) if changed else {}
    for package, category, manager in [('channels', 'channels', 'npm'), ('openclaw-coven', 'openclaw', 'pnpm')]:
        if tier != 'full' and not categories.get(category, False):
            skips.append(f'{package}: not applicable to changed paths under CI classification')
            continue
        cwd = f'packages/{package}'
        needs = ['node-pin']
        if manager == 'pnpm':
            add('pnpm-pin', internal + ['pnpm-pin'], needs=needs)
            needs = ['pnpm-pin']
        install = [manager, 'ci'] if manager == 'npm' else [manager, 'install', '--frozen-lockfile', '--ignore-scripts']
        add(f'{package}-install', install, cwd, needs)
        add(f'{package}-build', [manager, 'run', 'build'], cwd, [f'{package}-install'])
        add(f'{package}-test', [manager, 'test'], cwd, [f'{package}-install'])

    add('browser-readiness', ['node', 'scripts/browser-capability.mjs', '--check'], needs=['node-pin', browser_tools_setup])

    if tier == 'full':
        add('threads-unit', ['cargo', 'test', '--locked', '-p', 'coven-cli', '--bin', 'coven', '--features', 'threads-test-clock', 'threads_'], needs=['rust-pins'])
        add('threads-journeys', ['cargo', 'test', '--locked', '-p', 'coven-cli', '--test', 'threads_e2e', '--features', 'threads-test-clock', '--test', 'threads_identity_invariants', '--test', 'threads_protected_intake', '--test', 'threads_terminal_recovery', '--no-fail-fast'], needs=['rust-pins'])
        if system in ('Darwin', 'Linux'):
            add('afs-clippy', ['cargo', 'clippy', '-p', 'coven-afs', '--features', 'mount', '--all-targets', '--', '-D', 'warnings'], needs=['rust-pins'])
            add('afs-tests', ['cargo', 'test', '-p', 'coven-afs', '--features', 'mount', '--locked'], needs=['rust-pins'])
        else:
            skips.append('AFS mount feature: CI supports Linux/macOS only')
        if system == 'Darwin':
            add('restricted-tests', ['cargo', 'test', '--locked', '-p', 'coven-restricted-runtime-macos', '-p', 'coven-restricted-runtime', '--no-fail-fast'], needs=['rust-pins'])
            add('restricted-clippy', ['cargo', 'clippy', '--locked', '-p', 'coven-restricted-runtime-macos', '-p', 'coven-restricted-runtime', '--all-targets', '--', '-D', 'warnings'], needs=['rust-pins'])
        else:
            skips.append('restricted runtime: macOS host required; CI remains responsible')
    else:
        skips.append('native audit, Threads and AFS/restricted feature suites: full tier or routed CI')
    skips.extend([
        'cross-platform Linux/Windows/macOS runner matrix: CI-only; this run proves only its host',
        'exact-commit protocol and authority artifact packaging/upload: CI-only required jobs',
        'release-target npm onboarding and pinned engine contract: routed CI-only jobs',
        'performance baseline and real mount probe: CI informational/post-merge jobs',
        'browser downloads are opt-in; distro/bootstrap contracts remain separate B3 capability',
        'protected-main PR gate: CI-only final merge authority; local success is not CI acceptance',
    ])
    return checks, skips


def git_output(root, *args):
    return subprocess.check_output(['git', *args], cwd=root, text=True)


def resolve_range(root, revision_range):
    """Pin both ends before shared worktree refs can move during verification."""
    if revision_range.startswith('-') or '..' not in revision_range:
        raise ValueError('revision range must be START..END or START...END, not an option')
    separator = '...' if '...' in revision_range else '..'
    ends = revision_range.split(separator)
    if len(ends) != 2 or not all(ends):
        raise ValueError('revision range requires explicit START and END commits')
    commits = [git_output(root, 'rev-parse', '--verify', '--end-of-options',
                          revision + '^{commit}').strip() for revision in ends]
    return separator.join(commits)


def changed_paths(root, revision_range):
    if revision_range.startswith('-') or '..' not in revision_range:
        raise ValueError('revision range must be START..END or START...END, not an option')
    paths = set()
    for args in [('diff', '--name-only', '-z', revision_range, '--'),
                 ('diff', '--cached', '--name-only', '-z', '--'),
                 ('diff', '--name-only', '-z', '--'),
                 ('ls-files', '--others', '--exclude-standard', '-z')]:
        paths.update(filter(None, git_output(root, *args).split('\0')))
    return sorted(paths)


def internal_check(name, root):
    import json
    import re
    import tempfile
    if name == 'policy-inventory':
        validate_inventory(root)
        return 0
    if name in ('rust-pins', 'node-pin', 'pnpm-pin'):
        if name == 'rust-pins':
            pin = re.search(r'^channel\s*=\s*"([^"]+)"', (root / 'rust-toolchain.toml').read_text(), re.M).group(1)
            commands = [('rustc', ['rustc', '--version'], pin), ('cargo', ['cargo', '--version'], pin)]
        elif name == 'node-pin':
            commands = [('node', ['node', '--version'], (root / '.node-version').read_text().strip())]
        else:
            manager = json.loads((root / 'packages/openclaw-coven/package.json').read_text())['packageManager']
            if not manager.startswith('pnpm@'):
                raise ValueError('unsupported packageManager; expected a pnpm pin')
            commands = [('pnpm', ['pnpm', '--version'], manager.split('@', 1)[1])]
        for tool, argv, expected in commands:
            print(f"RUN version: {shlex.join(argv)}", flush=True)
            observed = subprocess.check_output(argv, cwd=root, text=True).strip()
            version = observed.split()[1] if tool in ('rustc', 'cargo') else observed.lstrip('v')
            print(f'{tool}: expected {expected}; observed {version}', flush=True)
            if version != expected:
                raise ValueError(f'{tool} pin mismatch; provision the repository pin before verification')
        return 0
    if name == 'working-policy':
        paths = set(filter(None, git_output(root, 'diff', '--name-only', '-z', 'HEAD', '--').split('\0')))
        paths.update(filter(None, git_output(root, 'ls-files', '--others', '--exclude-standard', '-z').split('\0')))
        files = sorted(path for path in paths if (root / path).is_file())
        docs = load_script(root, 'check-docs-ownership')
        errors = []
        for path in files:
            if docs.is_public_doc(path):
                errors.extend(docs.validate_page(path, (root / path).read_text()))
        for error in errors:
            print(error, flush=True)
        status = 1 if errors else 0
        if files:
            privacy_status = execute([('privacy-working', ['python3', 'scripts/check-coven-privacy.py', '--files', *files])], root)
            status = status or privacy_status
        return status
    if name == 'native-audit':
        output = Path(tempfile.mkdtemp(prefix='coven-dev-verify-')) / 'audit'
        print(f'Native audit evidence directory: {output}', flush=True)
        return execute([('native-audit-command', ['node', 'conformance/automations/runner/audit.mjs', '--output', str(output)])], root)
    raise ValueError(f'unknown internal check: {name}')


def main():
    import argparse
    import platform
    import sys
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description='Transparent local verification; CI remains final merge authority.')
    tiers = parser.add_mutually_exclusive_group(required=True)
    for tier in ('quick', 'pr', 'full'):
        tiers.add_argument('--' + tier, dest='tier', action='store_const', const=tier)
    tiers.add_argument('--internal', help=argparse.SUPPRESS)
    parser.add_argument('--range', default='origin/main...HEAD', dest='revision_range', help='committed change range (default: origin/main...HEAD); working changes are also inspected')
    parser.add_argument('--plan', action='store_true', help='print commands and skips without executing checks; never reports PASS')
    args = parser.parse_args()
    try:
        if args.internal:
            return internal_check(args.internal, root)
        revision_range = resolve_range(root, args.revision_range)
        changed = changed_paths(root, revision_range)
        checks, skips = plan(args.tier, revision_range, changed, root, platform.system())
        print(f'Tier: {args.tier}; root: {root}; requested range: {args.revision_range}', flush=True)
        print(f'Resolved committed range: {revision_range}', flush=True)
        print('CI remains final merge authority.', flush=True)
        for reason in skips:
            print(f'SKIP {reason}', flush=True)
        if args.plan:
            for check in checks:
                print(f"PLAN {check['name']} (cwd={check['cwd']}; needs={','.join(check['needs']) or 'none'}): {shlex.join(check['argv'])}")
            return 0
        result = run_checks(checks, root)
        print(f'Local {args.tier} result: {result}; CI-only and out-of-tier checks above remain unverified.', flush=True)
        return result
    except FileNotFoundError as error:
        print(f'FAIL verification: missing executable or required file: {error.filename}', file=sys.stderr)
        return 127
    except subprocess.CalledProcessError as error:
        print(f'FAIL verification preflight (exit {error.returncode})', file=sys.stderr)
        return error.returncode if error.returncode > 0 else 128 - error.returncode
    except (ValueError, KeyError, OSError) as error:
        print(f'FAIL verification: {error}', file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print('FAIL verification: interrupted', file=sys.stderr)
        return 130


if __name__ == '__main__':
    raise SystemExit(main())
