#!/usr/bin/env python3
"""Bound cooperative task allocation; never delete existing work to meet a cap."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

LIMITS = {'task worktrees': 12, 'local branches': 32, 'origin branches': 48}


def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()


def inventory():
    worktrees = git('worktree', 'list', '--porcelain').split('\n\n')
    local = git('for-each-ref', '--format=%(refname:short)', 'refs/heads').splitlines()
    remote = git('for-each-ref', '--format=%(refname:short)', 'refs/remotes/origin').splitlines()
    return {
        'task worktrees': max(0, len(worktrees) - 1),
        'local branches': len([b for b in local if b != 'main']),
        'origin branches': len([b for b in remote if b not in ('origin/main', 'origin/HEAD')]),
    }


def check(counts, creating=False):
    # A create consumes one task worktree and local branch, not a remote branch.
    projected = dict(counts)
    if creating:
        for key in ('task worktrees', 'local branches'):
            projected[key] += 1
    failures = [f'{key}: {projected[key]} > {limit}' for key, limit in LIMITS.items() if projected[key] > limit]
    if failures:
        raise ValueError('allocation budget exceeded; prune verified-safe owned work first: ' + '; '.join(failures))


def create(args):
    common = Path(git('rev-parse', '--path-format=absolute', '--git-common-dir'))
    lock = common / 'task-worktree-allocation.lock'
    try:
        lock.mkdir()
    except FileExistsError:
        raise ValueError('allocation lock exists; inspect its owner before recovering a stale lock') from None
    try:
        (lock / 'owner.json').write_text(json.dumps({'pid': os.getpid(), 'issue': args.issue}))
        destination = Path(args.path).absolute()
        if os.path.lexists(destination):
            raise ValueError('destination already exists; preserve it and inspect ownership')
        subprocess.run(['git', 'check-ref-format', '--branch', args.branch], check=True, stdout=subprocess.DEVNULL)
        subprocess.run(['git', 'rev-parse', '--verify', 'origin/main^{commit}'], check=True, stdout=subprocess.DEVNULL)
        check(inventory(), creating=True)
        subprocess.run(['git', 'worktree', 'add', '--no-track', '-b', args.branch, str(destination), 'origin/main'], check=True)
        print(f'Created issue-{args.issue} task worktree. Enter it, then run: coven claim acquire issue-{args.issue}')
    finally:
        (lock / 'owner.json').unlink(missing_ok=True)
        lock.rmdir()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('inventory')
    commands.add_parser('check')
    allocation = commands.add_parser('create')
    allocation.add_argument('--issue', type=int, required=True)
    allocation.add_argument('--branch', required=True)
    allocation.add_argument('--path', required=True)
    args = parser.parse_args()
    try:
        if args.command == 'create':
            if args.issue <= 0:
                raise ValueError('issue must be a positive GitHub issue number')
            create(args)
        else:
            counts = inventory()
            print(json.dumps({'counts': counts, 'limits': LIMITS}, indent=2))
            if args.command == 'check':
                check(counts)
    except (ValueError, subprocess.CalledProcessError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
