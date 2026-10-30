#!/usr/bin/env python3
"""Bound cooperative task allocation; never delete existing work to meet a cap."""
import argparse
from contextlib import contextmanager
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


@contextmanager
def allocation_lock(issue):
    common = Path(git('rev-parse', '--path-format=absolute', '--git-common-dir'))
    lock = common / 'task-worktree-allocation.lock'
    try:
        lock.mkdir()
    except FileExistsError:
        raise ValueError('allocation lock exists; inspect its owner before recovering a stale lock') from None
    try:
        (lock / 'owner.json').write_text(json.dumps({'pid': os.getpid(), 'issue': issue}))
        yield
    finally:
        (lock / 'owner.json').unlink(missing_ok=True)
        lock.rmdir()


def create(args):
    with allocation_lock(args.issue):
        destination = Path(args.path).absolute()
        if os.path.lexists(destination):
            raise ValueError('destination already exists; preserve it and inspect ownership')
        subprocess.run(['git', 'check-ref-format', '--branch', args.branch], check=True, stdout=subprocess.DEVNULL)
        subprocess.run(['git', 'rev-parse', '--verify', 'origin/main^{commit}'], check=True, stdout=subprocess.DEVNULL)
        check(inventory(), creating=True)
        subprocess.run(['git', 'worktree', 'add', '--no-track', '-b', args.branch, str(destination), 'origin/main'], check=True)
        print(f'Created issue-{args.issue} task worktree. Enter it, then run: coven claim acquire issue-{args.issue}')


def record_publication_exception(args, branch, head, count):
    if not args.exception_issue or args.exception_issue <= 0 or not args.reason or not args.reason.strip():
        raise ValueError('origin branches budget exceeded; prune first or provide an open exception issue and reason')
    endpoint = f'repos/OpenCoven/coven/issues/{args.exception_issue}'
    issue = json.loads(subprocess.check_output(['gh', 'api', endpoint], text=True))
    if issue.get('state') != 'open':
        raise ValueError('publication requires an open exception issue')
    reason = ' '.join(args.reason.split())
    body = (f'Publication budget exception for #{args.issue}: new branch `{branch}` at `{head}`. '
            f'Live origin task branches: {count}; limit: {LIMITS["origin branches"]}; projected: {count + 1}.\n\n'
            f'Reason: {reason}\n\n'
            'This records one requested publication, not a limit change or completed push. '
            'Retire the delivered branch after protected delivery and record retained cleanup blockers on this issue.')
    comment = json.loads(subprocess.check_output(
        ['gh', 'api', '--method', 'POST', endpoint + '/comments', '--input', '-'],
        input=json.dumps({'body': body}), text=True,
    ))
    if type(comment.get('id')) is not int or comment['id'] <= 0:
        raise ValueError('publication exception was not recorded')
    print(f'Recorded budget exception on OpenCoven/coven#{args.exception_issue}, comment {comment["id"]}')


def publish(args):
    with allocation_lock(args.issue):
        branch = git('branch', '--show-current')
        if not branch or branch == 'main':
            raise ValueError('publish requires a checked-out task branch; main and detached HEAD are refused')
        head = git('rev-parse', 'HEAD')
        # Count the actual push destination, which may differ from the fetch
        # URL. Multiple destinations cannot share one observed budget.
        destinations = git('remote', 'get-url', '--push', '--all', 'origin').splitlines()
        if len(destinations) != 1:
            raise ValueError('publication requires exactly one push destination for origin')
        destination = destinations[0]
        heads = dict(line.split('\t', 1)[::-1] for line in git('ls-remote', '--heads', destination).splitlines())
        count = sum(ref != 'refs/heads/main' for ref in heads)
        ref = 'refs/heads/' + branch
        new = ref not in heads
        if new and count + 1 > LIMITS['origin branches']:
            record_publication_exception(args, branch, head, count)
        command = ['git', 'push']
        if new:
            # Empty expectation permits creation only, never replacing a ref
            # that appeared after the live read. Updates remain fast-forward.
            command.append(f'--force-with-lease={ref}:')
        # Bind both the observed destination and source even if external Git
        # configuration or branch state changes after this snapshot.
        subprocess.run(command + [destination, f'{head}:{ref}'], check=True)
        print(f'Push accepted for {branch} at {head}; verify hosted CI and protected delivery separately.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('inventory')
    commands.add_parser('check')
    allocation = commands.add_parser('create')
    allocation.add_argument('--issue', type=int, required=True)
    allocation.add_argument('--branch', required=True)
    allocation.add_argument('--path', required=True)
    publication = commands.add_parser('publish')
    publication.add_argument('--issue', type=int, required=True)
    publication.add_argument('--exception-issue', type=int)
    publication.add_argument('--reason')
    args = parser.parse_args()
    try:
        if args.command in ('create', 'publish'):
            if args.issue <= 0:
                raise ValueError('issue must be a positive GitHub issue number')
            if args.command == 'create':
                create(args)
            else:
                publish(args)
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
