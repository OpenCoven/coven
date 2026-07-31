---
title: "Troubleshoot Coven core access"
summary: "Recover from missing harnesses, daemon/socket failures, state-directory issues, and project-boundary rejections."
read_when:
  - Coven cannot launch or list sessions
  - A harness is not detected in the current shell
description: "A diagnosis-first guide for Coven doctor, daemon status, COVEN_HOME, and project-root access failures."
---

# Troubleshoot Coven core access

Start with the smallest read-only check that can locate the failed boundary. Do not delete `COVEN_HOME`, session data, or sockets as a first response.

## No usable harness

```sh
coven doctor
```

Run the install or login command that `doctor` prints, in the same shell environment where you run Coven. Then repeat:

```sh
coven doctor --json | jq -e '.ok'
```

One ready supported harness is enough for core access. An unavailable optional adapter is not necessarily a blocker.

## Daemon is stopped, stale, or unreachable

```sh
coven daemon status
coven daemon status --json
```

For an expected stopped state:

```sh
coven daemon start
coven daemon status
```

For a stale or failing state, use the specific recovery sequence in [Daemon will not start](/help/daemon-wont-start). Do not expose the local socket to a network as a workaround.

## Unexpected state directory

```sh
printf '%s\n' "${COVEN_HOME:-$HOME/.coven}"
coven doctor
```

`COVEN_HOME` contains local daemon state, including the session store and socket. Confirm the variable is intentional before changing it. Use [Coven home](/daemon/coven-home) for supported layout and migration guidance.

## Project or cwd rejected

Run from the intended repository root, then narrow the request:

```sh
cd /path/to/project
coven run codex "describe this repository" --permission read-only
```

If you use `--cwd`, it must resolve inside that project root. This restriction is enforced by the Rust authority layer and should not be bypassed with symlinks or a parent directory.

## Session list or attach fails

```sh
coven doctor --json
coven daemon status --json
coven sessions --json
```

These commands distinguish environment readiness, daemon reachability, and ledger access. For a session that is no longer live, use its log or archive state rather than repeatedly trying to attach.

## Related

- [Core access](/guides/core-access)
- [Daemon troubleshooting](/help/daemon-wont-start)
- [Harness not found](/help/harness-not-found)
- [Paths and COVEN_HOME](/help/paths)
