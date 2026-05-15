# TUI Theme Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the duplicated, brand-drifted color palettes in `chat.rs` and `main.rs` with a single `theme` module sourced from `brand/ui/color-tokens.css`, supporting `NO_COLOR` and truecolor/256-color/no-color graceful degradation.

**Architecture:** One new file `crates/coven-cli/src/theme.rs` containing brand-mirrored raw tokens, semantic tokens callsites import, an `OnceLock`-cached `TerminalMode` resolved from env (`NO_COLOR` → TTY → `COLORTERM` → `TERM`), and adapters that emit ratatui `Color` / `Style` or Display-based ANSI escapes. `chat.rs` and `main.rs` lose their palette constants and consume the module.

**Tech Stack:** Rust 1.70+ (uses `std::io::IsTerminal`), ratatui 0.30, crossterm 0.29. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-05-15-tui-theme-module-design.md`](../specs/2026-05-15-tui-theme-module-design.md)

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `crates/coven-cli/src/theme.rs` | **Create** | All palette, mode detection, output adapters, tests |
| `crates/coven-cli/src/main.rs` | **Modify** | Declare `mod theme;` (line ~32); delete palette consts (lines 252–257); delete `ansi(color_enabled, code)` helper (line ~1201); update render fns that took `color_enabled` |
| `crates/coven-cli/src/chat.rs` | **Modify** | Delete palette consts (lines 26–31); replace ~30 `Style::default().fg(X)` callsites |

No other files change.

---

## Task 1: Module skeleton

**Files:**
- Create: `crates/coven-cli/src/theme.rs`
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Create theme.rs with just the Rgb type**

Write to `crates/coven-cli/src/theme.rs`:

```rust
//! Brand-aligned palette, terminal-mode detection, and output adapters for
//! both the ratatui-based chat TUI and the raw-ANSI launcher/session browser.
//!
//! Tokens mirror `brand/ui/color-tokens.css` and are enforced by the drift
//! test in this module.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_is_copy_and_eq() {
        let a = Rgb { r: 1, g: 2, b: 3 };
        let b = a;
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Wire it into the crate**

Edit `crates/coven-cli/src/main.rs`. Existing `mod` declarations are alphabetical, ending at lines 31–32:

```rust
mod store;
mod verification;
```

Insert `mod theme;` between them (alphabetically):

```rust
mod store;
mod theme;
mod verification;
```

- [ ] **Step 3: Verify the crate still builds**

Run: `cargo build -p coven-cli`
Expected: builds cleanly (one warning about unused `theme` module is acceptable).

- [ ] **Step 4: Run the new test**

