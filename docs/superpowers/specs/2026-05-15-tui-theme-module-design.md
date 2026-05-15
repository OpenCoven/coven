# TUI Theme Module — Design

**Status:** Approved — ready for implementation plan
**Date:** 2026-05-15
**Scope:** Phase 1 of a larger TUI structural-cleanup effort. Subsequent phases (file splitting, command extraction, UX polish) are out of scope here.

---

## Problem

The CLI ships three TUIs (chat, launcher, session browser) across two files, with a palette duplicated in incompatible formats and drifted from the brand spec:

- `crates/coven-cli/src/chat.rs` defines `PURPLE`, `GOLD`, `MOON`, `DIM_FG`, `SURFACE`, `SURFACE_LIGHT` as `ratatui::style::Color::Indexed(...)` constants.
- `crates/coven-cli/src/main.rs` defines `PURPLE`, `GOLD`, `ROSE`, `MOON` as raw ANSI escape strings (`"\x1b[38;5;141m"`).
- The two palettes overlap in name but use different rendering paths and slightly different semantics.
- `brand/ui/color-tokens.css` is the canonical brand palette (`#6E4BFF`, `#8A63FF`, `#A78BFF`, `#0A84FF`, `#FF3B30`, `#30D158`, plus text/surface variants). The TUI's `GOLD` (`#ffd700`) and `MOON` (`#87d7ff`) do not exist in brand; `PURPLE` is close but not identical.
- `docs/BRANDING-ADHERENCE.md:45` states: *"Future TUI/web app surfaces must import `brand/ui/color-tokens.css` and `brand/ui/typography.css` or mirror them in platform-native constants."*
- No `NO_COLOR` support; no truecolor detection; no graceful degradation; no respect for piped stdout.

This spec defines a single `theme` module that resolves all of the above.

## Non-goals

These are explicitly out of scope for Phase 1 and must not creep in:

- File splitting of `chat.rs` or `main.rs` into submodules (Phases 2 & 3).
- UX changes — help overlay, key bindings, focus model, layout (Phase 5).
- A `--color={auto,always,never}` CLI flag — env-var-only by decision.
- `FORCE_COLOR` / `CLICOLOR` / `CLICOLOR_FORCE` support.
- Light-terminal palette / `color-scheme: light`. Brand is dark-only.
- Windows legacy console (`enable_virtual_terminal_processing()`).
- High-contrast / accessibility variants.
- Wiring `DANGER` / `SUCCESS` to actual callsites — defined and tested only.

## Module layout

Single file, no new crate:

```
crates/coven-cli/src/theme.rs
```

`main.rs` adds `mod theme;` once. Two consumers: `chat.rs` (ratatui path) and `main.rs` (raw-ANSI path). Both import semantic constants from `theme::`.

A separate crate is rejected on YAGNI grounds — there is one binary today. Promote to a workspace crate when a second Rust surface (e.g. daemon TUI) needs it.

## Tokens

### Layer 1 — Brand tokens (mirror `brand/ui/color-tokens.css`)

```rust
pub mod brand {
    use super::Rgb;
    pub const PURPLE_1:    Rgb = Rgb { r: 0x6E, g: 0x4B, b: 0xFF };
    pub const PURPLE_2:    Rgb = Rgb { r: 0x8A, g: 0x63, b: 0xFF };
    pub const PURPLE_3:    Rgb = Rgb { r: 0xA7, g: 0x8B, b: 0xFF };
    pub const ACCENT_BLUE: Rgb = Rgb { r: 0x0A, g: 0x84, b: 0xFF };
    pub const DANGER:      Rgb = Rgb { r: 0xFF, g: 0x3B, b: 0x30 };
    pub const SUCCESS:     Rgb = Rgb { r: 0x30, g: 0xD1, b: 0x58 };
    pub const TEXT:        Rgb = Rgb { r: 0xF0, g: 0xF0, b: 0xF0 }; // rgba(255,255,255,.94) on black
    pub const TEXT_MUTED:  Rgb = Rgb { r: 0xA3, g: 0xA3, b: 0xA3 }; // .64
    pub const TEXT_FAINT:  Rgb = Rgb { r: 0x6B, g: 0x6B, b: 0x6B }; // .42
    pub const SURFACE_1:   Rgb = Rgb { r: 0x05, g: 0x05, b: 0x07 };
    pub const SURFACE_2:   Rgb = Rgb { r: 0x08, g: 0x08, b: 0x12 };
}
```

