---
summary: "Every Coven-specific environment variable."
description: "Reference for environment variables that change Coven CLI behavior, including color output, terminal capability detection, and state directory overrides."
read_when:
  - Looking up an env var
  - Disabling color output in CI or piped scripts
  - Forcing truecolor or 256-color rendering in the TUI
title: "Environment variables"
---

Coven reads a small set of environment variables to adapt its output to your terminal and shell. They all have safe defaults, so you only need to set them when you want to override the detected behavior.

## Color and terminal capabilities

The Coven CLI and TUI render with a brand-aligned palette and automatically downgrade to 256-color or no-color output when your terminal cannot display truecolor. Detection happens once per process and is cached for the lifetime of that command.

| Variable | Effect |
|---|---|
| `NO_COLOR` | When set to any non-empty value, Coven disables all ANSI color and style escapes. Empty values are treated as unset. Follows the [no-color.org](https://no-color.org) convention. |
| `COLORTERM` | When set to `truecolor` or `24bit`, Coven emits 24-bit RGB escapes. Any other value falls through to `TERM`-based detection. |
| `TERM` | Used as a fallback. Values ending in `-direct` enable truecolor, values ending in `-256color` enable 256-color rendering, `dumb` or unset disables color, and any other value enables 256-color rendering. |

Coven also disables color when standard output is not a TTY (for example, when you pipe `coven sessions` into another command or redirect it to a file). You do not need to set `NO_COLOR` for piped commands.

### Examples

Disable color in CI or log capture:

```bash
NO_COLOR=1 coven sessions
```

Force truecolor in a terminal that does not advertise it:

```bash
COLORTERM=truecolor coven tui
```

Force a plain 256-color render:

```bash
TERM=xterm-256color coven tui
```

## State and configuration

| Variable | Effect |
|---|---|
| `COVEN_HOME` | Overrides the default state directory (`~/.coven`). Use this to keep multiple Coven profiles on one machine, or to relocate state to a different disk. |

See [Paths and state directories](/help/paths) for the default layout on each operating system.
