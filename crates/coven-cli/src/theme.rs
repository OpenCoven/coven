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

/// Raw brand tokens, mirroring `brand/ui/color-tokens.css`.
/// Enforced by the `brand_tokens_mirror_color_tokens_css` test.
pub mod brand {
    use super::Rgb;
    pub const PURPLE_1: Rgb = Rgb {
        r: 0x7A,
        g: 0x6D,
        b: 0xAA,
    };
    pub const PURPLE_2: Rgb = Rgb {
        r: 0x9A,
        g: 0x8E,
        b: 0xCD,
    };
    pub const PURPLE_3: Rgb = Rgb {
        r: 0xC5,
        g: 0xBD,
        b: 0xED,
    };
    pub const ACCENT_BLUE: Rgb = Rgb {
        r: 0x0A,
        g: 0x84,
        b: 0xFF,
    };
    #[allow(dead_code)]
    pub const DANGER: Rgb = Rgb {
        r: 0xFF,
        g: 0x3B,
        b: 0x30,
    };
    #[allow(dead_code)]
    pub const SUCCESS: Rgb = Rgb {
        r: 0x30,
        g: 0xD1,
        b: 0x58,
    };
    /// rgba(255, 255, 255, 0.94) on black = round(255 * 0.94) = 240
    pub const TEXT: Rgb = Rgb {
        r: 0xF0,
        g: 0xF0,
        b: 0xF0,
    };
    /// rgba(255, 255, 255, 0.64) on black = round(255 * 0.64) = 163
    pub const TEXT_MUTED: Rgb = Rgb {
        r: 0xA3,
        g: 0xA3,
        b: 0xA3,
    };
    /// rgba(255, 255, 255, 0.42) on black = round(255 * 0.42) = 107
    pub const TEXT_FAINT: Rgb = Rgb {
        r: 0x6B,
        g: 0x6B,
        b: 0x6B,
    };
    /// True-black canvas (`--oc-surface-0`). The terminal canvas/backdrop —
    /// distinct from `SURFACE_1`, which is the brand chrome surface that sits
    /// on top of it.
    pub const SURFACE_0: Rgb = Rgb { r: 0, g: 0, b: 0 };
    pub const SURFACE_1: Rgb = Rgb {
        r: 0x0F,
        g: 0x0A,
        b: 0x14,
    };
    pub const SURFACE_2: Rgb = Rgb {
        r: 0x1A,
        g: 0x18,
        b: 0x25,
    };
    /// Lifted brand surface (`--oc-surface-3`) — used for scrollbar tracks
    /// and other quiet recessed chrome where pure black is too harsh.
    pub const SURFACE_3: Rgb = Rgb {
        r: 0x2A,
        g: 0x24,
        b: 0x38,
    };
    /// `--oc-border-subtle` flattened on black: rgba(255,255,255,0.08) →
    /// 0x14 per channel. Use for unfocused rules and divider lines.
    pub const BORDER_SUBTLE: Rgb = Rgb {
        r: 0x14,
        g: 0x14,
        b: 0x14,
    };
    /// `--oc-border-strong` flattened on black: rgba(255,255,255,0.14) →
    /// 0x24 per channel. Use for focused rules and emphasized dividers.
    pub const BORDER_STRONG: Rgb = Rgb {
        r: 0x24,
        g: 0x24,
        b: 0x24,
    };
}

// ── Semantic tokens (what callsites import) ──