Run: `cargo test -p coven-cli theme::tests::rgb_is_copy_and_eq`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs crates/coven-cli/src/main.rs
git commit -m "feat(theme): add module skeleton with Rgb type"
```

---

## Task 2: Brand-token mirror with drift detection

This is the load-bearing test: it parses `brand/ui/color-tokens.css` at test time and asserts every brand constant matches. Outside-in TDD — write the test first, fill in helpers and constants until it passes.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write the failing drift test**

Append inside `mod tests { ... }` in `theme.rs`:

```rust
    #[test]
    fn brand_tokens_mirror_color_tokens_css() {
        let css = include_str!("../../../brand/ui/color-tokens.css");
        let vars = parse_css_vars(css);

        assert_eq!(brand::PURPLE_1,    rgb_from_hex(&vars["--oc-purple-1"]),    "--oc-purple-1");
        assert_eq!(brand::PURPLE_2,    rgb_from_hex(&vars["--oc-purple-2"]),    "--oc-purple-2");
        assert_eq!(brand::PURPLE_3,    rgb_from_hex(&vars["--oc-purple-3"]),    "--oc-purple-3");
        assert_eq!(brand::ACCENT_BLUE, rgb_from_hex(&vars["--oc-accent-blue"]), "--oc-accent-blue");
        assert_eq!(brand::DANGER,      rgb_from_hex(&vars["--oc-danger"]),      "--oc-danger");
        assert_eq!(brand::SUCCESS,     rgb_from_hex(&vars["--oc-success"]),     "--oc-success");
        assert_eq!(brand::SURFACE_1,   rgb_from_hex(&vars["--oc-surface-1"]),   "--oc-surface-1");
        assert_eq!(brand::SURFACE_2,   rgb_from_hex(&vars["--oc-surface-2"]),   "--oc-surface-2");

        assert_eq!(brand::TEXT,       flatten_on_black(&vars["--oc-text"]),       "--oc-text");
        assert_eq!(brand::TEXT_MUTED, flatten_on_black(&vars["--oc-text-muted"]), "--oc-text-muted");
        assert_eq!(brand::TEXT_FAINT, flatten_on_black(&vars["--oc-text-faint"]), "--oc-text-faint");
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: errors for unresolved `brand`, `parse_css_vars`, `rgb_from_hex`, `flatten_on_black`.

- [ ] **Step 3: Add the brand module above `mod tests`**

Insert into `theme.rs` between the `Rgb` struct and `#[cfg(test)] mod tests`:

```rust
/// Raw brand tokens, mirroring `brand/ui/color-tokens.css`.
/// Enforced by the `brand_tokens_mirror_color_tokens_css` test.
pub mod brand {
    use super::Rgb;
    pub const PURPLE_1:    Rgb = Rgb { r: 0x6E, g: 0x4B, b: 0xFF };
    pub const PURPLE_2:    Rgb = Rgb { r: 0x8A, g: 0x63, b: 0xFF };
    pub const PURPLE_3:    Rgb = Rgb { r: 0xA7, g: 0x8B, b: 0xFF };
    pub const ACCENT_BLUE: Rgb = Rgb { r: 0x0A, g: 0x84, b: 0xFF };
    pub const DANGER:      Rgb = Rgb { r: 0xFF, g: 0x3B, b: 0x30 };
    pub const SUCCESS:     Rgb = Rgb { r: 0x30, g: 0xD1, b: 0x58 };
    /// rgba(255, 255, 255, 0.94) on black = round(255 * 0.94) = 240
    pub const TEXT:        Rgb = Rgb { r: 0xF0, g: 0xF0, b: 0xF0 };
    /// rgba(255, 255, 255, 0.64) on black = round(255 * 0.64) = 163
    pub const TEXT_MUTED:  Rgb = Rgb { r: 0xA3, g: 0xA3, b: 0xA3 };
    /// rgba(255, 255, 255, 0.42) on black = round(255 * 0.42) = 107
    pub const TEXT_FAINT:  Rgb = Rgb { r: 0x6B, g: 0x6B, b: 0x6B };
    pub const SURFACE_1:   Rgb = Rgb { r: 0x05, g: 0x05, b: 0x07 };
    pub const SURFACE_2:   Rgb = Rgb { r: 0x08, g: 0x08, b: 0x12 };
}
```

- [ ] **Step 4: Add the test helpers inside `mod tests`**

Inside `mod tests` (above the tests), add:

```rust
    use std::collections::HashMap;

    fn rgb_from_hex(hex: &str) -> Rgb {
        let s = hex.trim().trim_start_matches('#');
        assert_eq!(s.len(), 6, "expected 6-char hex, got {hex:?}");
        let r = u8::from_str_radix(&s[0..2], 16).unwrap();
        let g = u8::from_str_radix(&s[2..4], 16).unwrap();
        let b = u8::from_str_radix(&s[4..6], 16).unwrap();
        Rgb { r, g, b }
    }

    /// Parse `rgba(r, g, b, a)` and flatten on black: each channel becomes round(channel * a).
    fn flatten_on_black(rgba: &str) -> Rgb {
        let inner = rgba
            .trim()
            .strip_prefix("rgba(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or_else(|| panic!("expected rgba(...), got {rgba:?}"));
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        assert_eq!(parts.len(), 4, "expected 4 components in {rgba:?}");
        let r: u16 = parts[0].parse().unwrap();
        let g: u16 = parts[1].parse().unwrap();
        let b: u16 = parts[2].parse().unwrap();
        let a: f64 = parts[3].parse().unwrap();
        let flat = |c: u16| (c as f64 * a).round() as u8;
        Rgb { r: flat(r), g: flat(g), b: flat(b) }
    }

    /// Tiny purpose-built parser for `:root { --name: value; ... }`. Ignores everything else.
    fn parse_css_vars(css: &str) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for line in css.lines() {
            let line = line.trim();
            if !line.starts_with("--") {
                continue;
            }
            // Strip trailing comment if any.
            let line = line.split("/*").next().unwrap().trim();
            let line = line.trim_end_matches(';').trim();
            if let Some((name, value)) = line.split_once(':') {
                out.insert(name.trim().to_string(), value.trim().to_string());
            }
        }
        out
    }
```

- [ ] **Step 5: Verify the drift test passes**

Run: `cargo test -p coven-cli theme::tests::brand_tokens_mirror_color_tokens_css`
Expected: 1 passed. If it fails, the `assert_eq!` diff shows which constant disagrees with the CSS — fix the constant value, not the CSS.

- [ ] **Step 6: Run all module tests**

Run: `cargo test -p coven-cli theme::`
Expected: 2 passed (rgb_is_copy_and_eq + brand_tokens_mirror_color_tokens_css).

- [ ] **Step 7: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): mirror brand color tokens with drift detection"
```

---

## Task 3: Semantic tokens

Thin layer on top of brand tokens. What every callsite imports.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
    #[test]
    fn semantic_tokens_resolve_to_brand_tokens() {
        assert_eq!(PRIMARY,        brand::PURPLE_3);
        assert_eq!(PRIMARY_STRONG, brand::PURPLE_2);
        assert_eq!(AGENT_LABEL,    brand::PURPLE_2);
        assert_eq!(USER_LABEL,     brand::PURPLE_1);
        assert_eq!(HINT_KEY,       brand::TEXT);
        assert_eq!(HINT_LABEL,     brand::TEXT_MUTED);
        assert_eq!(FIELD_LABEL,    brand::TEXT_MUTED);
        assert_eq!(DANGER,         brand::DANGER);
        assert_eq!(SUCCESS,        brand::SUCCESS);
        assert_eq!(DIM,            brand::TEXT_FAINT);
        assert_eq!(SURFACE,        brand::SURFACE_1);
        assert_eq!(SURFACE_STRONG, brand::SURFACE_2);
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: errors for unresolved `PRIMARY`, `PRIMARY_STRONG`, etc.

- [ ] **Step 3: Add semantic constants**

In `theme.rs`, immediately after the `brand` module (before `mod tests`), add:

```rust
// ── Semantic tokens (what callsites import) ──

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