The text-variant RGBs are flatten-on-black of the canonical `rgba(...)` values: `round(channel * alpha)` with black background. Drift detection (see Tests) enforces this against the CSS.

### Layer 2 — Semantic tokens (what callsites use)

```rust
pub const PRIMARY:        Rgb = brand::PURPLE_3;
pub const PRIMARY_STRONG: Rgb = brand::PURPLE_2;
pub const AGENT_LABEL:    Rgb = brand::PURPLE_2;
pub const USER_LABEL:     Rgb = brand::PURPLE_1;
pub const HINT_KEY:       Rgb = brand::TEXT;
pub const HINT_LABEL:     Rgb = brand::TEXT_MUTED;
pub const FIELD_LABEL:    Rgb = brand::TEXT_MUTED;
pub const DANGER:         Rgb = brand::DANGER;
pub const SUCCESS:        Rgb = brand::SUCCESS;
pub const DIM:            Rgb = brand::TEXT_FAINT;
pub const SURFACE:        Rgb = brand::SURFACE_1;
pub const SURFACE_STRONG: Rgb = brand::SURFACE_2;
```

### Mapping rationale

The current TUI uses four colors with overlapping semantics. The strict-brand mapping collapses them onto brand tokens, using bold weight to preserve hierarchy where hue alone would collapse on 256-color terminals.

| Current | Used for | New semantic | New raw |
|---|---|---|---|
| `PURPLE` (`#af87ff`) | borders, scrollbar, system msgs, title, cancel text | `PRIMARY` | `PURPLE_3` |
| `GOLD` (`#ffd700`) on titles | "Coven TUI", "Coven quick start", help-overlay title | `PRIMARY_STRONG` (bold) | `PURPLE_2` |
| `GOLD` on agent rows | "◆ AgentName" prefix | `AGENT_LABEL` (bold) | `PURPLE_2` |
| `GOLD` on key hints | `↑↓`, `Enter`, `Esc`, `/help`, `Ctrl+C` | `HINT_KEY` (bold) | `TEXT` |
| `MOON` (`#87d7ff`) on user rows | "▶ You" prefix | `USER_LABEL` (bold) | `PURPLE_1` |
| `ROSE` (`#ffafd7`) | session-browser field names | `FIELD_LABEL` | `TEXT_MUTED` |
| `DIM_FG` | inactive items | `DIM` | `TEXT_FAINT` |
| `SURFACE`/`SURFACE_LIGHT` | block backgrounds | `SURFACE` / `SURFACE_STRONG` | `SURFACE_1` / `SURFACE_2` |

### Accepted tradeoffs

1. **User identity drops MOON's cool blue.** PURPLE_1 anchors user identity instead. The TUI becomes purple-dominant with weight-based hierarchy. The brand-compliant alternative — `ACCENT_BLUE` for user identity — was rejected because brand says "controlled accents: use sparingly," and user messages appear on every screen.

2. **256-color terminals compress the purples.** Verified via the `nearest_256` algorithm: PURPLE_1 → 63, PURPLE_2 → 99, PURPLE_3 → 141 — distinguishable on 256-color terms. The bold-weight differentiation is redundant safety, not load-bearing, on standard xterm-256 palettes.

3. **`GOLD` and `ROSE` disappear visually.** This is a deliberate brand alignment. Users on `main` today will see hint-bar keys go from amber to bold white, and session-browser field labels go from pink to muted gray.

## Terminal-mode detection

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TerminalMode { TrueColor, Indexed256, NoColor }

pub fn mode() -> TerminalMode;  // OnceLock-cached process-wide
```

### Resolution order

First match wins:

1. `NO_COLOR` set to a non-empty value → `NoColor` (per [no-color.org](https://no-color.org); empty-string treated as unset, matching `supports-color` and other ecosystem conventions).
2. Stdout is not a TTY (`std::io::IsTerminal`) → `NoColor`.
3. `COLORTERM=truecolor` or `COLORTERM=24bit` → `TrueColor`.
4. `TERM` matches `*-direct` → `TrueColor`.
5. `TERM` matches `*-256color` → `Indexed256`.
6. `TERM=dumb` or `TERM` unset → `NoColor`.
7. Default → `Indexed256`.

### Implementation shape

```rust
use std::sync::OnceLock;
static MODE: OnceLock<TerminalMode> = OnceLock::new();

pub fn mode() -> TerminalMode {
    *MODE.get_or_init(detect_mode)
}