pub const PRIMARY: Rgb = brand::PURPLE_3;
pub const PRIMARY_STRONG: Rgb = brand::PURPLE_2;
pub const AGENT_LABEL: Rgb = brand::PURPLE_2;
pub const USER_LABEL: Rgb = brand::PURPLE_1;
pub const HINT_KEY: Rgb = brand::TEXT;
pub const HINT_LABEL: Rgb = brand::TEXT_MUTED;
pub const FIELD_LABEL: Rgb = brand::TEXT_MUTED;
pub const DANGER: Rgb = brand::DANGER;
pub const SUCCESS: Rgb = brand::SUCCESS;
pub const DIM: Rgb = brand::TEXT_FAINT;
pub const SURFACE: Rgb = brand::SURFACE_1;
pub const SURFACE_STRONG: Rgb = brand::SURFACE_2;
/// Body text — replaces the ad-hoc `Color::White` that screens were reaching
/// for. Brand-aligned, near-white but never pure white.
pub const TEXT: Rgb = brand::TEXT;
/// Secondary body text — quieter than `TEXT`, brighter than `DIM`. Replaces
/// hand-rolled 256-color indices for agent-side message bodies.
pub const TEXT_DIM: Rgb = brand::TEXT_MUTED;
/// Inactive border / divider color. Replaces hand-picked 256-color indices
/// for input bezels and other quiet outlines.
pub const BORDER_DIM: Rgb = brand::TEXT_FAINT;
/// Scrollbar track and other recessed chrome (very dark, brand-tinted).
pub const SCROLL_TRACK: Rgb = brand::SURFACE_3;
/// Bottom-most canvas color behind every TUI screen.
pub const BACKDROP: Rgb = brand::SURFACE_0;
/// Quiet divider line (e.g. unfocused single-rule input area). Mirrors
/// `--oc-border-subtle` from `brand/ui/color-tokens.css`.
pub const BORDER_SUBTLE: Rgb = brand::BORDER_SUBTLE;
/// Emphasized divider line (e.g. focused input area, active rule).
/// Mirrors `--oc-border-strong`.
pub const BORDER_STRONG: Rgb = brand::BORDER_STRONG;

// ── Syntax highlighting tokens (chat code-block rendering) ──

/// Keyword token in highlighted code blocks. Pairs with `Modifier::BOLD` at
/// the call site so weight, not saturation, carries the emphasis.
pub const SYNTAX_KEYWORD: Rgb = brand::PURPLE_2;
/// String / character literal in highlighted code blocks. Reuses the brand
/// green — green-for-strings is the most universal terminal convention.
pub const SYNTAX_STRING: Rgb = brand::SUCCESS;
/// Numeric literal in highlighted code blocks. Brand accent-blue keeps
/// numbers distinct from the purple keyword/attribute family.
pub const SYNTAX_NUMBER: Rgb = brand::ACCENT_BLUE;
/// Comment in highlighted code blocks. Quiet & italic so comments recede.
pub const SYNTAX_COMMENT: Rgb = brand::TEXT_FAINT;
/// Attribute / decorator / lifetime token. Lighter purple than the keyword
/// shade so the two purple groups stay distinguishable.
pub const SYNTAX_ATTRIBUTE: Rgb = brand::PURPLE_3;
/// Removed / deletion token — diff `-` lines, future "rejected" hunks.
/// Reuses the brand danger red so the visual matches universal diff
/// convention (`+` green for additions, `-` red for deletions).
pub const SYNTAX_REMOVED: Rgb = brand::DANGER;

// ── Status semantics ──

/// What a status indicator is communicating. Drives the color of "ready",
/// "working", "error" etc. across the TUI so renderers never pick a raw
/// `Color::Green` again. `Ready` is consumed today by the chat status bar;
/// the other variants are pre-wired for the screen renderers landing in the
/// next phase and are reachable via `status_token` / `status_style`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    #[allow(dead_code)]
    Working,
    #[allow(dead_code)]
    Warning,
    #[allow(dead_code)]
    Error,
    #[allow(dead_code)]
    Idle,
}

/// Brand token for a status semantic. Use via `status_style` or
/// `ratatui_style(status_token(...))`.
pub fn status_token(status: Status) -> Rgb {
    match status {
        Status::Ready => SUCCESS,
        Status::Working => PRIMARY,
        Status::Warning => PRIMARY_STRONG,
        Status::Error => DANGER,
        Status::Idle => DIM,
    }
}

// ── Terminal-mode detection ──

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TerminalMode {
    TrueColor,
    Indexed256,
    NoColor,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct EnvInputs<'a> {
    pub no_color: Option<&'a str>,
    pub colorterm: Option<&'a str>,
    pub term: Option<&'a str>,
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
        Some(t) if t.ends_with("-direct") => TerminalMode::TrueColor,
        Some(t) if t.ends_with("-256color") => TerminalMode::Indexed256,
        Some("dumb") | None => TerminalMode::NoColor,
        Some(_) => TerminalMode::Indexed256,
    }
}

use std::sync::OnceLock;

static MODE: OnceLock<TerminalMode> = OnceLock::new();

/// Resolve the terminal mode for this process. Cached on first call.
pub fn mode() -> TerminalMode {
    *MODE.get_or_init(detect_mode)
}

