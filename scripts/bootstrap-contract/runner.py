#!/usr/bin/env python3
"""Run exact-source bootstrap contracts in disposable containers, not Cloud VMs."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import uuid

HERE = Path(__file__).resolve().parent


def load_images(source=HERE / 'images.json'):
    try:
        entries = json.loads(Path(source).read_text())
        if set(entries) != {'debian12', 'ubuntu24'}:
            raise ValueError
        for entry in entries.values():
            if (not re.fullmatch(r'docker.io/library/(debian|ubuntu)@sha256:[0-9a-f]{64}', entry['image'])
                    or entry['platform'] != 'linux/amd64'
                    or entry['distribution'] not in ('debian', 'ubuntu')
                    or entry['codename'] not in ('bookworm', 'noble')):
                raise ValueError
    except (OSError, ValueError, TypeError, KeyError):
        raise ValueError('invalid pinned container inventory') from None
    return entries


def validate_output(repo, output):
    repo, output = Path(repo).resolve(), Path(output).resolve()
    if repo == output or repo in output.parents or output in repo.parents:
        raise ValueError('output and repository paths must not overlap')
    if output.exists():
        raise ValueError('choose a new output directory to preserve previous evidence')
    return output


def container_command(image, source, output, sha, name):
    return ['docker', 'run', '--rm', '--name', name, '--platform', image['platform'],
            '--security-opt', 'no-new-privileges',
            '--mount', f'type=bind,source={source},target=/source,readonly',
            '--mount', f'type=bind,source={output},target=/evidence',
            '--env', 'DEBIAN_FRONTEND=noninteractive', '--env', 'COVEN_CONTRACT_SOURCE_SHA=' + sha,
            '--env', 'COVEN_CONTRACT_DISTRO=' + image['distribution'],
            '--env', 'COVEN_CONTRACT_CODENAME=' + image['codename'],
            image['image'], 'bash', '/source/scripts/bootstrap-contract/container.sh']


def git_environment():
    return {'PATH': os.environ.get('PATH', os.defpath), 'GIT_CONFIG_NOSYSTEM': '1',
            'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_TERMINAL_PROMPT': '0', 'GIT_OPTIONAL_LOCKS': '0'}


def git(repo, *args):
    return subprocess.check_output(['git', '-c', 'core.fsmonitor=false', '-c', 'core.hooksPath=' + os.devnull,
                                    '-C', str(repo), *args], env=git_environment(),
                                   text=True, stderr=subprocess.DEVNULL).strip()


def remove_container(name):
    try:
        removed = subprocess.run(['docker', 'rm', '-f', name], capture_output=True, timeout=30, check=False)
        if removed.returncode == 0:
            return True
        # --rm may already have removed it. A failed inspect/list is not evidence
        # of absence; require a successful exact-name query with no matching IDs.
        remaining = subprocess.run(['docker', 'ps', '-aq', '--filter', 'name=^/' + name + '$'],
                                   capture_output=True, text=True, timeout=30, check=False)
        return remaining.returncode == 0 and not remaining.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return False


def execute(repo, image, output):
    output = validate_output(repo, output)
    report = {'schema_version': 1, 'environment': 'disposable-container', 'cloud_parity': False,
              'image': image, 'status': 'failed'}
    if shutil.which('docker') is None:
        return {**report, 'code': 'docker-unavailable'}
    if git(repo, 'status', '--porcelain', '--untracked-files=no'):
        return {**report, 'code': 'dirty-source'}
    if Path(git(repo, 'rev-parse', '--show-toplevel')).resolve() != Path(repo).resolve():
        return {**report, 'code': 'repository-root-required'}
    sha = git(repo, 'rev-parse', 'HEAD')
    report['source_sha'] = sha
    for filename in ('scripts/cloud/bootstrap.sh', 'scripts/cloud/readiness.sh',
                     'scripts/bootstrap-contract/container.sh', 'scripts/bootstrap-contract/inside.py'):
        try:
            git(repo, 'cat-file', '-e', sha + ':' + filename)
        except subprocess.CalledProcessError:
            return {**report, 'code': 'dependency-source-missing', 'required_file': filename}
    output.mkdir(parents=True)
    name = 'coven-contract-' + uuid.uuid4().hex
    try:
        with tempfile.TemporaryDirectory(prefix='coven-contract-source-') as temporary:
            source = Path(temporary) / 'repo'
            source.mkdir()
            git(source, 'init', '-q')
            # Fetch only the exact commit into a self-contained .git directory;
            # worktree .git pointer files cannot be mounted across hosts safely.
            git(source, 'fetch', '-q', '--depth=1', Path(repo).resolve().as_uri(), sha)
            git(source, 'checkout', '-q', '--detach', 'FETCH_HEAD')
            if (git(source, 'rev-parse', 'HEAD') != sha
                    or Path(git(source, 'rev-parse', '--show-toplevel')).resolve() != source.resolve()):
                raise ValueError('source checkout mismatch')
            command = container_command(image, source, output, sha, name)
            print(json.dumps({'status': 'run', 'command': command}), flush=True)
            # Container scripts emit safe structured output only. Retain raw Docker
            # diagnostics locally without echoing proxy/registry credentials.
            with (output / 'container.stdout').open('wb') as stdout, (output / 'container.stderr').open('wb') as stderr:
                result = subprocess.run(command, stdout=stdout, stderr=stderr, timeout=5400, check=False)
            report['exit_code'] = result.returncode
            if result.returncode != 0:
                report['code'] = 'container-failed'
            else:
                result_report = json.loads((output / 'result.json').read_text())
                if (result_report.get('status') != 'passed' or result_report.get('source_sha') != sha
                        or result_report.get('distribution') != image['distribution']):
                    raise ValueError('container evidence does not match requested source and distribution')
                report.update(status='passed', code='contract-passed')
    except subprocess.TimeoutExpired:
        report['code'] = 'container-timeout'
    except (OSError, ValueError, subprocess.SubprocessError):
        report['code'] = 'container-or-source-error'
    finally:
        # Remove only our unpredictable owned container, including after client timeout.
        if not remove_container(name):
            report.update(status='failed', code='container-cleanup-failed')
        (output / 'runner.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', choices=['debian12', 'ubuntu24'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repo', type=Path, default=HERE.parents[1])
    parser.add_argument('--plan', action='store_true')
    args = parser.parse_args()
    try:
        image = load_images()[args.image]
        if args.plan:
            output = validate_output(args.repo, args.output)
            report = {'status': 'planned', 'cloud_parity': False, 'image': image,
                      'command': container_command(image, Path('/temporary/exact-source'), output, '<commit-sha>', '<owned-container>')}
        else:
            report = execute(args.repo, image, args.output)
    except (OSError, ValueError, subprocess.SubprocessError):
        report = {'status': 'failed', 'code': 'invalid-input-or-source'}
    print(json.dumps(report, sort_keys=True))
    return 0 if report['status'] in ('passed', 'planned') else 1


if __name__ == '__main__':
    raise SystemExit(main())