struct EnvInputs<'a> {
    no_color:      Option<&'a str>,
    colorterm:     Option<&'a str>,
    term:          Option<&'a str>,
    stdout_is_tty: bool,
}

fn detect_mode() -> TerminalMode {
    detect_mode_from(EnvInputs {
        no_color:      std::env::var("NO_COLOR").ok().as_deref(),
        colorterm:     std::env::var("COLORTERM").ok().as_deref(),
        term:          std::env::var("TERM").ok().as_deref(),
        stdout_is_tty: std::io::IsTerminal::is_terminal(&std::io::stdout()),
    })
}

fn detect_mode_from(e: EnvInputs<'_>) -> TerminalMode {
    if e.no_color.map(|v| !v.is_empty()).unwrap_or(false) { return TerminalMode::NoColor; }
    if !e.stdout_is_tty                                    { return TerminalMode::NoColor; }
    match e.colorterm {
        Some("truecolor") | Some("24bit") => return TerminalMode::TrueColor,
        _ => {}
    }
    match e.term {
        Some(t) if t.ends_with("-direct")    => TerminalMode::TrueColor,
        Some(t) if t.ends_with("-256color")  => TerminalMode::Indexed256,
        Some("dumb") | None                  => TerminalMode::NoColor,
        Some(_)                              => TerminalMode::Indexed256,
    }
}
```

The split between `detect_mode()` (reads real env) and `detect_mode_from(EnvInputs)` (pure) is what makes the mode-detection table tests possible without mutating process env.

## Output adapters

```rust
pub fn ratatui_color(c: Rgb) -> ratatui::style::Color;
pub fn ratatui_style(c: Rgb) -> ratatui::style::Style;   // sugar: Style::default().fg(...)

pub fn fg(c: Rgb)    -> Fg;     // Display impl writes 38;2 | 38;5 | ""
pub fn bg(c: Rgb)    -> Bg;
pub fn reset()       -> Reset;  // Display impl writes "\x1b[0m" or ""

pub struct Fg(Rgb);
pub struct Bg(Rgb);
pub struct Reset;
```

### Per-mode behavior

| Function | `TrueColor` | `Indexed256` | `NoColor` |
|---|---|---|---|
| `ratatui_color(c)` | `Color::Rgb(r, g, b)` | `Color::Indexed(nearest_256(c))` | `Color::Reset` |
| `Fg(c)` Display | `"\x1b[38;2;R;G;Bm"` | `"\x1b[38;5;Nm"` | `""` |
| `Bg(c)` Display | `"\x1b[48;2;R;G;Bm"` | `"\x1b[48;5;Nm"` | `""` |
| `Reset` Display | `"\x1b[0m"` | `"\x1b[0m"` | `""` |

In `NoColor`, `ratatui_color` returns `Color::Reset` — ratatui falls back to the terminal default. We never force a foreground color when the user has opted out.

### Why `Display` wrappers and not `String`

- Zero-alloc: format machinery writes directly into the destination buffer; no intermediate allocation per call.
- Preserves the existing `println!("{purple}…{reset}")` pattern in `main.rs` — callsites bind once at the top of the function.
- The current `ansi(color_enabled, code)` helper at `main.rs:1201` and its `color_enabled: bool` plumbing is deleted; `theme::mode()` is the single source of truth.

### 256-color downgrade algorithm

Inlined, no new dependency. ~25 lines:

```rust
fn nearest_256(c: Rgb) -> u8 {
    let to_6 = |v: u8| -> u8 {
        if v < 48 { 0 }
        else if v < 115 { 1 }
        else { ((v as u16 - 35) / 40) as u8 }
    };
    let cube_idx = 16 + 36 * to_6(c.r) + 6 * to_6(c.g) + to_6(c.b);

    let gray = ((c.r as u16 + c.g as u16 + c.b as u16) / 3) as u8;
    let gray_idx = if gray < 8 { 16 }
                   else if gray > 247 { 231 }
                   else { 232 + (gray - 8) / 10 };

    if dist2(c, palette_rgb(cube_idx)) <= dist2(c, palette_rgb(gray_idx))
        { cube_idx } else { gray_idx }
}
```

`palette_rgb(u8) -> Rgb` returns the canonical xterm-256 RGB for an indexed value; `dist2` is squared euclidean distance.

## Callsite migration

### `chat.rs`

- Delete constants at lines 26–31.
- Add `use crate::theme::{self, PRIMARY, PRIMARY_STRONG, AGENT_LABEL, USER_LABEL, HINT_KEY, HINT_LABEL, DIM, SURFACE, SURFACE_STRONG};`
- Replace `Style::default().fg(PURPLE)` with `theme::ratatui_style(PRIMARY)` (and similar for the other constants).
- Replace `MessageRole::User => (Style::default().fg(MOON).bold(), ...)` with `theme::ratatui_style(USER_LABEL).bold()`.
- Replace `MessageRole::Agent => (Style::default().fg(GOLD).bold(), ...)` with `theme::ratatui_style(AGENT_LABEL).bold()`.
- Replace key-hint `GOLD` styling with `theme::ratatui_style(HINT_KEY).bold()`; the surrounding prose (which is currently default-styled) gets `theme::ratatui_style(HINT_LABEL)`.
- ~30 styling sites touched.

### `main.rs`

- Delete constants at lines 252–257.
- Replace `const PURPLE: &str = "..."` callsites by binding `let primary = theme::fg(theme::PRIMARY); let reset = theme::reset();` once per function. Format-string bodies stay identical: `println!("{primary}...{reset}")`.
- Delete the `ansi(color_enabled, code)` helper (line 1201).
- Remove the `color_enabled: bool` parameter from `render_session_browser_frame_with_color` (line 876) and `render_magical_tui_frame_with_color_and_width` (line 1046), plus their callers. The decision moves to `theme::mode()`.
- ~6 functions touched at the top; ~2 function signatures simplified.

### Behavior change worth flagging

Today `main.rs` has callers that explicitly pass `color_enabled = false` for raw-terminal output paths. In the new world that's expressed as "stdout is not a TTY → `NoColor`." The two should be equivalent in practice — the raw-terminal paths are reached when stdout is captured for non-interactive output. The migration must verify no caller depended on `color_enabled` diverging from TTY detection.

## Tests

All tests in `#[cfg(test)] mod tests` inside `theme.rs`. No external infra.

