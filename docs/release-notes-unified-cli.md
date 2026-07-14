# Coven — The Unified CLI

> Draft release notes for `@opencoven/cli`. Version stamped at release. Covers the unification work (one `coven` CLI end to end).

**One project. Any harness. Visible work — now behind a single command.**

`coven` is now the only thing you install and the only command you type. It runs your coding agent, records every session, and manages the engine for you. No second tool, no second install, no second state directory.

---

## TL;DR

```bash
npm install -g @opencoven/cli
coven
```

That's it. On first run, `coven` offers to download and install the Coven engine automatically, then drops you into the interactive UI. Everything else — sessions, search, auth, models, doctor — is one `coven <thing>` away.

---

## Highlights

### One install, one command
Installing `@opencoven/cli` is all you need. The Coven engine (the interactive agent runtime) is downloaded, checksum-verified, pinned, and kept up to date **by `coven` itself** — you never install or update it separately.

- `coven` → interactive UI (auto-installs the engine on first run)
- `coven "fix the failing login test"` → casts a free-text task
- `coven run <harness> "task"` → a recorded, project-scoped session (`codex`, `claude`, or the built-in `coven-code` engine)

### Your interactive sessions are now first-class
Interactive engine sessions register themselves in Coven's ledger, so the work you do in the UI is no longer invisible:

- `coven sessions` lists them alongside daemon-run sessions
- `coven sessions search <term>` searches **across** them — including the text of your interactive conversations
- `coven attach <id>` replays a session's recorded ledger

### One place for everything
- `coven doctor` — a single **Credentials** panel answers "am I logged in, to what, via what," plus engine health, version, and pin
- `coven --version` — shows the whole stack: `coven <v> (engine coven-code <v>, pinned <v>)`
- `coven auth` / `coven models` / `coven acp` — manage credentials, list models, or start the ACP server, all through `coven`
- `coven engine status | install | which` — manage the engine directly when you want to
- `coven code <anything>` — raw escape hatch straight to the engine

### One state root: `~/.coven`
Engine state now lives under `~/.coven/code/` instead of `~/.coven-code/`. If you have an existing `~/.coven-code/`, **it is migrated in place automatically** on first run under `coven`, with a compatibility symlink left behind so nothing breaks during the transition. A shared `~/.coven/settings.json` lets a few cross-tool defaults (model, theme, permission mode) live at the unified root.

### One brand
User-facing surfaces now say **Coven**. Running the engine binary directly still works, but prints a one-line hint pointing you at the supported `coven` CLI.

---

## Under the hood

- **Pinned, verified engine.** Each `coven` release pins an exact engine version and a SHA-256 for every release archive (`engine.lock`); installs fail closed on a checksum mismatch. A daily job proposes a pin-bump PR when a new engine ships.
- **A CI-enforced contract.** The exact CLI/stream/env surfaces `coven` relies on are specified and tested in both repos' CI, so an engine change that would break the unified CLI is caught before release.
- **Process boundary by design.** `coven` (MIT) drives the engine (GPL-3.0) as a separate, managed process — never linked. You get one seamless experience; the two projects keep their own licenses.

---

## Upgrading

- **New users:** `npm install -g @opencoven/cli`, then `coven`.
- **Existing engine users:** just start using `coven`. Your `~/.coven-code/` data migrates to `~/.coven/code/` automatically (with a symlink back for the transition); your auth, sessions, and settings come with it.
- **Non-interactive / CI:** set `COVEN_NO_AUTO_INSTALL=1` to skip the first-run install prompt, and `coven engine install` to provision the engine explicitly.

## Deprecations

- The standalone **`@opencoven/coven-code` npm package is deprecated** in favor of `@opencoven/cli`. The engine still ships as a binary via GitHub Releases (that's what `coven` installs); you no longer install it from npm yourself.

## Notes

- Platforms: macOS and Linux (Windows support is in progress).
- The engine binary is still named `coven-code` internally — that's deliberate and doesn't affect how you use `coven`.
