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

The wrapper exposes the `coven` command and launches the native Windows binary
when the release package includes one for your platform. `coven doctor` is the
first verification step: it checks local state and reports whether supported
harness CLIs such as Codex, Claude Code, or GitHub Copilot CLI are available on
`PATH`.

Use native Windows and WSL2 as separate Coven environments. If you install Coven in PowerShell, install the harness CLIs in PowerShell too. If you install Coven inside WSL2, follow [WSL2 install](/install/wsl2) and keep the daemon state inside WSL.

## First run

From a project directory:

```powershell
coven
```

Bare `coven`, `coven chat`, and `coven tui` open the managed Coven interactive
UI powered by `coven-code`. On the first interactive run, Coven offers to
install the pinned engine if it is missing. The older in-process TUI is a
temporary compatibility fallback: explicitly set `COVEN_LEGACY_TUI=1` to use
it. It is deprecated and will be removed.

You can also use the explicit CLI flow:

```powershell
coven doctor
coven daemon start
coven run codex "fix the failing tests"
coven run claude "audit this branch" --think
coven sessions
```

Install and authenticate at least one harness CLI before expecting `coven run` to launch work. If `coven doctor` reports a missing harness, install that tool, open a new terminal so `PATH` is refreshed, and run `coven doctor` again.

Codex:

```powershell
npm install -g @openai/codex
codex login
```

Claude Code:

```powershell
npm install -g @anthropic-ai/claude-code
claude doctor
```

## Windows notes

- `coven doctor` should work in PowerShell even when the `HOME` environment variable is absent. Coven resolves its default store from `COVEN_HOME`, `HOME`, `USERPROFILE`, `HOMEDRIVE` + `HOMEPATH`, or the platform home directory.
- Keep `COVEN_HOME` on a local path owned by your Windows user when you override it.
- To override the store path in PowerShell, use:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven doctor
```

- Run Coven and your harness CLI from the same environment. A harness installed only inside WSL2 is not available to native Windows PowerShell unless you expose it separately.
- The legacy in-process TUI is only for temporary compatibility. Set
  `COVEN_LEGACY_TUI=1` explicitly if a legacy workflow requires it; do not use
  it as the default Windows interactive UI.

## Verification loop

```powershell
coven --version
coven doctor
coven daemon restart
coven daemon status
cd C:\path\to\project
coven run codex "describe this repo"
coven sessions
```

## Related

- [Get started with Coven](/GETTING-STARTED)
- [Install overview](/install/index)
- [Coven TUI](/start/coven-tui)
- [Troubleshooting](/TROUBLESHOOTING)
- [CLI reference](/reference/cli)
