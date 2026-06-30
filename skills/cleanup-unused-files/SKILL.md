---
name: cleanup-unused-files
description: Aggressively clean disk space by removing caches, build artifacts, temporary folders, tool state, and other reproducible local junk from repos and developer machines. Use when the user asks to free space, nuke caches, wipe local build output, remove stale tool directories, clear package-manager caches, reset local developer state, or do a broad cleanup of bulky unused files.
---

# Cleanup Unused Files

Optimize for reclaiming disk space fast.

Prefer obvious, reproducible junk over careful semantic analysis.

## Default stance

- Target caches, build output, temp folders, and local tool state first.
- Prefer exact paths over broad globs.
- Be explicit about blast radius.
- Treat anything under source directories as higher risk unless it is known build output.
- If the cleanup touches machine-wide tool state or login/session data, call that out clearly before running it.

## Primary targets

Common high-yield cleanup targets include:

- project build output like `target/`, `dist/`, `.next/`, `coverage/`, `.turbo/`
- package-manager caches
- SDK/tool caches
- app caches under `~/Library/Caches`
- stale temp folders
- old logs, dumps, and exports
- backup/swap/temp files

Examples of the user's preferred cleanup style:

```bash
rm -rf ~/.codex
rm -rf ~/Library/Caches
rm -rf ~/.rustup
rm -rf ~/.bun
rm -rf ~/.npm
rm -rf ~/.lmstudio
rm -rf src-tauri/target
```

Also valid tool-native cleanup commands when they fit better:

```bash
cargo clean --manifest-path src-tauri/Cargo.toml
npm cache clean --force
brew cleanup --prune=all
rustup toolchain list | grep -v default | xargs -I{} rustup toolchain uninstall {}
```

## Workflow

### 1) Identify cleanup scope

Determine whether the user wants:

- project-only cleanup
- home-directory cache cleanup
- machine-wide developer reset
- maximum disk recovery regardless of re-download cost

If the request is vague, default to an audit of likely large cache directories first.

### 2) Prefer highest-yield directories

When disk recovery is the goal, focus on these before hunting tiny files:

- `~/Library/Caches`
- `~/.npm`
- `~/.bun`
- `~/.rustup`
- `~/.cargo`
- `~/.codex`
- `~/.lmstudio`
- repo-local `target/`, `dist/`, `.next/`, `coverage/`
- large temp/export folders under the workspace or home directory

### 3) Distinguish safe nukes from state resets

Usually safe and reproducible:

- build output
- download caches
- derived data
- package caches
- logs
- temp files

Higher impact, but still valid if explicitly requested:

- `~/.rustup` (removes installed toolchains)
- `~/.cargo` (removes installed cargo binaries, registry, and config-adjacent state)
- `~/.codex`
- `~/.bun`
- `~/.npm`
- `~/.lmstudio`
- all of `~/Library/Caches`

Call out that these may force reinstallation, re-downloads, reindexing, or loss of local session/tool state.

### 4) Execute with exact commands

Prefer short, direct commands with obvious targets.

Examples:

```bash
rm -rf ~/.codex
rm -rf ~/Library/Caches
rm -rf ~/.rustup
rm -rf ~/.bun
rm -rf ~/.npm
rm -rf ~/.lmstudio
rm -rf src-tauri/target
```

For repo build cleanup, exact-path deletion is fine.

For machine-wide cleanup, group commands by tool/system and explain briefly what each group resets.

Avoid fancy one-liners if they make the blast radius less legible.

### 5) Summarize outcome

Report:

- what paths were removed
- which ones were skipped
- likely consequences such as re-downloads or tool re-bootstrap
- any follow-up commands the user may need

## Decision rules

Delete immediately when:

- the user explicitly names the path
- the directory is clearly cache/build output/tool state
- the directory is reproducible and not the source of truth

Pause and confirm when:

- the path is broad and not explicitly requested
- the path may contain user-authored content
- the cleanup may remove credentials, custom models, local databases, or app data beyond cache
- the command would recurse through mixed-purpose directories

## Preferred style

Be blunt and practical.

When asked for a cleanup recipe, produce something like this:

```bash
# Rust / Tauri build output
rm -rf src-tauri/target

# Tool and package caches
rm -rf ~/.npm
rm -rf ~/.bun
rm -rf ~/.codex
rm -rf ~/.lmstudio

# macOS app caches
rm -rf ~/Library/Caches

# Rust toolchains and state
rm -rf ~/.rustup
```

Favor the biggest wins first.