### 1. Brand drift detection

```rust
#[test]
fn brand_tokens_mirror_color_tokens_css() {
    let css = include_str!("../../../brand/ui/color-tokens.css");
    let vars = parse_css_vars(css);

    assert_eq!(brand::PURPLE_1,    rgb_from_hex(&vars["--oc-purple-1"]));
    assert_eq!(brand::PURPLE_2,    rgb_from_hex(&vars["--oc-purple-2"]));
    assert_eq!(brand::PURPLE_3,    rgb_from_hex(&vars["--oc-purple-3"]));
    assert_eq!(brand::ACCENT_BLUE, rgb_from_hex(&vars["--oc-accent-blue"]));
    assert_eq!(brand::DANGER,      rgb_from_hex(&vars["--oc-danger"]));
    assert_eq!(brand::SUCCESS,     rgb_from_hex(&vars["--oc-success"]));
    assert_eq!(brand::SURFACE_1,   rgb_from_hex(&vars["--oc-surface-1"]));
    assert_eq!(brand::SURFACE_2,   rgb_from_hex(&vars["--oc-surface-2"]));

    assert_eq!(brand::TEXT,       flatten_on_black(&vars["--oc-text"]));
    assert_eq!(brand::TEXT_MUTED, flatten_on_black(&vars["--oc-text-muted"]));
    assert_eq!(brand::TEXT_FAINT, flatten_on_black(&vars["--oc-text-faint"]));
}
```

`parse_css_vars` is a purpose-built parser (~30 lines) handling only `:root { --name: #hex; --name: rgba(r, g, b, a); }`. `flatten_on_black(rgba(255,255,255,0.94))` yields `Rgb { 240, 240, 240 }` via `round(channel * alpha)`.

`include_str!` resolves at compile time relative to `theme.rs`; if the CSS file moves, the build breaks visibly.

### 2. `detect_mode_from` — table-driven

Eleven rows, each a `#[test]` or one parametric test:

| `NO_COLOR` | `COLORTERM` | `TERM` | TTY | Expected |
|---|---|---|---|---|
| `Some("1")` | `Some("truecolor")` | `Some("xterm-256color")` | true | `NoColor` |
| `Some("")` | `Some("truecolor")` | `Some("xterm-256color")` | true | `TrueColor` |
| `None` | `Some("truecolor")` | `Some("xterm-256color")` | true | `TrueColor` |
| `None` | `Some("24bit")` | `Some("xterm-256color")` | true | `TrueColor` |
| `None` | `None` | `Some("xterm-direct")` | true | `TrueColor` |
| `None` | `None` | `Some("xterm-256color")` | true | `Indexed256` |
| `None` | `None` | `Some("xterm")` | true | `Indexed256` |
| `None` | `None` | `Some("dumb")` | true | `NoColor` |
| `None` | `None` | `None` | true | `NoColor` |
| `None` | `Some("truecolor")` | `Some("xterm-256color")` | false | `NoColor` |
| `Some("1")` | `None` | `None` | false | `NoColor` |