fn detect_mode() -> TerminalMode {
    use std::io::IsTerminal;
    let no_color = std::env::var("NO_COLOR").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    let term = std::env::var("TERM").ok();
    detect_mode_from(EnvInputs {
        no_color: no_color.as_deref(),
        colorterm: colorterm.as_deref(),
        term: term.as_deref(),
        stdout_is_tty: std::io::stdout().is_terminal(),
    })
}

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
        + 6 * channel_to_cube_step(c.g)
        + channel_to_cube_step(c.b);

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

// ── ratatui adapters ──

use ratatui::style::{Color as RatColor, Style as RatStyle};

/// Convert an `Rgb` token to a ratatui `Color`, respecting the active `TerminalMode`.
pub fn ratatui_color(c: Rgb) -> RatColor {
    ratatui_color_with_mode(c, mode())
}

pub(crate) fn ratatui_color_with_mode(c: Rgb, m: TerminalMode) -> RatColor {
    match m {
        TerminalMode::TrueColor => RatColor::Rgb(c.r, c.g, c.b),
        TerminalMode::Indexed256 => RatColor::Indexed(nearest_256(c)),
        TerminalMode::NoColor => RatColor::Reset,
    }
}

/// Sugar over `Style::default().fg(ratatui_color(c))` — the most common idiom.
pub fn ratatui_style(c: Rgb) -> RatStyle {
    RatStyle::default().fg(ratatui_color(c))
}

/// Sugar over `ratatui_style(status_token(s))` — keeps status indicators
/// in renderers tied to the `Status` semantic, not raw colors.
pub fn status_style(status: Status) -> RatStyle {
    ratatui_style(status_token(status))
}

// ── ANSI Display wrappers ──

use std::fmt;

/// Foreground-color ANSI escape. Use in format strings:
/// `println!("{}Title{}", theme::fg(theme::PRIMARY), theme::reset())`
#[derive(Copy, Clone, Debug)]
pub struct Fg {
    rgb: Rgb,
    mode: TerminalMode,
}

// `Bg` is pre-wired alongside `Fg` for symmetry; full backgrounds via ANSI
// escape land in a later phase (currently main.rs uses ratatui Style::bg).
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub struct Bg {
    rgb: Rgb,
    mode: TerminalMode,
}

#[derive(Copy, Clone, Debug)]
pub struct Reset {
    mode: TerminalMode,
}

impl Fg {
    pub(crate) fn with_mode(rgb: Rgb, mode: TerminalMode) -> Self {
        Self { rgb, mode }
    }
}
#[allow(dead_code)]
impl Bg {
    pub(crate) fn with_mode(rgb: Rgb, mode: TerminalMode) -> Self {
        Self { rgb, mode }
    }
}
impl Reset {
    pub(crate) fn with_mode(mode: TerminalMode) -> Self {
        Self { mode }
    }
}

/// Foreground escape for the active mode.
pub fn fg(c: Rgb) -> Fg {
    Fg::with_mode(c, mode())
}

/// Background escape for the active mode.
#[allow(dead_code)]
pub fn bg(c: Rgb) -> Bg {
    Bg::with_mode(c, mode())
}

/// SGR-reset escape for the active mode. Empty in `NoColor`.
pub fn reset() -> Reset {
    Reset::with_mode(mode())
}

impl fmt::Display for Fg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::TrueColor => {
                write!(f, "\x1b[38;2;{};{};{}m", self.rgb.r, self.rgb.g, self.rgb.b)
            }
            TerminalMode::Indexed256 => write!(f, "\x1b[38;5;{}m", nearest_256(self.rgb)),
            TerminalMode::NoColor => Ok(()),
        }
    }
}

impl fmt::Display for Bg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::TrueColor => {
                write!(f, "\x1b[48;2;{};{};{}m", self.rgb.r, self.rgb.g, self.rgb.b)
            }
            TerminalMode::Indexed256 => write!(f, "\x1b[48;5;{}m", nearest_256(self.rgb)),
            TerminalMode::NoColor => Ok(()),
        }
    }
}

impl fmt::Display for Reset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::NoColor => Ok(()),
            _ => f.write_str("\x1b[0m"),
        }
    }
}

// ── Palette ──

