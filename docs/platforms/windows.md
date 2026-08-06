---
summary: "Coven on Windows — caveats and supported flows."
read_when:
  - Operating on Windows
title: "Windows"
description: "Coven on Windows: native and WSL2 environments, COVEN_HOME isolation, and owner-only local IPC for harness sessions."
---

## Install path

For native Windows, install from PowerShell or Windows Terminal:

```powershell
npm install -g @opencoven/cli
coven doctor
```

Run Coven and harness CLIs from the same environment. A harness installed only
inside WSL2 is not visible to native PowerShell unless you bridge it yourself.

## Native Windows versus WSL2

Pick one environment for a working session:

- Native Windows: use PowerShell, native paths, and native harness installs.
- WSL2: follow [WSL2](/platforms/wsl2), use Linux paths, and keep state inside
  the distro.

Do not point native Windows Coven and WSL2 Coven at the same `COVEN_HOME`.

## State

Use the default user state unless you need isolation:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven doctor
```

Keep the directory on a local path owned by your Windows user.

## Daemon transport

Native Windows runs the daemon on an owner-only named pipe scoped to the
selected `COVEN_HOME`. It does not create `<COVEN_HOME>/coven.sock`; run
`coven daemon status` to inspect the active endpoint. WSL2 remains a separate
Unix environment and uses its own Unix socket.

## Interactive UI

Bare `coven`, `coven chat`, and `coven tui` open the managed Coven interactive
UI powered by `coven-code`. On the first interactive run, Coven offers to
install the pinned engine if it is missing. The older in-process TUI is a
deprecated temporary compatibility fallback, enabled only with
`COVEN_LEGACY_TUI=1`, and will be removed.

## Verify

```powershell
coven --version
coven doctor
coven daemon restart
coven daemon status
cd C:\path\to\project
coven run codex "describe this repo"
coven sessions
```

If `coven doctor` reports a newly installed harness as missing, open a fresh
terminal and run it again so `PATH` is refreshed.

## Related

- [Windows install](/install/windows)
- [WSL2](/platforms/wsl2)
- [Troubleshooting](/TROUBLESHOOTING)