- [ ] **Step 4: Verify the test passes**

Run: `cargo test -p coven-cli theme::tests::semantic_tokens_resolve_to_brand_tokens`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): add semantic token layer"
```

---

## Task 4: TerminalMode enum and `detect_mode_from`

Pure rule resolution, table-driven tests. No env reads here yet.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write the failing table test**

Append inside `mod tests`:

```rust
    #[test]
    fn detect_mode_from_table() {
        use TerminalMode::*;
        let cases: &[(EnvInputs<'_>, TerminalMode)] = &[
            // NO_COLOR=1 always wins, even with truecolor and TTY
            (EnvInputs { no_color: Some("1"),  colorterm: Some("truecolor"), term: Some("xterm-256color"), stdout_is_tty: true  }, NoColor),
            // Empty-string NO_COLOR treated as unset
            (EnvInputs { no_color: Some(""),   colorterm: Some("truecolor"), term: Some("xterm-256color"), stdout_is_tty: true  }, TrueColor),
            // COLORTERM=truecolor wins on a TTY
            (EnvInputs { no_color: None,       colorterm: Some("truecolor"), term: Some("xterm-256color"), stdout_is_tty: true  }, TrueColor),
            // COLORTERM=24bit also yields truecolor
            (EnvInputs { no_color: None,       colorterm: Some("24bit"),     term: Some("xterm-256color"), stdout_is_tty: true  }, TrueColor),
            // TERM=*-direct yields truecolor even without COLORTERM
            (EnvInputs { no_color: None,       colorterm: None,              term: Some("xterm-direct"),   stdout_is_tty: true  }, TrueColor),
            // TERM=*-256color yields indexed 256
            (EnvInputs { no_color: None,       colorterm: None,              term: Some("xterm-256color"), stdout_is_tty: true  }, Indexed256),
            // Plain xterm: default to indexed 256
            (EnvInputs { no_color: None,       colorterm: None,              term: Some("xterm"),          stdout_is_tty: true  }, Indexed256),
            // TERM=dumb forces no color
            (EnvInputs { no_color: None,       colorterm: None,              term: Some("dumb"),           stdout_is_tty: true  }, NoColor),
            // TERM unset forces no color
            (EnvInputs { no_color: None,       colorterm: None,              term: None,                   stdout_is_tty: true  }, NoColor),
            // Piped stdout always disables color, regardless of env
            (EnvInputs { no_color: None,       colorterm: Some("truecolor"), term: Some("xterm-256color"), stdout_is_tty: false }, NoColor),
            (EnvInputs { no_color: Some("1"),  colorterm: None,              term: None,                   stdout_is_tty: false }, NoColor),
        ];
        for (i, (inputs, expected)) in cases.iter().enumerate() {
            assert_eq!(
                detect_mode_from(*inputs),
                *expected,
                "row {i}: {inputs:?}",
            );
        }
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: errors for unresolved `TerminalMode`, `EnvInputs`, `detect_mode_from`.

- [ ] **Step 3: Add the type and pure function**

In `theme.rs`, after the semantic-token block (still before `mod tests`):

```rust
// ── Terminal-mode detection ──

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TerminalMode {
    TrueColor,
    Indexed256,
    NoColor,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct EnvInputs<'a> {
    pub no_color:      Option<&'a str>,
    pub colorterm:     Option<&'a str>,
    pub term:          Option<&'a str>,
    pub stdout_is_tty: bool,
}

pub(crate) fn detect_mode_from(e: EnvInputs<'_>) -> TerminalMode {
    // 1. NO_COLOR set to non-empty value always wins (per no-color.org;
    //    empty-string treated as unset per supports-color convention).
    if e.no_color.map(|v| !v.is_empty()).unwrap_or(false) {
        return TerminalMode::NoColor;
    }
    // 2. Piped/redirected stdout — never emit escapes.
    if !e.stdout_is_tty {
        return TerminalMode::NoColor;
    }
    // 3. Explicit truecolor declaration.
    match e.colorterm {
        Some("truecolor") | Some("24bit") => return TerminalMode::TrueColor,
        _ => {}
    }
    // 4. TERM-based fallback.
    match e.term {
        Some(t) if t.ends_with("-direct")   => TerminalMode::TrueColor,
        Some(t) if t.ends_with("-256color") => TerminalMode::Indexed256,
        Some("dumb") | None                 => TerminalMode::NoColor,
        Some(_)                             => TerminalMode::Indexed256,
    }
}
```

- [ ] **Step 4: Verify the table test passes**

Run: `cargo test -p coven-cli theme::tests::detect_mode_from_table`
Expected: 1 passed (11 cases inside).

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): add TerminalMode and pure detect_mode_from"
```

---

## Task 5: Cached `mode()` and env-reading `detect_mode`

Bridge from pure rules to real process env.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write a smoke test**

Append inside `mod tests`:

```rust
    #[test]
    fn mode_returns_a_value_and_caches() {
        let first = mode();
        let second = mode();
        assert_eq!(first, second, "mode() must return the same value on repeated calls");
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: error for unresolved `mode`.

- [ ] **Step 3: Implement `mode()` and `detect_mode()`**

In `theme.rs`, immediately after `detect_mode_from`:

```rust
use std::sync::OnceLock;

static MODE: OnceLock<TerminalMode> = OnceLock::new();

/// Resolve the terminal mode for this process. Cached on first call.
pub fn mode() -> TerminalMode {
    *MODE.get_or_init(detect_mode)
}

fn detect_mode() -> TerminalMode {
    use std::io::IsTerminal;
    let no_color  = std::env::var("NO_COLOR").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    let term      = std::env::var("TERM").ok();
    detect_mode_from(EnvInputs {
        no_color:      no_color.as_deref(),
        colorterm:     colorterm.as_deref(),
        term:          term.as_deref(),
        stdout_is_tty: std::io::stdout().is_terminal(),
    })
}
```

- [ ] **Step 4: Verify the test passes**

Run: `cargo test -p coven-cli theme::tests::mode_returns_a_value_and_caches`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): add cached mode() and detect_mode() reading env"
```

---

## Task 6: `nearest_256` downgrade

Standard xterm-256 cube + grayscale-ramp algorithm. Pinned table test.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write the failing pinned-table test**

Append inside `mod tests`:

```rust
    #[test]
    fn nearest_256_brand_tokens() {
        assert_eq!(nearest_256(brand::PURPLE_3),    141);
        assert_eq!(nearest_256(brand::PURPLE_2),     99);
        assert_eq!(nearest_256(brand::PURPLE_1),     63);
        assert_eq!(nearest_256(brand::ACCENT_BLUE),  33);
        assert_eq!(nearest_256(brand::DANGER),      203);
        assert_eq!(nearest_256(brand::SUCCESS),      77);
        assert_eq!(nearest_256(Rgb { r: 0,   g: 0,   b: 0   }),  16);
        assert_eq!(nearest_256(Rgb { r: 255, g: 255, b: 255 }), 231);
        assert_eq!(nearest_256(Rgb { r: 128, g: 128, b: 128 }), 244);
        assert_eq!(nearest_256(Rgb { r: 248, g: 248, b: 248 }), 231);
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: error for unresolved `nearest_256`.

- [ ] **Step 3: Implement the algorithm**

In `theme.rs`, after the `mode()` block:

```rust
// ── 256-color downgrade ──

/// Round one 0..=255 channel into the 6-step xterm cube (0, 95, 135, 175, 215, 255).
fn channel_to_cube_step(v: u8) -> u8 {
    if v < 48 {
        0
    } else if v < 115 {
        1
    } else {
        ((v as u16 - 35) / 40) as u8
    }
}

/// xterm-256 palette RGB for indices 16..=255 (cube 16..=231, grayscale 232..=255).
fn palette_rgb(idx: u8) -> Rgb {
    if idx < 16 {
        // Lower 16 are terminal-defined; we never produce them, but return black as a sentinel.
        return Rgb { r: 0, g: 0, b: 0 };
    }
    if idx >= 232 {
        let v = 8 + (idx - 232) as u16 * 10;
        let v = v.min(255) as u8;
        return Rgb { r: v, g: v, b: v };
    }
    let levels: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let n = idx - 16;
    let r = levels[(n / 36) as usize];
    let g = levels[((n / 6) % 6) as usize];
    let b = levels[(n % 6) as usize];
    Rgb { r, g, b }
}

fn dist2(a: Rgb, b: Rgb) -> u32 {
    let dr = a.r as i32 - b.r as i32;
    let dg = a.g as i32 - b.g as i32;
    let db = a.b as i32 - b.b as i32;
    (dr * dr + dg * dg + db * db) as u32
}

fn nearest_256(c: Rgb) -> u8 {
    let cube_idx = 16
        + 36 * channel_to_cube_step(c.r)
        +  6 * channel_to_cube_step(c.g)
        +      channel_to_cube_step(c.b);

    let gray = ((c.r as u16 + c.g as u16 + c.b as u16) / 3) as u8;
    let gray_idx = if gray < 8 {
        16
    } else if gray > 247 {
        231
    } else {
        232 + (gray - 8) / 10
    };

    if dist2(c, palette_rgb(cube_idx)) <= dist2(c, palette_rgb(gray_idx)) {
        cube_idx
    } else {
        gray_idx
    }
}
```

- [ ] **Step 4: Verify the pinned table passes**

Run: `cargo test -p coven-cli theme::tests::nearest_256_brand_tokens`
Expected: 1 passed. If any row fails, the diff shows the actual vs expected index — fix the algorithm, do not change the pinned value. (If the algorithm is correct but a value was wrong in the plan, update the plan, but trust the algorithm only after the other rows pass.)

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): add nearest_256 downgrade algorithm"
```

---

## Task 7: ratatui adapters

Convert `Rgb` to ratatui's `Color` and `Style`, respecting mode.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn ratatui_color_with_mode_truecolor() {
        use ratatui::style::Color;
        assert_eq!(
            ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::TrueColor),
            Color::Rgb(0xA7, 0x8B, 0xFF),
        );
    }
    #[test]
    fn ratatui_color_with_mode_indexed_256() {
        use ratatui::style::Color;
        assert_eq!(
            ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::Indexed256),
            Color::Indexed(141),
        );
    }
    #[test]
    fn ratatui_color_with_mode_no_color() {
        use ratatui::style::Color;
        assert_eq!(
            ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::NoColor),
            Color::Reset,
        );
    }
    #[test]
    fn ratatui_style_returns_style_with_fg() {
        use ratatui::style::{Color, Style};
        // Whatever mode is active, the result is a Style whose fg matches ratatui_color.
        let s: Style = ratatui_style(brand::PURPLE_3);
        let expected = Style::default().fg(ratatui_color(brand::PURPLE_3));
        assert_eq!(s, expected);
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: errors for unresolved `ratatui_color`, `ratatui_color_with_mode`, `ratatui_style`.

- [ ] **Step 3: Implement the ratatui adapters**

In `theme.rs`, after the `nearest_256` block:

```rust
// ── ratatui adapters ──

use ratatui::style::{Color as RatColor, Style as RatStyle};

/// Convert an `Rgb` token to a ratatui `Color`, respecting the active `TerminalMode`.
pub fn ratatui_color(c: Rgb) -> RatColor {
    ratatui_color_with_mode(c, mode())
}

pub(crate) fn ratatui_color_with_mode(c: Rgb, m: TerminalMode) -> RatColor {
    match m {
        TerminalMode::TrueColor  => RatColor::Rgb(c.r, c.g, c.b),
        TerminalMode::Indexed256 => RatColor::Indexed(nearest_256(c)),
        TerminalMode::NoColor    => RatColor::Reset,
    }
}

/// Sugar over `Style::default().fg(ratatui_color(c))` — the most common idiom.
pub fn ratatui_style(c: Rgb) -> RatStyle {
    RatStyle::default().fg(ratatui_color(c))
}
```

- [ ] **Step 4: Verify the tests pass**

Run: `cargo test -p coven-cli theme::tests::ratatui`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): add ratatui_color and ratatui_style adapters"
```

---

## Task 8: ANSI Display wrappers (Fg, Bg, Reset)

Zero-allocation `Display` impls for use inside `format!` / `println!`.

**Files:**
- Modify: `crates/coven-cli/src/theme.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn fg_emits_truecolor_escape() {
        assert_eq!(
            format!("{}", Fg::with_mode(brand::PURPLE_3, TerminalMode::TrueColor)),
            "\x1b[38;2;167;139;255m",
        );
    }
    #[test]
    fn fg_emits_indexed_256_escape() {
        assert_eq!(
            format!("{}", Fg::with_mode(brand::PURPLE_3, TerminalMode::Indexed256)),
            "\x1b[38;5;141m",
        );
    }
    #[test]
    fn fg_emits_nothing_in_no_color() {
        assert_eq!(
            format!("{}", Fg::with_mode(brand::PURPLE_3, TerminalMode::NoColor)),
            "",
        );
    }
    #[test]
    fn bg_emits_truecolor_escape() {
        assert_eq!(
            format!("{}", Bg::with_mode(brand::SURFACE_2, TerminalMode::TrueColor)),
            "\x1b[48;2;8;8;18m",
        );
    }
    #[test]
    fn bg_emits_nothing_in_no_color() {
        assert_eq!(
            format!("{}", Bg::with_mode(brand::SURFACE_2, TerminalMode::NoColor)),
            "",
        );
    }
    #[test]
    fn reset_is_empty_in_no_color() {
        assert_eq!(format!("{}", Reset::with_mode(TerminalMode::NoColor)), "");
    }
    #[test]
    fn reset_emits_sgr_zero_otherwise() {
        assert_eq!(format!("{}", Reset::with_mode(TerminalMode::TrueColor)), "\x1b[0m");
        assert_eq!(format!("{}", Reset::with_mode(TerminalMode::Indexed256)), "\x1b[0m");
    }
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p coven-cli --tests`
Expected: errors for unresolved `Fg`, `Bg`, `Reset`.

- [ ] **Step 3: Implement the Display wrappers**

In `theme.rs`, after the ratatui adapters:

```rust
// ── ANSI Display wrappers ──

use std::fmt;

/// Foreground-color ANSI escape. Use in format strings:
/// `println!("{}Title{}", theme::fg(theme::PRIMARY), theme::reset())`
pub struct Fg {
    rgb: Rgb,
    mode: TerminalMode,
}

pub struct Bg {
    rgb: Rgb,
    mode: TerminalMode,
}

pub struct Reset {
    mode: TerminalMode,
}

impl Fg {
    pub(crate) fn with_mode(rgb: Rgb, mode: TerminalMode) -> Self { Self { rgb, mode } }
}
impl Bg {
    pub(crate) fn with_mode(rgb: Rgb, mode: TerminalMode) -> Self { Self { rgb, mode } }
}
impl Reset {
    pub(crate) fn with_mode(mode: TerminalMode) -> Self { Self { mode } }
}

/// Foreground escape for the active mode.
pub fn fg(c: Rgb) -> Fg { Fg::with_mode(c, mode()) }

/// Background escape for the active mode.
pub fn bg(c: Rgb) -> Bg { Bg::with_mode(c, mode()) }

/// SGR-reset escape for the active mode. Empty in `NoColor`.
pub fn reset() -> Reset { Reset::with_mode(mode()) }

impl fmt::Display for Fg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::TrueColor  => write!(f, "\x1b[38;2;{};{};{}m", self.rgb.r, self.rgb.g, self.rgb.b),
            TerminalMode::Indexed256 => write!(f, "\x1b[38;5;{}m", nearest_256(self.rgb)),
            TerminalMode::NoColor    => Ok(()),
        }
    }
}

impl fmt::Display for Bg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::TrueColor  => write!(f, "\x1b[48;2;{};{};{}m", self.rgb.r, self.rgb.g, self.rgb.b),
            TerminalMode::Indexed256 => write!(f, "\x1b[48;5;{}m", nearest_256(self.rgb)),
            TerminalMode::NoColor    => Ok(()),
        }
    }
}

impl fmt::Display for Reset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::NoColor => Ok(()),
            _                     => f.write_str("\x1b[0m"),
        }
    }
}
```

- [ ] **Step 4: Verify the tests pass**

Run: `cargo test -p coven-cli theme::`
Expected: all theme tests passing (count: 1 from Task 1 + 1 + 1 + 1 + 1 + 1 + 4 + 7 = 17). If the count drifts, that's fine as long as none fail.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/theme.rs
git commit -m "feat(theme): add Display-based ANSI Fg/Bg/Reset wrappers"
```

---

## Task 9: Migrate `chat.rs`

Mechanical replacement of the ratatui-path palette. The "test" is that the existing crate builds and tests still pass.

**Files:**
- Modify: `crates/coven-cli/src/chat.rs`

- [ ] **Step 1: Replace the constant block**

Open `crates/coven-cli/src/chat.rs`. Find lines 24–31 (the `// ── OpenCoven palette ──` block):

```rust
// ── OpenCoven palette (256-color) ──────────────────────────────────────────

const PURPLE: Color = Color::Indexed(141); // #af87ff — signature purple
const GOLD: Color = Color::Indexed(220); // #ffd700 — accent gold
const MOON: Color = Color::Indexed(117); // #87d7ff — cool accent
const DIM_FG: Color = Color::Indexed(243); // muted gray
const SURFACE: Color = Color::Indexed(235); // dark surface
const SURFACE_LIGHT: Color = Color::Indexed(237); // slightly lighter surface
```

Replace with:

```rust
// ── Theme imports (palette lives in crate::theme) ──────────────────────────

use crate::theme::{
    self, AGENT_LABEL, DIM, HINT_KEY, HINT_LABEL, PRIMARY, PRIMARY_STRONG, SURFACE,
    SURFACE_STRONG, USER_LABEL,
};
```

- [ ] **Step 2: See every compile error**

Run: `cargo build -p coven-cli 2>&1 | head -60`
Expected: many "cannot find value `PURPLE`/`GOLD`/`MOON`/`DIM_FG`/`SURFACE_LIGHT` in this scope" errors, plus possibly an unused-import warning. These errors are the work list.

- [ ] **Step 3: Replace every callsite, top-to-bottom**

For each error reported, replace per this mapping (use editor find/replace, but verify each one in context — the bold-weight semantics are preserved by `.bold()` on the right-hand side):

| Old expression | New expression |
|---|---|
| `Style::default().fg(PURPLE)` | `theme::ratatui_style(PRIMARY)` |
| `Style::default().fg(PURPLE).bold()` | `theme::ratatui_style(PRIMARY).bold()` |
| `Style::default().fg(PURPLE).italic()` | `theme::ratatui_style(PRIMARY).italic()` |
| `Style::default().fg(GOLD)` | `theme::ratatui_style(HINT_KEY).bold()` *(see note below)* |
| `Style::default().fg(GOLD).bold()` | `theme::ratatui_style(PRIMARY_STRONG).bold()` *(or `AGENT_LABEL` — see note)* |
| `Style::default().fg(MOON)` | `theme::ratatui_style(USER_LABEL)` |
| `Style::default().fg(MOON).bold()` | `theme::ratatui_style(USER_LABEL).bold()` |
| `Style::default().fg(DIM_FG)` | `theme::ratatui_style(DIM)` |
| `.bg(SURFACE)` | `.bg(theme::ratatui_color(SURFACE))` |
| `.bg(SURFACE_LIGHT)` | `.bg(theme::ratatui_color(SURFACE_STRONG))` |

**Semantic-disambiguation notes** (look at the surrounding code, not just the syntax):

- **GOLD on hint-bar keys** (lines ~680–695, the `↑↓`, `Enter`, `Esc`, `/help`, `/agent`, `Ctrl+C`, `PgUp/PgDn` spans): map to `theme::ratatui_style(HINT_KEY).bold()`. The bold weight is what differentiates them now that the hue collapses to TEXT.
- **GOLD on the agent message label** (line 562, `MessageRole::Agent`): map to `theme::ratatui_style(AGENT_LABEL).bold()`.
- **GOLD on section/help titles** (lines 524, 756, 770, 809, 830, 834): map to `theme::ratatui_style(PRIMARY_STRONG).bold()`.

If a callsite is ambiguous, leave it with `PRIMARY_STRONG.bold()` — it will still be brand-correct, just less semantically precise. The semantic refinement can happen in a follow-up.

- [ ] **Step 4: Verify the crate builds**

Run: `cargo build -p coven-cli`
Expected: builds cleanly, no `PURPLE`/`GOLD`/`MOON`/`DIM_FG`/`SURFACE_LIGHT` errors. If you see any of those names remaining, you missed a callsite — re-run step 3.

- [ ] **Step 5: Run all crate tests**

Run: `cargo test -p coven-cli`
Expected: all tests pass, including the existing `chat::tests` module and all theme tests.

- [ ] **Step 6: Commit**

```bash
git add crates/coven-cli/src/chat.rs
git commit -m "refactor(chat): migrate to crate::theme palette"
```

---

## Task 10: Migrate `main.rs`

Delete the raw-ANSI constants, replace per-function `let` bindings with `theme::fg(...)`/`theme::reset()`, and remove the `color_enabled: bool` plumbing.

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Delete the palette constants**

Find lines 252–257:

```rust
const PURPLE: &str = "\x1b[38;5;141m";
const GOLD: &str = "\x1b[38;5;220m";
const ROSE: &str = "\x1b[38;5;218m";
const MOON: &str = "\x1b[38;5;117m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
```

Delete all six.

**Semantic note on the old `DIM` constant:** `"\x1b[2m"` is the SGR *faint-intensity attribute*, not a color — it tells the terminal to dim whatever foreground is active. Our `theme::DIM` (resolving to `brand::TEXT_FAINT` = `#6B6B6B`) is a fixed gray foreground instead. These are different ANSI mechanisms producing similar visual effects.

For this migration: map old `{dim}...{reset}` callsites to `theme::fg(theme::DIM)` + `theme::reset()`. The result is consistently brand-aligned dimmed text. If you discover a callsite where dim was layered *on top of an already-colored* foreground (i.e. the dim attribute was modifying another color rather than standing on its own), revisit — the replacement may need to be `theme::fg(theme::DIM)` applied to the same span, since the new approach overwrites rather than modifies. Inventory inspection suggests this layering does not occur in `main.rs` today (lines 886, 1057 both bind `dim` standalone), but verify.

- [ ] **Step 2: Delete the `ansi(color_enabled, code)` helper**

Find around line 1201:

```rust
fn ansi(enabled: bool, code: &'static str) -> &'static str {
    if enabled { code } else { "" }
}
```

Delete it.

- [ ] **Step 3: Update `render_session_browser_frame_with_color`**

Find the signature (line 876 area):

```rust
fn render_session_browser_frame_with_color(
    /* ... existing params ... */,
    color_enabled: bool,
) -> String {
    let purple = ansi(color_enabled, PURPLE);
    let gold   = ansi(color_enabled, GOLD);
    let rose   = ansi(color_enabled, ROSE);
    let moon   = ansi(color_enabled, MOON);
    /* ... body uses {purple}/{gold}/{rose}/{moon}/{reset} ... */
}
```

Remove the `color_enabled: bool` parameter and replace the `ansi(...)` bindings with `theme::fg(...)` calls. The `reset` binding becomes `theme::reset()`. Map the old names to the new tokens per the spec:

```rust
fn render_session_browser_frame_with_color(
    /* ... existing params, no color_enabled ... */
) -> String {
    let primary        = theme::fg(theme::PRIMARY);
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let field_label    = theme::fg(theme::FIELD_LABEL);
    let user_label     = theme::fg(theme::USER_LABEL);
    let dim            = theme::fg(theme::DIM);
    let reset          = theme::reset();
    /* ... body: replace {purple}->{primary}, {gold}->{primary_strong},
                         {rose}->{field_label}, {moon}->{user_label},
                         {dim}->{dim} (name unchanged, semantics now a gray fg) */
}
```

Find every call to `render_session_browser_frame_with_color(...)` (grep for the function name) and remove the `color_enabled` argument.

- [ ] **Step 4: Update `render_magical_tui_frame_with_color_and_width`**

Same pattern at line 1046:

```rust
fn render_magical_tui_frame_with_color_and_width(
    /* ... existing params, no color_enabled ... */
) -> String {
    let primary        = theme::fg(theme::PRIMARY);
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let field_label    = theme::fg(theme::FIELD_LABEL);
    let user_label     = theme::fg(theme::USER_LABEL);
    let dim            = theme::fg(theme::DIM);
    let reset          = theme::reset();
    /* ... body: same name mapping as Step 3 ... */
}
```

Update its callers.

- [ ] **Step 5: Update bare `println!` sites**

Search for remaining uses of `{PURPLE}`, `{GOLD}`, `{ROSE}`, `{MOON}`, `{RESET}` in format strings. Each one needs a `let primary = theme::fg(theme::PRIMARY); let reset = theme::reset();` (and whichever others it uses) bound at the top of the enclosing function, then format-string identifier renames.

Specific sites known from inventory:
- Line 470 area: `println!("{PURPLE}The circle fades...")` — bind `primary` and `reset` at top of containing fn.
- Line 563 area: `println!("{GOLD}Coven TUI{RESET}")` — bind `primary_strong` and `reset`.
- Line 576 area: `println!("{GOLD}Coven quick start{RESET}")` — same.
- Line 588 area: `println!("{GOLD}Run an agent in this project{RESET}")` — same.
- Line 776, 795, 801 area: more `{PURPLE}...{RESET}` cancellation lines.

Run grep to find any remaining ones:

```bash
grep -nE "\{(PURPLE|GOLD|ROSE|MOON|RESET|DIM)\}" crates/coven-cli/src/main.rs
```

Expected after this step: no matches.

- [ ] **Step 6: Verify the crate builds**

Run: `cargo build -p coven-cli`
Expected: builds cleanly. If you see "cannot find value `PURPLE`" or similar, you missed a site — re-run grep.

- [ ] **Step 7: Run all tests**

Run: `cargo test -p coven-cli`
Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/coven-cli/src/main.rs
git commit -m "refactor(cli): migrate main.rs palettes to crate::theme"
```

---

## Task 11: Acceptance verification

Manual checks for the four acceptance criteria not covered by unit tests.

**Files:** none

- [ ] **Step 1: Verify `NO_COLOR` strips escapes**

Run: `NO_COLOR=1 cargo run -p coven-cli -- sessions 2>&1 | cat | head -20`
Expected: output contains no `\x1b[` sequences. Pipe through `od -c` or `cat -v` to be sure:

```bash
NO_COLOR=1 cargo run -p coven-cli -- sessions 2>&1 | cat -v | grep -E '\^\[' | head
```

Expected: no matches.

- [ ] **Step 2: Verify piped stdout strips escapes**

Run: `cargo run -p coven-cli -- sessions 2>&1 | cat -v | grep -E '\^\[' | head`
Expected: no matches. (Without `NO_COLOR`, but with stdout piped — `IsTerminal` should evaluate to false and we should fall to `NoColor`.)

- [ ] **Step 3: Verify truecolor terminals show distinct purples**

If running in a truecolor-capable terminal (iTerm2, Alacritty, Kitty, modern Terminal.app, WezTerm — most modern terminals):

```bash
echo $COLORTERM   # should print 'truecolor' or '24bit'
cargo run -p coven-cli -- chat   # or whichever command shows messages from multiple roles
```

Verify by eye:
- System messages (PURPLE_3 italic): lightest purple, italic.
- Agent labels (PURPLE_2 bold): medium purple, bold.
- User labels (PURPLE_1 bold): deepest purple, bold.

All three should be visibly distinguishable.

- [ ] **Step 4: Verify 256-color downgrade**

Force-degrade and inspect output:

```bash
COLORTERM= TERM=xterm-256color cargo run -p coven-cli -- sessions 2>&1 | cat -v | head -20
```

Expected: output contains `^[[38;5;Nm` style escapes (indexed), not `^[[38;2;R;G;Bm` (truecolor).

- [ ] **Step 5: Verify drift test still passes on a clean run**

Run: `cargo test -p coven-cli theme::tests::brand_tokens_mirror_color_tokens_css`
Expected: 1 passed.

- [ ] **Step 6: Final crate-wide test sweep**

Run: `cargo test -p coven-cli`
Expected: every test in the crate passes.

- [ ] **Step 7: No commit needed**

This task is verification only. If any step fails, return to the relevant earlier task and fix.

---

## Done

When Task 11 is complete, every acceptance criterion in the spec is met:

1. ✅ `theme.rs` exists and implements the module surface — Tasks 1–8.
2. ✅ No `Color::Indexed(...)` palette constants in `chat.rs` — Task 9.
3. ✅ No `"\x1b[38;5;..."` literal palette constants in `main.rs` — Task 10.
4. ✅ `ansi(color_enabled, ...)` and `color_enabled: bool` parameters gone — Task 10.
5. ✅ Tests pass — Tasks 1–8 each end with a passing test; Task 11 step 6 confirms.
6. ✅ `NO_COLOR=1 coven sessions` produces no escape codes — Task 11 step 1.
7. ✅ `coven sessions | cat` produces no escape codes — Task 11 step 2.
8. ✅ Three role-purples visibly distinct on truecolor — Task 11 step 3.
9. ✅ Drift test passes — Task 11 step 5.