/// Pre-resolved foreground escapes for every common token, plus the matching
/// `Reset`. Renderers building a raw-ANSI frame can grab one of these and
/// stop writing `theme::Fg::with_mode(theme::PRIMARY, mode)` six times in a
/// row. Phase 2 defines this helper and exercises it via tests; Phase 3
/// will migrate the existing per-renderer boilerplate over.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub struct Palette {
    pub mode: TerminalMode,
    pub primary: Fg,
    pub primary_strong: Fg,
    pub user_label: Fg,
    pub agent_label: Fg,
    pub field_label: Fg,
    pub hint_key: Fg,
    pub hint_label: Fg,
    pub text: Fg,
    pub text_dim: Fg,
    pub dim: Fg,
    pub reset: Reset,
}

/// Palette for the active terminal mode.
#[allow(dead_code)]
pub fn palette() -> Palette {
    palette_for(mode())
}

/// Palette for an explicit mode (useful for tests and the plain-text
/// renderers that pass `TerminalMode::NoColor`).
pub fn palette_for(mode: TerminalMode) -> Palette {
    Palette {
        mode,
        primary: Fg::with_mode(PRIMARY, mode),
        primary_strong: Fg::with_mode(PRIMARY_STRONG, mode),
        user_label: Fg::with_mode(USER_LABEL, mode),
        agent_label: Fg::with_mode(AGENT_LABEL, mode),
        field_label: Fg::with_mode(FIELD_LABEL, mode),
        hint_key: Fg::with_mode(HINT_KEY, mode),
        hint_label: Fg::with_mode(HINT_LABEL, mode),
        text: Fg::with_mode(TEXT, mode),
        text_dim: Fg::with_mode(TEXT_DIM, mode),
        dim: Fg::with_mode(DIM, mode),
        reset: Reset::with_mode(mode),
    }
}

// ── Width helpers ──

/// Truncate `value` to at most `limit` characters, using `…` as the last
/// character when truncation happens. `chars().count()` is intentional: every
/// terminal column we render today is a single character cell, and `…` itself
/// is one cell. Wide-cell text (CJK, emoji) is out of scope for the current
/// TUI screens; if that changes, swap this for `unicode_width`.
pub fn fit_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_string();
    }
    if limit == 0 {
        return String::new();
    }
    if limit == 1 {
        return "…".to_string();
    }
    let mut fitted: String = value.chars().take(limit - 1).collect();
    fitted.push('…');
    fitted
}

