---
summary: "Install Coven on native Windows."
read_when:
  - Installing on Windows
title: "Windows install"
description: "Install Coven on Windows: how to set up the wrapper, native daemon binary, COVEN_HOME, and harness CLIs on a Windows host or WSL2 environment."
---

# Windows install

Install the wrapper globally from PowerShell, Windows Terminal, or any terminal that can run Node.js packages:

```powershell
npm install -g @opencoven/cli
coven doctor
```

The wrapper exposes the `coven` command and launches the native Windows binary when the release package includes one for your platform. `coven doctor` is the first verification step: it checks local state and reports whether supported harness CLIs such as Codex or Claude Code are available on `PATH`.

## First run

From a project directory:

```powershell
coven
```

The default command opens the prompt-first TUI. You can also use the explicit CLI flow:

```powershell
coven doctor
coven daemon start
coven run codex "fix the failing tests"
coven sessions
```

Install and authenticate at least one harness CLI before expecting `coven run` to launch work. If `coven doctor` reports a missing harness, install that tool, open a new terminal so `PATH` is refreshed, and run `coven doctor` again.

## Windows notes

- `coven doctor` should work in PowerShell even when the `HOME` environment variable is absent. Coven resolves its default store from `COVEN_HOME`, `HOME`, `USERPROFILE`, `HOMEDRIVE` + `HOMEPATH`, or the platform home directory.
- Keep `COVEN_HOME` on a local path owned by your Windows user when you override it.
- To override the store path in PowerShell, use:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven doctor
```

- Run Coven and your harness CLI from the same environment. A harness installed only inside WSL2 is not available to native Windows PowerShell unless you expose it separately.
- If terminal input behaves oddly, update to the latest wrapper and run `coven tui` again. The Windows TUI filters key-press events so typed characters, arrows, and Enter should be handled once.

## Related

- [Get started with Coven](/GETTING-STARTED)
- [Coven TUI](/start/coven-tui)
- [Troubleshooting](/TROUBLESHOOTING)
- [CLI reference](/reference/cli)