### 3. `nearest_256` — pinned table

```rust
#[test]
fn nearest_256_brand_tokens() {
    assert_eq!(nearest_256(brand::PURPLE_3),         141);
    assert_eq!(nearest_256(brand::PURPLE_2),          99);
    assert_eq!(nearest_256(brand::PURPLE_1),          63);
    assert_eq!(nearest_256(brand::ACCENT_BLUE),       33);
    assert_eq!(nearest_256(brand::DANGER),           203);
    assert_eq!(nearest_256(brand::SUCCESS),           77);
    assert_eq!(nearest_256(Rgb {r:0,   g:0,   b:0  }),  16);
    assert_eq!(nearest_256(Rgb {r:255, g:255, b:255}), 231);
    assert_eq!(nearest_256(Rgb {r:128, g:128, b:128}), 244);
    assert_eq!(nearest_256(Rgb {r:248, g:248, b:248}), 231);
}
```

### 4. Output adapter Display

`Fg`, `Bg`, `Reset` each get a `with_mode(rgb, mode)` constructor (pure) so test output can be asserted without touching the cached `mode()`:

```rust
#[test] fn fg_emits_truecolor() {
    assert_eq!(format!("{}", Fg::with_mode(brand::PURPLE_3, TerminalMode::TrueColor)),
               "\x1b[38;2;167;139;255m");
}
#[test] fn fg_emits_indexed_256() {
    assert_eq!(format!("{}", Fg::with_mode(brand::PURPLE_3, TerminalMode::Indexed256)),
               "\x1b[38;5;141m");
}
#[test] fn fg_emits_nothing_in_no_color() {
    assert_eq!(format!("{}", Fg::with_mode(brand::PURPLE_3, TerminalMode::NoColor)), "");
}
#[test] fn reset_is_empty_in_no_color() {
    assert_eq!(format!("{}", Reset::with_mode(TerminalMode::NoColor)), "");
}
#[test] fn reset_emits_sgr_zero_otherwise() {
    assert_eq!(format!("{}", Reset::with_mode(TerminalMode::TrueColor)), "\x1b[0m");
}
```

### 5. `ratatui_color` mode behavior

```rust
#[test] fn ratatui_color_returns_reset_in_no_color() {
    assert_eq!(ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::NoColor),
               ratatui::style::Color::Reset);
}
#[test] fn ratatui_color_returns_rgb_in_truecolor() {
    assert_eq!(ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::TrueColor),
               ratatui::style::Color::Rgb(0xA7, 0x8B, 0xFF));
}
```

### What is deliberately not tested

- **Visual snapshots.** Phase 1 changes colors only; a snapshot would just pin the new mapping. Snapshots arrive in Phase 5.
- **Real-terminal integration.** `IsTerminal` is hard to fake without a PTY. The split-pure-function design makes integration tests unnecessary — the rule logic is fully covered by the table.
- **Cross-platform behavior.** No Windows-console-specific tests; we rely on crossterm's existing VT enablement.

## Estimated diff scale

| File | Lines |
|---|---|
| `theme.rs` (new, incl. tests) | ~200 |
| `chat.rs` | ~40 (palette deleted; ~30 Style sites updated) |
| `main.rs` | ~80 (palette deleted; ~6 function tops adjusted; `ansi()` + `color_enabled` removed) |
| `main.rs` (`mod theme;`) | 1 |

No public crate API changes — `coven-cli` is a binary.

## Acceptance criteria

This phase is complete when:

1. `crates/coven-cli/src/theme.rs` exists and implements the module surface in this spec.
2. No `Color::Indexed(...)` palette constants remain in `chat.rs`.
3. No `"\x1b[38;5;..."` literal palette constants remain in `main.rs`.
4. The `ansi(color_enabled, ...)` helper and `color_enabled: bool` parameters are gone.
5. All tests in §Tests pass.
6. `NO_COLOR=1 coven sessions` produces no escape codes in its output (manual check).
7. `coven sessions | cat` produces no escape codes (manual check — TTY detection).
8. On a truecolor terminal, the three role-purples are visibly distinct.
9. The drift test passes against the current `brand/ui/color-tokens.css`.
