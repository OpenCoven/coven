"""Real distro checks plus explicitly labelled mock contracts inside a container."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

REPO = Path(__file__).resolve().parents[2]


def source_hashes(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(Path(root).rglob('*')) if path.is_file() and path.suffix in ('.list', '.sources')}


def execute(output):
    sys.path.insert(0, str(REPO / 'scripts/cloud'))
    from runtime import native_diagnostic
    report = {'schema_version': 1, 'status': 'failed', 'cloud_parity': False, 'cases': []}
    source = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
    report['source_sha'] = source
    if source != os.environ['COVEN_CONTRACT_SOURCE_SHA']:
        return {**report, 'code': 'source-mismatch'}
    os_release = {}
    for line in Path('/etc/os-release').read_text().splitlines():
        key, separator, value = line.partition('=')
        if separator:
            os_release[key] = value.strip('"')
    report['distribution'] = os_release.get('ID')
    report['codename'] = os_release.get('VERSION_CODENAME')
    if (report['distribution'] != os.environ['COVEN_CONTRACT_DISTRO'] or
            report['codename'] != os.environ['COVEN_CONTRACT_CODENAME']):
        return {**report, 'code': 'distribution-mismatch'}

    def run(name, command, *, expected_code=None, env=None, evidence='real-container', timeout=1200, report_file=None):
        try:
            result = subprocess.run(command, cwd=REPO, env=env, capture_output=True, text=True,
                                    stdin=subprocess.DEVNULL, timeout=timeout, check=False)
        except subprocess.TimeoutExpired:
            report['cases'].append({'name': name, 'status': 'failed', 'code': 'timeout', 'evidence': evidence})
            raise RuntimeError('case-failed') from None
        if report_file is not None:
            try:
                structured = json.loads(result.stdout)
                if not isinstance(structured, dict):
                    raise ValueError
                (output / report_file).write_text(json.dumps(structured, indent=2) + '\n')
            except ValueError:
                report['cases'].append({'name': name, 'status': 'failed', 'code': 'invalid-json-report', 'evidence': evidence})
                raise RuntimeError('case-failed') from None
        diagnostic = native_diagnostic(result.stderr)
        passed = result.returncode == 0 if expected_code is None else (
            result.returncode != 0 and diagnostic is not None and diagnostic['code'] == expected_code)
        report['cases'].append({'name': name, 'status': 'passed' if passed else 'failed',
                                'exit_code': result.returncode, 'evidence': evidence,
                                **({'diagnostic': diagnostic} if diagnostic else {})})
        if not passed:
            raise RuntimeError('case-failed')
        return result.stdout

    try:
        helper = ['bash', 'scripts/install-native-link-dependencies.sh', 'libopenblas-dev']
        original_sources = source_hashes(Path('/etc/apt'))
        run('native-root-first', helper)
        run('native-root-second', helper)
        if source_hashes(Path('/etc/apt')) != original_sources:
            raise RuntimeError('apt-source-mutated')
        run('native-nonroot', ['setpriv', '--reuid=65534', '--regid=65534', '--clear-groups', *helper],
            expected_code='native_privilege')
        with tempfile.TemporaryDirectory(prefix='coven-source-contract-') as temporary:
            fixture = Path(temporary)
            (fixture / 'sources.list.d').mkdir()
            osfile = fixture / 'os-release'
            osfile.write_text('ID=alpine\nVERSION_CODENAME=edge\n')
            run('unsupported-os-fixture', helper, expected_code='native_distribution',
                env={**os.environ, 'COVEN_OS_RELEASE_FILE': str(osfile)}, evidence='injected-metadata')
            foreign = 'http://archive.ubuntu.com/ubuntu' if report['distribution'] == 'debian' else 'http://deb.debian.org/debian'
            (fixture / 'sources.list').write_text(f"deb {foreign} {report['codename']} main\n")
            run('foreign-source-fixture', helper, expected_code='native_source_validation',
                env={**os.environ, 'COVEN_APT_ETC_DIR': str(fixture)}, evidence='injected-metadata')
            (fixture / 'sources.list.d' / (report['distribution'] + '.sources')).write_text('Types: deb\nTypes: deb\n')
            run('malformed-source-fixture', helper, expected_code='native_source_validation',
                env={**os.environ, 'COVEN_APT_ETC_DIR': str(fixture)}, evidence='injected-metadata')
        # These tests inject command/network/toolchain outcomes. Running them in
        # a distro does not turn their mock outcomes into real network denials.
        for name in ('config', 'runtime', 'provision', 'bootstrap', 'readiness'):
            run('mock-' + name, ['python3', f'scripts/cloud/{name}-test.py'], evidence='mock-contract')
        cache = '/work/tool-cache'
        for attempt in ('first', 'second'):
            body = run('bootstrap-' + attempt, ['bash', 'scripts/cloud/bootstrap.sh', '--cache', cache], timeout=2400, report_file='bootstrap-' + attempt + '.json')
            result = json.loads(body)
            (output / ('bootstrap-' + attempt + '.json')).write_text(json.dumps(result, indent=2) + '\n')
            if result.get('development_ready') is not True or result.get('source_sha') != source:
                raise RuntimeError('bootstrap-not-ready')
            # Operational/provider readiness is not inferred from development readiness.
        body = run('fresh-readiness', ['bash', 'scripts/cloud/readiness.sh', '--cache', cache], timeout=1200, report_file='readiness.json')
        result = json.loads(body)
        (output / 'readiness.json').write_text(json.dumps(result, indent=2) + '\n')
        if result.get('development_ready') is not True or result.get('source_sha') != source:
            raise RuntimeError('readiness-failed')
        if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=REPO).strip():
            raise RuntimeError('tracked-source-mutated')
        if source_hashes(Path('/etc/apt')) != original_sources:
            raise RuntimeError('apt-source-mutated')
        report.update(status='passed', code='contract-passed', tracked_source_unchanged=True, apt_sources_unchanged=True)
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError):
        report['code'] = 'contract-failed'
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = execute(args.output)
    (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
    return 0 if result['status'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
