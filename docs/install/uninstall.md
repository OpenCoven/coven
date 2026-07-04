---
summary: "How to remove Coven cleanly without losing project sessions."
read_when:
  - Removing Coven from a workstation
title: "Uninstalling Coven"
description: "Uninstall Coven cleanly: stop the daemon, remove the wrapper, and decide whether to keep or wipe COVEN_HOME and the session ledger."
---

# Uninstalling Coven

Uninstall has two separate decisions:

1. Remove the `coven` command.
2. Keep or delete `COVEN_HOME`, which contains local session history, sockets, logs, and keys.

Stop the daemon first:

```sh
coven daemon stop
```

## npm wrapper

```sh
npm uninstall -g @opencoven/cli
```

Verify the command is gone:

```sh
command -v coven
```

PowerShell:

```powershell
Get-Command coven
```

If another install path still exposes `coven`, remove that binary or adjust `PATH`.

## Source or cargo install

Remove the binary you copied onto `PATH`:

```sh
rm "$HOME/.local/bin/coven"
```

Windows PowerShell:

```powershell
Remove-Item "$env:USERPROFILE\.local\bin\coven.exe"
```

Adjust the path if you installed the binary somewhere else.

## Keep or delete state

To preserve sessions for a later reinstall, leave `COVEN_HOME` in place.

To delete the default state directory on macOS, Linux, or WSL2:

```sh
rm -rf "$HOME/.coven"
```

PowerShell:

```powershell
Remove-Item -Recurse -Force "$env:USERPROFILE\.coven"
```

Only delete `COVEN_HOME` after confirming there are no sessions, logs, or local keys you need to keep.

## Services

If you installed a launchd user agent:

```sh
launchctl bootout gui/UID/coven
rm ~/Library/LaunchAgents/coven.plist
```

Replace `UID` with the output of `id -u`.

If you installed a systemd user unit:

```sh
systemctl --user disable --now coven-daemon.service
rm "$HOME/.config/systemd/user/coven-daemon.service"
systemctl --user daemon-reload
```

## Related

- [COVEN_HOME layout](/daemon/coven-home)
- [Install overview](/install/index)
- [Troubleshooting](/TROUBLESHOOTING)