#[cfg(test)]
mod tests {
    use super::*;
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
        Rgb {
            r: flat(r),
            g: flat(g),
            b: flat(b),
        }
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
                out.entry(name.trim().to_string())
                    .or_insert_with(|| value.trim().to_string());
            }
        }
        out
    }

    #[test]
    fn rgb_is_copy_and_eq() {
        let a = Rgb { r: 1, g: 2, b: 3 };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn brand_tokens_mirror_color_tokens_css() {
        let css = include_str!("../../../brand/ui/color-tokens.css");
        let vars = parse_css_vars(css);

        assert_eq!(
            brand::PURPLE_1,
            rgb_from_hex(&vars["--oc-purple-1"]),
            "--oc-purple-1"
        );
        assert_eq!(
            brand::PURPLE_2,
            rgb_from_hex(&vars["--oc-purple-2"]),
            "--oc-purple-2"
        );
        assert_eq!(
            brand::PURPLE_3,
            rgb_from_hex(&vars["--oc-purple-3"]),
            "--oc-purple-3"
        );
        assert_eq!(
            brand::ACCENT_BLUE,
            rgb_from_hex(&vars["--oc-accent-blue"]),
            "--oc-accent-blue"
        );
        assert_eq!(
            brand::DANGER,
            rgb_from_hex(&vars["--oc-danger"]),
            "--oc-danger"
        );
        assert_eq!(
            brand::SUCCESS,
            rgb_from_hex(&vars["--oc-success"]),
            "--oc-success"
        );
        assert_eq!(
            brand::SURFACE_0,
            rgb_from_hex(&vars["--oc-surface-0"]),
            "--oc-surface-0"
        );
        assert_eq!(
            brand::SURFACE_1,
            rgb_from_hex(&vars["--oc-surface-1"]),
            "--oc-surface-1"
        );
        assert_eq!(
            brand::SURFACE_2,
            rgb_from_hex(&vars["--oc-surface-2"]),
            "--oc-surface-2"
        );
        assert_eq!(
            brand::SURFACE_3,
            rgb_from_hex(&vars["--oc-surface-3"]),
            "--oc-surface-3"
        );

        assert_eq!(
            brand::TEXT,
            flatten_on_black(&vars["--oc-text"]),
            "--oc-text"
        );
        assert_eq!(
            brand::TEXT_MUTED,
            flatten_on_black(&vars["--oc-text-muted"]),
            "--oc-text-muted"
        );
        assert_eq!(
            brand::TEXT_FAINT,
            flatten_on_black(&vars["--oc-text-faint"]),
            "--oc-text-faint"
        );

        assert_eq!(
            brand::BORDER_SUBTLE,
            flatten_on_black(&vars["--oc-border-subtle"]),
            "--oc-border-subtle"
        );
        assert_eq!(
            brand::BORDER_STRONG,
            flatten_on_black(&vars["--oc-border-strong"]),
            "--oc-border-strong"
        );
    }

    #[test]
    fn semantic_tokens_resolve_to_brand_tokens() {
        assert_eq!(PRIMARY, brand::PURPLE_3);
        assert_eq!(PRIMARY_STRONG, brand::PURPLE_2);
        assert_eq!(AGENT_LABEL, brand::PURPLE_2);
        assert_eq!(USER_LABEL, brand::PURPLE_1);
        assert_eq!(HINT_KEY, brand::TEXT);
        assert_eq!(HINT_LABEL, brand::TEXT_MUTED);
        assert_eq!(FIELD_LABEL, brand::TEXT_MUTED);
        assert_eq!(DANGER, brand::DANGER);
        assert_eq!(SUCCESS, brand::SUCCESS);
        assert_eq!(DIM, brand::TEXT_FAINT);
        assert_eq!(SURFACE, brand::SURFACE_1);
        assert_eq!(SURFACE_STRONG, brand::SURFACE_2);
        assert_eq!(TEXT, brand::TEXT);
        assert_eq!(TEXT_DIM, brand::TEXT_MUTED);
        assert_eq!(BORDER_DIM, brand::TEXT_FAINT);
        assert_eq!(SCROLL_TRACK, brand::SURFACE_3);
        assert_eq!(BACKDROP, brand::SURFACE_0);
        assert_eq!(BORDER_SUBTLE, brand::BORDER_SUBTLE);
        assert_eq!(BORDER_STRONG, brand::BORDER_STRONG);
        assert_eq!(SYNTAX_KEYWORD, brand::PURPLE_2);
        assert_eq!(SYNTAX_STRING, brand::SUCCESS);
        assert_eq!(SYNTAX_NUMBER, brand::ACCENT_BLUE);
        assert_eq!(SYNTAX_COMMENT, brand::TEXT_FAINT);
        assert_eq!(SYNTAX_ATTRIBUTE, brand::PURPLE_3);
        assert_eq!(SYNTAX_REMOVED, brand::DANGER);
    }

    #[test]
    fn status_token_maps_each_variant_to_a_semantic_token() {
        assert_eq!(status_token(Status::Ready), SUCCESS);
        assert_eq!(status_token(Status::Working), PRIMARY);
        assert_eq!(status_token(Status::Warning), PRIMARY_STRONG);
        assert_eq!(status_token(Status::Error), DANGER);
        assert_eq!(status_token(Status::Idle), DIM);
    }

    #[test]
    fn status_style_in_no_color_mode_resolves_to_reset_fg() {
        // We can't easily flip global mode in a test, but `status_style`
        // composes `ratatui_style` over `status_token`. Spot-check the chain
        // by asserting `ratatui_color_with_mode` collapses to Reset in
        // NoColor — which is the property `status_style` inherits.
        use ratatui::style::Color;
        assert_eq!(
            ratatui_color_with_mode(status_token(Status::Ready), TerminalMode::NoColor),
            Color::Reset,
        );
        assert_eq!(
            ratatui_color_with_mode(status_token(Status::Error), TerminalMode::NoColor),
            Color::Reset,
        );
    }

    #[test]
    fn palette_in_no_color_mode_emits_no_escapes() {
        let p = palette_for(TerminalMode::NoColor);
        assert_eq!(p.mode, TerminalMode::NoColor);
        for s in [
            format!("{}", p.primary),
            format!("{}", p.primary_strong),
            format!("{}", p.user_label),
            format!("{}", p.agent_label),
            format!("{}", p.field_label),
            format!("{}", p.hint_key),
            format!("{}", p.hint_label),
            format!("{}", p.text),
            format!("{}", p.text_dim),
            format!("{}", p.dim),
            format!("{}", p.reset),
        ] {
            assert!(s.is_empty(), "expected empty in NoColor, got {s:?}");
        }
    }

    #[test]
    fn palette_in_true_color_mode_emits_truecolor_escapes() {
        let p = palette_for(TerminalMode::TrueColor);
        // PRIMARY = brand::PURPLE_3 = 0xC5 0xBD 0xED
        assert_eq!(format!("{}", p.primary), "\x1b[38;2;197;189;237m");
        // RESET is SGR-zero
        assert_eq!(format!("{}", p.reset), "\x1b[0m");
        // TEXT = brand::TEXT = 0xF0 0xF0 0xF0
        assert_eq!(format!("{}", p.text), "\x1b[38;2;240;240;240m");
    }

    #[test]
    fn palette_in_indexed_256_mode_emits_indexed_escapes() {
        let p = palette_for(TerminalMode::Indexed256);
        // PRIMARY → 183 (verified by nearest_256_brand_tokens)
        assert_eq!(format!("{}", p.primary), "\x1b[38;5;183m");
        // RESET still SGR-zero in Indexed256
        assert_eq!(format!("{}", p.reset), "\x1b[0m");
    }

    #[test]
    fn fit_chars_returns_input_when_already_within_limit() {
        assert_eq!(fit_chars("hello", 10), "hello");
        assert_eq!(fit_chars("hello", 5), "hello");
    }

    #[test]
    fn fit_chars_truncates_with_ellipsis_when_over_limit() {
        assert_eq!(fit_chars("hello world", 8), "hello w…");
        assert_eq!(fit_chars("abcdef", 3), "ab…");
    }

    #[test]
    fn fit_chars_handles_zero_and_one_limit_edge_cases() {
        assert_eq!(fit_chars("anything", 0), "");
        assert_eq!(fit_chars("anything", 1), "…");
        // Empty input is always under any limit.
        assert_eq!(fit_chars("", 0), "");
        assert_eq!(fit_chars("", 5), "");
    }

    #[test]
    fn fit_chars_counts_chars_not_bytes_for_multibyte_input() {
        // 5 chars, each multi-byte. Already within a 5-cell limit.
        assert_eq!(fit_chars("héllo", 5), "héllo");
        // Truncating multi-byte input keeps char-aligned slices.
        assert_eq!(fit_chars("héllo world", 6), "héllo…");
    }

    #[test]
    fn detect_mode_from_table() {
        use TerminalMode::*;
        let cases: &[(EnvInputs<'_>, TerminalMode)] = &[
            // NO_COLOR=1 always wins, even with truecolor and TTY
            (
                EnvInputs {
                    no_color: Some("1"),
                    colorterm: Some("truecolor"),
                    term: Some("xterm-256color"),
                    stdout_is_tty: true,
                },
                NoColor,
            ),
            // Empty-string NO_COLOR treated as unset
            (
                EnvInputs {
                    no_color: Some(""),
                    colorterm: Some("truecolor"),
                    term: Some("xterm-256color"),
                    stdout_is_tty: true,
                },
                TrueColor,
            ),
            // COLORTERM=truecolor wins on a TTY
            (
                EnvInputs {
                    no_color: None,
                    colorterm: Some("truecolor"),
                    term: Some("xterm-256color"),
                    stdout_is_tty: true,
                },
                TrueColor,
            ),
            // COLORTERM=24bit also yields truecolor
            (
                EnvInputs {
                    no_color: None,
                    colorterm: Some("24bit"),
                    term: Some("xterm-256color"),
                    stdout_is_tty: true,
                },
                TrueColor,
            ),
            // TERM=*-direct yields truecolor even without COLORTERM
            (
                EnvInputs {
                    no_color: None,
                    colorterm: None,
                    term: Some("xterm-direct"),
                    stdout_is_tty: true,
                },
                TrueColor,
            ),
            // TERM=*-256color yields indexed 256
            (
                EnvInputs {
                    no_color: None,
                    colorterm: None,
                    term: Some("xterm-256color"),
                    stdout_is_tty: true,
                },
                Indexed256,
            ),
            // Plain xterm: default to indexed 256
            (
                EnvInputs {
                    no_color: None,
                    colorterm: None,
                    term: Some("xterm"),
                    stdout_is_tty: true,
                },
                Indexed256,
            ),
            // TERM=dumb forces no color
            (
                EnvInputs {
                    no_color: None,
                    colorterm: None,
                    term: Some("dumb"),
                    stdout_is_tty: true,
                },
                NoColor,
            ),
            // TERM unset forces no color
            (
                EnvInputs {
                    no_color: None,
                    colorterm: None,
                    term: None,
                    stdout_is_tty: true,
                },
                NoColor,
            ),
            // Piped stdout always disables color, regardless of env
            (
                EnvInputs {
                    no_color: None,
                    colorterm: Some("truecolor"),
                    term: Some("xterm-256color"),
                    stdout_is_tty: false,
                },
                NoColor,
            ),
            (
                EnvInputs {
                    no_color: Some("1"),
                    colorterm: None,
                    term: None,
                    stdout_is_tty: false,
                },
                NoColor,
            ),
        ];
        for (i, (inputs, expected)) in cases.iter().enumerate() {
            assert_eq!(detect_mode_from(*inputs), *expected, "row {i}: {inputs:?}",);
        }
    }

    #[test]
    fn mode_returns_a_value_and_caches() {
        let first = mode();
        let second = mode();
        assert_eq!(
            first, second,
            "mode() must return the same value on repeated calls"
        );
    }

    #[test]
    fn nearest_256_brand_tokens() {
        assert_eq!(nearest_256(brand::PURPLE_3), 183);
        assert_eq!(nearest_256(brand::PURPLE_2), 104);
        assert_eq!(nearest_256(brand::PURPLE_1), 97);
        assert_eq!(nearest_256(brand::ACCENT_BLUE), 33);
        assert_eq!(nearest_256(brand::DANGER), 203);
        assert_eq!(nearest_256(brand::SUCCESS), 77);
        assert_eq!(nearest_256(Rgb { r: 0, g: 0, b: 0 }), 16);
        assert_eq!(
            nearest_256(Rgb {
                r: 255,
                g: 255,
                b: 255
            }),
            231
        );
        assert_eq!(
            nearest_256(Rgb {
                r: 128,
                g: 128,
                b: 128
            }),
            244
        );
        assert_eq!(
            nearest_256(Rgb {
                r: 248,
                g: 248,
                b: 248
            }),
            231
        );
    }

    #[test]
    fn ratatui_color_with_mode_truecolor() {
        use ratatui::style::Color;
        assert_eq!(
            ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::TrueColor),
            Color::Rgb(0xC5, 0xBD, 0xED),
        );
    }
    #[test]
    fn ratatui_color_with_mode_indexed_256() {
        use ratatui::style::Color;
        assert_eq!(
            ratatui_color_with_mode(brand::PURPLE_3, TerminalMode::Indexed256),
            Color::Indexed(183),
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
        use ratatui::style::Style;
        let s: Style = ratatui_style(brand::PURPLE_3);
        let expected = Style::default().fg(ratatui_color(brand::PURPLE_3));
        assert_eq!(s, expected);
    }

    #[test]
    fn fg_emits_truecolor_escape() {
        assert_eq!(
            format!(
                "{}",
                Fg::with_mode(brand::PURPLE_3, TerminalMode::TrueColor)
            ),
            "\x1b[38;2;197;189;237m",
        );
    }
    #[test]
    fn fg_emits_indexed_256_escape() {
        assert_eq!(
            format!(
                "{}",
                Fg::with_mode(brand::PURPLE_3, TerminalMode::Indexed256)
            ),
            "\x1b[38;5;183m",
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
            format!(
                "{}",
                Bg::with_mode(brand::SURFACE_2, TerminalMode::TrueColor)
            ),
            "\x1b[48;2;26;24;37m",
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
        assert_eq!(
            format!("{}", Reset::with_mode(TerminalMode::TrueColor)),
            "\x1b[0m"
        );
        assert_eq!(
            format!("{}", Reset::with_mode(TerminalMode::Indexed256)),
            "\x1b[0m"
        );
    }
}
