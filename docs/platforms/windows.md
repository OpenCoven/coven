---
summary: "Coven on Windows — caveats and supported flows."
read_when:
  - Operating on Windows
title: "Windows"
description: "Coven on Windows: native wrapper plus Linux daemon under WSL2, and how COVEN_HOME and the local socket connect Windows clients to harnesses."
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
