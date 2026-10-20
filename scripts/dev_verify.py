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
