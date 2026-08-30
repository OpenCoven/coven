#!/usr/bin/env python3
from __future__ import annotations

import argparse
import sys
from pathlib import Path

CATEGORY_NAMES = [
    'docs_only',
    'rust',
    'afs',
    'channels',
    'openclaw',
    'npm_packaging',
    'engine',
    'workflow',
    'cargo_metadata',
]


def normalize(path: str) -> str:
    return path.replace('\\', '/')


def classify(paths: list[str]) -> dict[str, bool]:
    if not paths:
        raise ValueError('no paths provided')
    categories = {name: False for name in CATEGORY_NAMES}
    docs_only = True
    for raw in paths:
        path = normalize(raw)
        if not path:
            continue
        is_docs = path.startswith('docs/') or path.endswith('.md') or path in {'LICENSE', 'PATENTS'}
        if not is_docs:
            docs_only = False
        is_cargo_metadata = (
            path == 'Cargo.lock'
            or path == 'deny.toml'
            or path == 'Cargo.toml'
            or path.endswith('/Cargo.toml')
        )
        is_rust = is_cargo_metadata or path.startswith('crates/') or path.endswith('.rs')
        is_afs = is_cargo_metadata or path.startswith('crates/coven-afs/') or path == 'crates/coven-cli/src/afs_mount.rs' or path == 'scripts/afs-mount-smoke.sh'
        is_channels = path.startswith('packages/channels/') or path.startswith('crates/coven-channels/')
        is_openclaw = path.startswith('packages/openclaw-coven/') or path.startswith('crates/coven-openclaw/')
        is_help_surface = (
            path.startswith('docs/reference/cli')
            or path in {
                'docs/development/cli-core-functionality.md',
                'docs/guides/core-access.md',
                'docs/guides/session-operations.md',
                'docs/guides/automation-json.md',
                'scripts/cli-docs-test.mjs',
            }
        )
        is_npm = is_cargo_metadata or path.startswith('npm/') or path.startswith('crates/coven-cli/') or path in {
            'scripts/publish-npm.mjs',
            'scripts/publish-npm-test.mjs',
            'scripts/release-npm-context.mjs',
            'scripts/release-npm-platform-matrix.mjs',
            'scripts/release-required-checks.json',
            'scripts/verify-release-commit-gate.mjs',
            'scripts/verify-release-commit-gate-test.mjs',
            'scripts/test-cli-prepublish.mjs',
            'scripts/test-cli-prepublish-test.mjs',
            'scripts/user-journey-e2e.mjs',
            'scripts/user-journey-e2e-test.mjs',
            'scripts/fixtures/fake-codex.mjs',
        } or is_help_surface
        is_engine = path in {
            'crates/coven-cli/src/engine.rs',
            'crates/coven-cli/src/engine_install.rs',
            'crates/coven-cli/engine.lock',
            'scripts/pin-engine.sh',
        }
        is_workflow = path.startswith('.github/workflows/') or path in {
            'scripts/classify-ci-changes.py',
            'scripts/classify-ci-changes-test.py',
            'scripts/check-workflows.sh',
            'scripts/check-workflows-test.py',
            'scripts/check-ci-workflow-test.py',
        }
        categories['rust'] |= is_rust
        categories['afs'] |= is_afs
        categories['channels'] |= is_channels
        categories['openclaw'] |= is_openclaw
        categories['npm_packaging'] |= is_npm
        categories['engine'] |= is_engine
        categories['workflow'] |= is_workflow
        categories['cargo_metadata'] |= is_cargo_metadata
    categories['docs_only'] = docs_only
    if categories['workflow']:
        for name in CATEGORY_NAMES:
            if name != 'docs_only':
                categories[name] = True
    if not categories['docs_only'] and not any(categories[name] for name in CATEGORY_NAMES if name != 'docs_only'):
        categories['rust'] = True
    return categories


def write_github_output(results: dict[str, bool], handle) -> None:
    for name in CATEGORY_NAMES:
        handle.write(f'{name}={str(results[name]).lower()}\n')


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('--github-output')
    args = parser.parse_args()
    data = [line.strip() for line in sys.stdin.read().splitlines() if line.strip()]
    results = classify(data)
    if args.github_output:
        with open(args.github_output, 'a', encoding='utf-8') as handle:
            write_github_output(results, handle)
    else:
        write_github_output(results, sys.stdout)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
