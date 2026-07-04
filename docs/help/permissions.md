---
summary: "Socket permissions, accessibility prompts, and sandboxing notes."
read_when:
  - Resolving permission errors
title: "Permissions"
description: "Permissions Coven needs on each host: filesystem access for COVEN_HOME, socket bind rights, and the same-user model the daemon enforces for clients."
---

Coven runs as your local user. It needs permission to write `COVEN_HOME`, create
or connect to the local daemon socket, and launch the harness CLI from the
project directory you choose.

Check the active setup:

```sh
coven doctor
coven daemon status
```

## State directory permissions

The state directory should be owned by the same user that runs Coven:

```sh
mkdir -p "$HOME/.coven"
chmod 700 "$HOME/.coven"
COVEN_HOME="$HOME/.coven" coven doctor
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven doctor
```

Do not use `sudo coven` to work around permission errors. That creates root-owned
state and can make the normal user unable to start the daemon later.

## Socket access

`coven daemon status` should be run by the same user that started the daemon:

```sh
coven daemon restart
coven daemon status
```

If a supervisor starts the daemon, make the supervisor user, `PATH`, and
`COVEN_HOME` explicit. A daemon running under another account will not share your
shell's state or provider authentication.

## Harness permissions

Coven does not own provider credentials. Codex and Claude Code authenticate
themselves:

```sh
codex login
claude doctor
coven doctor
```

If `coven doctor` reports the harness is ready but `coven run` fails, run the
harness command directly in the same shell and project directory. Fix provider
auth or sandbox prompts there first.

## Platform notes

- macOS may show accessibility, terminal, or developer-tool prompts from the
  harness CLI. Approve those for the terminal app that launches Coven.
- Linux systemd user services must define `PATH` and `COVEN_HOME` if they differ
  from system defaults.
- Windows shells should run Coven and harness CLIs from the same user profile.
- WSL2 should keep state and harness installs inside WSL, not split across
  Windows and Linux environments.

After changing permissions, restart:

```sh
coven daemon restart
coven doctor
```
