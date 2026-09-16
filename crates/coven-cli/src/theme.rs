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

impl Rgb {
    /// Linear interpolation toward `other`. `t` is clamped to `0.0..=1.0`, so
    /// `lerp` at 0 is `self` and at 1 is `other`.
    ///
    /// This exists for the launcher masthead's vertical ramp — the terminal
    /// analog of `--oc-gradient-signature`. It is deliberately not a general
    /// license to gradient UI chrome: `DESIGN.md` §3 keeps normal surfaces
    /// flat and reserves ramps for the ambient-backdrop exception.
    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let channel = |a: u8, b: u8| -> u8 {
            let a = f32::from(a);
            let b = f32::from(b);
            (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
        };
        Rgb {
            r: channel(self.r, other.r),
            g: channel(self.g, other.g),
            b: channel(self.b, other.b),
        }
    }
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

/// Light-appearance counterparts of [`brand`], mirroring the
/// `@media (prefers-color-scheme: light)` block of
/// `brand/ui/color-tokens.css`. Enforced by the same
/// `brand_tokens_mirror_color_tokens_css` drift test.
///
/// The flattening convention matches [`brand`] exactly, with one
/// substitution: where the dark tokens composite over `--oc-surface-0` =
/// `#000000`, these composite over the *light* `--oc-surface-0` = `#FAFBFD`
/// = (250, 251, 253). Each derived value below shows its arithmetic.
///
/// The violet spectrum is re-pitched rather than reused, and it inverts by
/// **role, not lightness** — `PURPLE_3` stays the most prominent member and
/// so becomes the *darkest* here. See the light block in the CSS for the
/// full rationale.
pub mod brand_light {
    use super::Rgb;

    /// Light `--oc-purple-1`: the dim / secondary label. Least prominent, so
    /// on a light ground it is the lightest of the three.
    pub const PURPLE_1: Rgb = Rgb {
        r: 0x6B,
        g: 0x59,
        b: 0xB6,
    };
    /// Light `--oc-purple-2`: the primary UI accent.
    pub const PURPLE_2: Rgb = Rgb {
        r: 0x5A,
        g: 0x48,
        b: 0xA4,
    };
    /// Light `--oc-purple-3`: the most prominent member (headings, `PRIMARY`),
    /// so on a light ground it is the darkest. `#C5BDED` → `#493B84` is a role
    /// mapping, not a transposition.
    pub const PURPLE_3: Rgb = Rgb {
        r: 0x49,
        g: 0x3B,
        b: 0x84,
    };
    pub const ACCENT_BLUE: Rgb = Rgb {
        r: 0x00,
        g: 0x65,
        b: 0xCB,
    };
    pub const DANGER: Rgb = Rgb {
        r: 0xD0,
        g: 0x0B,
        b: 0x00,
    };
    pub const SUCCESS: Rgb = Rgb {
        r: 0x1B,
        g: 0x77,
        b: 0x32,
    };
    /// rgba(15, 10, 20, 0.94) on #FAFBFD:
    /// r = round(15*0.94 + 250*0.06) = round(29.10) = 29 = 0x1D
    /// g = round(10*0.94 + 251*0.06) = round(24.46) = 24 = 0x18
    /// b = round(20*0.94 + 253*0.06) = round(33.98) = 34 = 0x22
    pub const TEXT: Rgb = Rgb {
        r: 0x1D,
        g: 0x18,
        b: 0x22,
    };
    /// rgba(15, 10, 20, 0.64) on #FAFBFD:
    /// r = round(15*0.64 + 250*0.36) = round(99.60) = 100 = 0x64
    /// g = round(10*0.64 + 251*0.36) = round(96.76) = 97 = 0x61
    /// b = round(20*0.64 + 253*0.36) = round(103.88) = 104 = 0x68
    pub const TEXT_MUTED: Rgb = Rgb {
        r: 0x64,
        g: 0x61,
        b: 0x68,
    };
    /// rgba(15, 10, 20, 0.50) on #FAFBFD. The alpha is 0.50 rather than the
    /// dark stop's 0.42 — see the CSS comment. Every channel lands on an
    /// exact .5 tie, resolved half-away-from-zero to match `f64::round`:
    /// r = round(15*0.50 + 250*0.50) = round(132.50) = 133 = 0x85
    /// g = round(10*0.50 + 251*0.50) = round(130.50) = 131 = 0x83
    /// b = round(20*0.50 + 253*0.50) = round(136.50) = 137 = 0x89
    pub const TEXT_FAINT: Rgb = Rgb {
        r: 0x85,
        g: 0x83,
        b: 0x89,
    };
    /// Light `--oc-surface-0`: the terminal canvas, and the base every
    /// `rgba(...)` token in this module is flattened against.
    pub const SURFACE_0: Rgb = Rgb {
        r: 0xFA,
        g: 0xFB,
        b: 0xFD,
    };
    pub const SURFACE_1: Rgb = Rgb {
        r: 0xF5,
        g: 0xF6,
        b: 0xF9,
    };
    pub const SURFACE_2: Rgb = Rgb {
        r: 0xED,
        g: 0xEE,
        b: 0xF3,
    };
    pub const SURFACE_3: Rgb = Rgb {
        r: 0xE8,
        g: 0xE4,
        b: 0xF5,
    };
    /// `--oc-border-subtle` flattened on #FAFBFD: rgba(0, 0, 0, 0.08):
    /// r = round(250*0.92) = 230 = 0xE6
    /// g = round(251*0.92) = round(230.92) = 231 = 0xE7
    /// b = round(253*0.92) = round(232.76) = 233 = 0xE9
    pub const BORDER_SUBTLE: Rgb = Rgb {
        r: 0xE6,
        g: 0xE7,
        b: 0xE9,
    };
    /// `--oc-border-strong` flattened on #FAFBFD: rgba(0, 0, 0, 0.14):
    /// r = round(250*0.86) = 215 = 0xD7
    /// g = round(251*0.86) = round(215.86) = 216 = 0xD8
    /// b = round(253*0.86) = round(217.58) = 218 = 0xDA
    pub const BORDER_STRONG: Rgb = Rgb {
        r: 0xD7,
        g: 0xD8,
        b: 0xDA,
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
/// `Color::Green` again. `Ready` is consumed by the chat status bar; the
/// full set is consumed by the observe CLI surfaces, which map a closed
/// vocabulary of daemon lifecycle words onto these semantics.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Working,
    Warning,
    Error,
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

/// Resolved value of the global `--color` flag. `Auto` defers to the env
/// conventions (NO_COLOR, CLICOLOR_FORCE, tty + TERM detection); `Always`
/// and `Never` override all of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

static COLOR_CHOICE: OnceLock<ColorChoice> = OnceLock::new();

/// Record the `--color` flag. Must run before the first render — `mode()`
/// caches on first use, so set this right after clap parsing. First call
/// wins; later calls are ignored.
pub fn set_color_choice(choice: ColorChoice) {
    let _ = COLOR_CHOICE.set(choice);
}

// ── Terminal appearance (light / dark background) ──

/// Which background the terminal is drawing on. Selects between the [`brand`]
/// tokens (mirroring the CSS `:root` block) and the [`brand_light`] tokens
/// (mirroring its `prefers-color-scheme: light` block).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Appearance {
    /// Today's behavior, and the fallback whenever detection is inconclusive.
    #[default]
    Dark,
    Light,
}

/// Resolved value of the global `--theme` flag. `Auto` defers to the env
/// conventions (`COVEN_THEME`, then `COLORFGBG`); `Light` and `Dark`
/// override both.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum AppearanceChoice {
    #[default]
    Auto,
    Light,
    Dark,
}

static APPEARANCE: OnceLock<Appearance> = OnceLock::new();

/// Record the resolved terminal appearance. Mirrors [`set_color_choice`]:
/// call it right after clap parsing, before anything renders. First call
/// wins; later calls are ignored.
pub fn set_appearance(appearance: Appearance) {
    let _ = APPEARANCE.set(appearance);
}

/// The process-wide appearance, defaulting to [`Appearance::Dark`].
///
/// Unlike [`mode`], this never lazily detects: resolution is explicit at
/// startup because probing a terminal's background needs raw mode and a
/// read with a timeout, and this function is first touched deep inside
/// rendering. An unresolved appearance reads as `Dark` — today's behavior —
/// and stays overridable by a later [`set_appearance`].
pub fn appearance() -> Appearance {
    APPEARANCE.get().copied().unwrap_or_default()
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct AppearanceEnv<'a> {
    pub coven_theme: Option<&'a str>,
    pub colorfgbg: Option<&'a str>,
}

/// Resolve the terminal appearance, cheapest and most explicit first:
///
/// 1. The `--theme` flag, when it is not `auto`.
/// 2. `COVEN_THEME` = `light` | `dark` | `auto` (case-insensitive).
/// 3. `COLORFGBG`, whose **last** field is the background color index.
/// 4. Otherwise [`Appearance::Dark`].
///
/// Deliberately no OSC 11 query: it needs raw mode and a read with a timeout
/// during startup, and a terminal that never answers would hang the CLI.
pub(crate) fn detect_appearance_from(choice: AppearanceChoice, e: AppearanceEnv<'_>) -> Appearance {
    // 1. The explicit flag outranks every env convention.
    match choice {
        AppearanceChoice::Light => return Appearance::Light,
        AppearanceChoice::Dark => return Appearance::Dark,
        AppearanceChoice::Auto => {}
    }
    // 2. COVEN_THEME. An unrecognized value is inconclusive, not an error:
    //    this runs before any output exists to complain through.
    if let Some(raw) = e.coven_theme {
        match raw.trim().to_ascii_lowercase().as_str() {
            "light" => return Appearance::Light,
            "dark" => return Appearance::Dark,
            _ => {}
        }
    }
    // 3. COLORFGBG. Terminals write "fg;bg" ("15;0") or "fg;extra;bg"
    //    ("15;default;0"), so the background is always the last field.
    if let Some(bg) = e.colorfgbg.and_then(|v| v.rsplit(';').next()) {
        // A literal "default" field means the terminal declined to say.
        if let Ok(index) = bg.trim().parse::<u8>() {
            match index {
                // The ANSI dark half: black, the six dark hues, and bright
                // black (8) which every scheme keeps dark.
                0..=6 | 8 => return Appearance::Dark,
                // 7 is light grey; 9..=15 are the bright half.
                7 | 9..=15 => return Appearance::Light,
                // Anything above the 16-color range tells us nothing about
                // brightness without the terminal's palette.
                _ => {}
            }
        }
    }
    // 4. Inconclusive — keep today's behavior.
    Appearance::Dark
}

/// Resolve the appearance from this process's environment.
pub fn detect_appearance(choice: AppearanceChoice) -> Appearance {
    let coven_theme = std::env::var("COVEN_THEME").ok();
    let colorfgbg = std::env::var("COLORFGBG").ok();
    detect_appearance_from(
        choice,
        AppearanceEnv {
            coven_theme: coven_theme.as_deref(),
            colorfgbg: colorfgbg.as_deref(),
        },
    )
}

/// Map a dark-appearance token value onto the active appearance.
///
/// The mapping is **value-keyed, not name-keyed**: it matches on the RGB
/// triple, so every semantic token sharing a brand value is remapped
/// together. `PRIMARY_STRONG`, `AGENT_LABEL` and `SYNTAX_KEYWORD` are all
/// `brand::PURPLE_2` and all three land on `brand_light::PURPLE_2`. That is
/// intended, and it is what keeps this a three-callsite change: the brand
/// defines its light block per *brand* token, not per semantic alias, and a
/// name-keyed map would need every callsite to pass a semantic identity it
/// does not carry.
///
/// A value with no light counterpart passes through unchanged. Every
/// `brand` token has one today, which `every_brand_token_has_a_light_counterpart`
/// keeps true.
pub(crate) fn for_appearance(c: Rgb, appearance: Appearance) -> Rgb {
    match appearance {
        Appearance::Dark => c,
        Appearance::Light => match c {
            brand::PURPLE_1 => brand_light::PURPLE_1,
            brand::PURPLE_2 => brand_light::PURPLE_2,
            brand::PURPLE_3 => brand_light::PURPLE_3,
            brand::ACCENT_BLUE => brand_light::ACCENT_BLUE,
            brand::DANGER => brand_light::DANGER,
            brand::SUCCESS => brand_light::SUCCESS,
            brand::TEXT => brand_light::TEXT,
            brand::TEXT_MUTED => brand_light::TEXT_MUTED,
            brand::TEXT_FAINT => brand_light::TEXT_FAINT,
            brand::SURFACE_0 => brand_light::SURFACE_0,
            brand::SURFACE_1 => brand_light::SURFACE_1,
            brand::SURFACE_2 => brand_light::SURFACE_2,
            brand::SURFACE_3 => brand_light::SURFACE_3,
            brand::BORDER_SUBTLE => brand_light::BORDER_SUBTLE,
            brand::BORDER_STRONG => brand_light::BORDER_STRONG,
            other => other,
        },
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct EnvInputs<'a> {
    pub no_color: Option<&'a str>,
    pub clicolor_force: Option<&'a str>,
    pub colorterm: Option<&'a str>,
    pub term: Option<&'a str>,
    pub stdout_is_tty: bool,
}

pub(crate) fn detect_mode_from(choice: ColorChoice, e: EnvInputs<'_>) -> TerminalMode {
    // 0. The explicit --color flag outranks every env convention.
    match choice {
        ColorChoice::Never => return TerminalMode::NoColor,
        ColorChoice::Always => return forced_mode(e),
        ColorChoice::Auto => {}
    }
    // 1. NO_COLOR set to non-empty value wins among the env conventions
    //    (per no-color.org; empty-string treated as unset per
    //    supports-color convention).
    if e.no_color.map(|v| !v.is_empty()).unwrap_or(false) {
        return TerminalMode::NoColor;
    }
    // 2. CLICOLOR_FORCE set to anything but "" or "0" forces escapes even
    //    when stdout is piped (https://bixense.com/clicolors/).
    if e.clicolor_force
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        return forced_mode(e);
    }
    // 3. Piped/redirected stdout — never emit escapes.
    if !e.stdout_is_tty {
        return TerminalMode::NoColor;
    }
    // 4. Explicit truecolor declaration.
    match e.colorterm {
        Some("truecolor") | Some("24bit") => return TerminalMode::TrueColor,
        _ => {}
    }
    // 5. TERM-based fallback.
    match e.term {
        Some(t) if t.ends_with("-direct") => TerminalMode::TrueColor,
        Some(t) if t.ends_with("-256color") => TerminalMode::Indexed256,
        Some("dumb") | None => TerminalMode::NoColor,
        Some(_) => TerminalMode::Indexed256,
    }
}

/// Mode when color is forced (`--color=always` or CLICOLOR_FORCE): honor a
/// declared truecolor terminal, otherwise fall back to indexed 256 — piped
/// output has no tty to probe, and forcing means the caller wants escapes.
fn forced_mode(e: EnvInputs<'_>) -> TerminalMode {
    match e.colorterm {
        Some("truecolor") | Some("24bit") => return TerminalMode::TrueColor,
        _ => {}
    }
    match e.term {
        Some(t) if t.ends_with("-direct") => TerminalMode::TrueColor,
        _ => TerminalMode::Indexed256,
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
    let clicolor_force = std::env::var("CLICOLOR_FORCE").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    let term = std::env::var("TERM").ok();
    detect_mode_from(
        COLOR_CHOICE.get().copied().unwrap_or_default(),
        EnvInputs {
            no_color: no_color.as_deref(),
            clicolor_force: clicolor_force.as_deref(),
            colorterm: colorterm.as_deref(),
            term: term.as_deref(),
            stdout_is_tty: std::io::stdout().is_terminal(),
        },
    )
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
    ratatui_color_for(c, m, appearance())
}

/// The appearance-explicit form. One of the three adapter choke points every
/// token flows through, so remapping here leaves all callsites untouched.
pub(crate) fn ratatui_color_for(c: Rgb, m: TerminalMode, a: Appearance) -> RatColor {
    match m {
        TerminalMode::TrueColor => {
            let c = for_appearance(c, a);
            RatColor::Rgb(c.r, c.g, c.b)
        }
        TerminalMode::Indexed256 => RatColor::Indexed(nearest_256(for_appearance(c, a))),
        // No color is emitted at all, so there is nothing to remap: light
        // appearance must never apply here.
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
    appearance: Appearance,
}

// `Bg` is pre-wired alongside `Fg` for symmetry; full backgrounds via ANSI
// escape land in a later phase (currently main.rs uses ratatui Style::bg).
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub struct Bg {
    rgb: Rgb,
    mode: TerminalMode,
    appearance: Appearance,
}

#[derive(Copy, Clone, Debug)]
pub struct Reset {
    mode: TerminalMode,
}

impl Fg {
    /// Escape for `mode`, in the process-wide [`appearance`].
    pub(crate) fn with_mode(rgb: Rgb, mode: TerminalMode) -> Self {
        Self {
            rgb,
            mode,
            appearance: appearance(),
        }
    }
    /// Escape for an explicit appearance. Test-only: production renderers
    /// resolve the appearance once at startup and go through `with_mode`.
    #[cfg(test)]
    pub(crate) fn with_mode_and_appearance(
        rgb: Rgb,
        mode: TerminalMode,
        appearance: Appearance,
    ) -> Self {
        Self {
            rgb,
            mode,
            appearance,
        }
    }
}
#[allow(dead_code)]
impl Bg {
    pub(crate) fn with_mode(rgb: Rgb, mode: TerminalMode) -> Self {
        Self {
            rgb,
            mode,
            appearance: appearance(),
        }
    }
    #[cfg(test)]
    pub(crate) fn with_mode_and_appearance(
        rgb: Rgb,
        mode: TerminalMode,
        appearance: Appearance,
    ) -> Self {
        Self {
            rgb,
            mode,
            appearance,
        }
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
                let c = for_appearance(self.rgb, self.appearance);
                write!(f, "\x1b[38;2;{};{};{}m", c.r, c.g, c.b)
            }
            TerminalMode::Indexed256 => write!(
                f,
                "\x1b[38;5;{}m",
                nearest_256(for_appearance(self.rgb, self.appearance))
            ),
            // No escape is emitted, so there is nothing to remap.
            TerminalMode::NoColor => Ok(()),
        }
    }
}

impl fmt::Display for Bg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            TerminalMode::TrueColor => {
                let c = for_appearance(self.rgb, self.appearance);
                write!(f, "\x1b[48;2;{};{};{}m", c.r, c.g, c.b)
            }
            TerminalMode::Indexed256 => write!(
                f,
                "\x1b[48;5;{}m",
                nearest_256(for_appearance(self.rgb, self.appearance))
            ),
            // No escape is emitted, so there is nothing to remap.
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

impl Palette {
    /// Palette that renders no escapes at all. The plain-text entry points
    /// (notably `observe::view_text`, whose output is fed to ratatui, which
    /// does not interpret ANSI) take this so their bytes stay escape-free.
    pub fn plain() -> Self {
        palette_for(TerminalMode::NoColor)
    }

    /// Foreground escape for an arbitrary brand token in this palette's mode.
    /// Keeps callers from reaching for `Fg::with_mode` and re-deriving the
    /// mode by hand.
    pub fn tint(self, rgb: Rgb) -> Fg {
        Fg::with_mode(rgb, self.mode)
    }

    /// Foreground escape for a status semantic — the only sanctioned way for
    /// a renderer to color "running" / "failed" / "idle".
    pub fn status(self, status: Status) -> Fg {
        self.tint(status_token(status))
    }
}

/// Palette for the active terminal mode.
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

    /// Parse `rgba(r, g, b, a)` and composite it over `base` — the same
    /// appearance's `--oc-surface-0`: each channel becomes
    /// round(channel * a + base * (1 - a)).
    ///
    /// With a black `base` the `base * (1 - a)` term is exactly zero, so this
    /// reduces to the dark convention round(channel * a) that `brand`
    /// documents, bit for bit.
    fn flatten_on(rgba: &str, base: Rgb) -> Rgb {
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
        let flat = |c: u16, base: u8| (c as f64 * a + base as f64 * (1.0 - a)).round() as u8;
        Rgb {
            r: flat(r, base.r),
            g: flat(g, base.g),
            b: flat(b, base.b),
        }
    }

    const LIGHT_MARKER: &str = "@media (prefers-color-scheme: light)";

    /// Remove every `/* ... */` span, including multi-line ones, so the
    /// scanners below only ever see declarations. Both blocks carry prose
    /// rationale that would otherwise be scanned for tokens.
    fn strip_css_comments(css: &str) -> String {
        let mut out = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(start) = rest.find("/*") {
            out.push_str(&rest[..start]);
            match rest[start + 2..].find("*/") {
                Some(end) => rest = &rest[start + 2 + end + 2..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    /// The brace-matched body of the `prefers-color-scheme: light` block.
    fn light_block(css: &str) -> String {
        let start = css.find(LIGHT_MARKER).expect("light block is present");
        let open = start
            + css[start..]
                .find('{')
                .expect("light block opens with a brace");
        let mut depth = 0usize;
        for (offset, ch) in css[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return css[open + 1..open + offset].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("light block never closes");
    }

    /// The `:root` defaults — everything ahead of the light block.
    fn root_vars(css: &str) -> HashMap<String, String> {
        let stripped = strip_css_comments(css);
        let cut = stripped.find(LIGHT_MARKER).expect("light block is present");
        parse_css_vars(&stripped[..cut])
    }

    /// The `prefers-color-scheme: light` overrides.
    fn light_vars(css: &str) -> HashMap<String, String> {
        parse_css_vars(&light_block(&strip_css_comments(css)))
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
        let vars = root_vars(css);

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
            flatten_on(&vars["--oc-text"], brand::SURFACE_0),
            "--oc-text"
        );
        assert_eq!(
            brand::TEXT_MUTED,
            flatten_on(&vars["--oc-text-muted"], brand::SURFACE_0),
            "--oc-text-muted"
        );
        assert_eq!(
            brand::TEXT_FAINT,
            flatten_on(&vars["--oc-text-faint"], brand::SURFACE_0),
            "--oc-text-faint"
        );

        assert_eq!(
            brand::BORDER_SUBTLE,
            flatten_on(&vars["--oc-border-subtle"], brand::SURFACE_0),
            "--oc-border-subtle"
        );
        assert_eq!(
            brand::BORDER_STRONG,
            flatten_on(&vars["--oc-border-strong"], brand::SURFACE_0),
            "--oc-border-strong"
        );
    }

    #[test]
    fn brand_light_tokens_mirror_the_css_light_block() {
        let css = include_str!("../../../brand/ui/color-tokens.css");
        let vars = light_vars(css);

        // Hex overrides carry straight across.
        for (token, name) in [
            (brand_light::PURPLE_1, "--oc-purple-1"),
            (brand_light::PURPLE_2, "--oc-purple-2"),
            (brand_light::PURPLE_3, "--oc-purple-3"),
            (brand_light::ACCENT_BLUE, "--oc-accent-blue"),
            (brand_light::DANGER, "--oc-danger"),
            (brand_light::SUCCESS, "--oc-success"),
            (brand_light::SURFACE_0, "--oc-surface-0"),
            (brand_light::SURFACE_1, "--oc-surface-1"),
            (brand_light::SURFACE_2, "--oc-surface-2"),
            (brand_light::SURFACE_3, "--oc-surface-3"),
        ] {
            assert_eq!(token, rgb_from_hex(&vars[name]), "{name}");
        }

        // `rgba(...)` overrides flatten against the *light* --oc-surface-0,
        // exactly as the dark ones flatten against the black one.
        for (token, name) in [
            (brand_light::TEXT, "--oc-text"),
            (brand_light::TEXT_MUTED, "--oc-text-muted"),
            (brand_light::TEXT_FAINT, "--oc-text-faint"),
            (brand_light::BORDER_SUBTLE, "--oc-border-subtle"),
            (brand_light::BORDER_STRONG, "--oc-border-strong"),
        ] {
            assert_eq!(
                token,
                flatten_on(&vars[name], brand_light::SURFACE_0),
                "{name}"
            );
        }
    }

    /// The light block must override every brand token the TUI can render,
    /// or `for_appearance` would pass a dark value through onto a light
    /// ground — the exact defect this appearance layer exists to prevent.
    #[test]
    fn every_brand_token_has_a_light_counterpart() {
        for (name, dark) in ALL_BRAND_TOKENS {
            let light = for_appearance(*dark, Appearance::Light);
            assert_ne!(
                light, *dark,
                "{name} has no light counterpart: for_appearance passed the \
                 dark value {dark:?} straight through"
            );
        }
    }

    #[test]
    fn web_asset_css_light_block_matches_the_brand_source() {
        // `web/assets/color-tokens.css` is a copy of the brand file, and the
        // page under `web/` imports it. If the two light blocks drift, the
        // site and the CLI disagree about the palette.
        let brand_css = include_str!("../../../brand/ui/color-tokens.css");
        let web_css = include_str!("../../../web/assets/color-tokens.css");
        assert_eq!(
            light_block(&strip_css_comments(brand_css)),
            light_block(&strip_css_comments(web_css)),
            "web/assets/color-tokens.css light block drifted from brand/ui/color-tokens.css",
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
    fn rgb_lerp_clamps_and_hits_both_endpoints() {
        let a = Rgb {
            r: 0,
            g: 10,
            b: 200,
        };
        let b = Rgb {
            r: 200,
            g: 20,
            b: 0,
        };
        assert_eq!(a.lerp(b, 0.0), a, "t=0 is the start token");
        assert_eq!(a.lerp(b, 1.0), b, "t=1 is the end token");
        assert_eq!(a.lerp(b, -5.0), a, "t below range clamps to the start");
        assert_eq!(a.lerp(b, 5.0), b, "t above range clamps to the end");
        let mid = a.lerp(b, 0.5);
        assert_eq!(
            mid,
            Rgb {
                r: 100,
                g: 15,
                b: 100
            },
            "midpoint rounds per channel"
        );
    }

    #[test]
    fn rgb_lerp_is_monotonic_across_the_brand_ramp() {
        // The masthead ramp must never step backwards, or the crown reads as
        // banded rather than lit.
        let steps: Vec<Rgb> = (0..5)
            .map(|i| brand::PURPLE_1.lerp(brand::PURPLE_3, i as f32 / 4.0))
            .collect();
        for pair in steps.windows(2) {
            assert!(
                pair[1].r >= pair[0].r && pair[1].g >= pair[0].g && pair[1].b >= pair[0].b,
                "ramp must not step backwards: {pair:?}"
            );
        }
        assert_eq!(steps[0], brand::PURPLE_1);
        assert_eq!(steps[4], brand::PURPLE_3);
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
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
                    clicolor_force: None,
                    colorterm: Some("truecolor"),
                    term: Some("xterm-256color"),
                    stdout_is_tty: false,
                },
                NoColor,
            ),
            (
                EnvInputs {
                    no_color: Some("1"),
                    clicolor_force: None,
                    colorterm: None,
                    term: None,
                    stdout_is_tty: false,
                },
                NoColor,
            ),
        ];
        for (i, (inputs, expected)) in cases.iter().enumerate() {
            assert_eq!(
                detect_mode_from(ColorChoice::Auto, *inputs),
                *expected,
                "row {i}: {inputs:?}",
            );
        }
    }

    #[test]
    fn clicolor_force_overrides_piped_stdout_but_not_no_color() {
        use TerminalMode::*;
        let piped_truecolor = EnvInputs {
            no_color: None,
            clicolor_force: Some("1"),
            colorterm: Some("truecolor"),
            term: None,
            stdout_is_tty: false,
        };
        assert_eq!(
            detect_mode_from(ColorChoice::Auto, piped_truecolor),
            TrueColor
        );

        let piped_plain = EnvInputs {
            no_color: None,
            clicolor_force: Some("1"),
            colorterm: None,
            term: None,
            stdout_is_tty: false,
        };
        // Forcing with no terminal declaration falls back to indexed 256.
        assert_eq!(detect_mode_from(ColorChoice::Auto, piped_plain), Indexed256);

        // "0" and "" mean unset per the CLICOLOR convention.
        for off in ["0", ""] {
            let inputs = EnvInputs {
                clicolor_force: Some(off),
                ..piped_plain
            };
            assert_eq!(detect_mode_from(ColorChoice::Auto, inputs), NoColor);
        }

        // NO_COLOR still wins over CLICOLOR_FORCE.
        let conflicted = EnvInputs {
            no_color: Some("1"),
            ..piped_truecolor
        };
        assert_eq!(detect_mode_from(ColorChoice::Auto, conflicted), NoColor);
    }

    #[test]
    fn color_flag_outranks_every_env_convention() {
        use TerminalMode::*;
        let colorful_tty = EnvInputs {
            no_color: None,
            clicolor_force: None,
            colorterm: Some("truecolor"),
            term: Some("xterm-256color"),
            stdout_is_tty: true,
        };
        assert_eq!(detect_mode_from(ColorChoice::Never, colorful_tty), NoColor);

        let no_color_piped = EnvInputs {
            no_color: Some("1"),
            clicolor_force: None,
            colorterm: Some("truecolor"),
            term: None,
            stdout_is_tty: false,
        };
        assert_eq!(
            detect_mode_from(ColorChoice::Always, no_color_piped),
            TrueColor
        );
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

    // ── Appearance ──

    /// Every token in `brand`, which is exactly the set `for_appearance`
    /// must cover.
    const ALL_BRAND_TOKENS: &[(&str, Rgb)] = &[
        ("PURPLE_1", brand::PURPLE_1),
        ("PURPLE_2", brand::PURPLE_2),
        ("PURPLE_3", brand::PURPLE_3),
        ("ACCENT_BLUE", brand::ACCENT_BLUE),
        ("DANGER", brand::DANGER),
        ("SUCCESS", brand::SUCCESS),
        ("TEXT", brand::TEXT),
        ("TEXT_MUTED", brand::TEXT_MUTED),
        ("TEXT_FAINT", brand::TEXT_FAINT),
        ("SURFACE_0", brand::SURFACE_0),
        ("SURFACE_1", brand::SURFACE_1),
        ("SURFACE_2", brand::SURFACE_2),
        ("SURFACE_3", brand::SURFACE_3),
        ("BORDER_SUBTLE", brand::BORDER_SUBTLE),
        ("BORDER_STRONG", brand::BORDER_STRONG),
    ];

    const ALL_MODES: [TerminalMode; 3] = [
        TerminalMode::TrueColor,
        TerminalMode::Indexed256,
        TerminalMode::NoColor,
    ];

    #[test]
    fn appearance_defaults_to_dark() {
        // No test calls `set_appearance` — it writes a process-wide OnceLock
        // that every other test in this binary would inherit. Appearance is
        // threaded explicitly through the `*_and_appearance` constructors
        // instead, which keeps this assertion meaningful.
        assert_eq!(Appearance::default(), Appearance::Dark);
        assert_eq!(appearance(), Appearance::Dark);
    }

    #[test]
    fn detect_appearance_precedence_table() {
        use Appearance::*;
        let cases: &[(AppearanceChoice, Option<&str>, Option<&str>, Appearance)] = &[
            // 1. The flag outranks both env vars.
            (AppearanceChoice::Light, Some("dark"), Some("15;0"), Light),
            (AppearanceChoice::Dark, Some("light"), Some("0;15"), Dark),
            // 2. COVEN_THEME outranks COLORFGBG.
            (AppearanceChoice::Auto, Some("light"), Some("15;0"), Light),
            (AppearanceChoice::Auto, Some("dark"), Some("0;15"), Dark),
            // Case and surrounding space are not significant.
            (AppearanceChoice::Auto, Some("  LIGHT "), None, Light),
            // COVEN_THEME=auto defers to COLORFGBG.
            (AppearanceChoice::Auto, Some("auto"), Some("0;15"), Light),
            (AppearanceChoice::Auto, Some("auto"), Some("15;0"), Dark),
            // An unrecognized COVEN_THEME is inconclusive, not fatal.
            (AppearanceChoice::Auto, Some("mauve"), Some("0;15"), Light),
            (AppearanceChoice::Auto, Some(""), Some("0;15"), Light),
            // 3. COLORFGBG: the LAST field is the background.
            (AppearanceChoice::Auto, None, Some("15;0"), Dark),
            (AppearanceChoice::Auto, None, Some("0;15"), Light),
            // The three-field form some terminals emit.
            (AppearanceChoice::Auto, None, Some("15;default;0"), Dark),
            (AppearanceChoice::Auto, None, Some("0;default;15"), Light),
            // 0-6 and 8 are the dark half; 7 and 9-15 the light half.
            (AppearanceChoice::Auto, None, Some("7;0"), Dark),
            (AppearanceChoice::Auto, None, Some("7;6"), Dark),
            (AppearanceChoice::Auto, None, Some("7;8"), Dark),
            (AppearanceChoice::Auto, None, Some("0;7"), Light),
            (AppearanceChoice::Auto, None, Some("0;9"), Light),
            // A literal "default" background is inconclusive.
            (AppearanceChoice::Auto, None, Some("15;default"), Dark),
            // So is a 256-color index, junk, or an absent variable.
            (AppearanceChoice::Auto, None, Some("15;200"), Dark),
            (AppearanceChoice::Auto, None, Some("nonsense"), Dark),
            (AppearanceChoice::Auto, None, Some(""), Dark),
            (AppearanceChoice::Auto, None, None, Dark),
        ];
        for (index, (choice, coven_theme, colorfgbg, expected)) in cases.iter().enumerate() {
            let env = AppearanceEnv {
                coven_theme: *coven_theme,
                colorfgbg: *colorfgbg,
            };
            assert_eq!(
                detect_appearance_from(*choice, env),
                *expected,
                "row {index}: {choice:?} {env:?}",
            );
        }
    }

    /// The pre-appearance adapter bodies, transcribed verbatim. The dark
    /// path must still produce exactly these bytes.
    fn legacy_ratatui_color(c: Rgb, m: TerminalMode) -> RatColor {
        match m {
            TerminalMode::TrueColor => RatColor::Rgb(c.r, c.g, c.b),
            TerminalMode::Indexed256 => RatColor::Indexed(nearest_256(c)),
            TerminalMode::NoColor => RatColor::Reset,
        }
    }
    fn legacy_fg(c: Rgb, m: TerminalMode) -> String {
        match m {
            TerminalMode::TrueColor => format!("\x1b[38;2;{};{};{}m", c.r, c.g, c.b),
            TerminalMode::Indexed256 => format!("\x1b[38;5;{}m", nearest_256(c)),
            TerminalMode::NoColor => String::new(),
        }
    }
    fn legacy_bg(c: Rgb, m: TerminalMode) -> String {
        match m {
            TerminalMode::TrueColor => format!("\x1b[48;2;{};{};{}m", c.r, c.g, c.b),
            TerminalMode::Indexed256 => format!("\x1b[48;5;{}m", nearest_256(c)),
            TerminalMode::NoColor => String::new(),
        }
    }

    #[test]
    fn dark_appearance_is_byte_identical_to_the_pre_appearance_adapters() {
        for (name, token) in ALL_BRAND_TOKENS {
            assert_eq!(
                for_appearance(*token, Appearance::Dark),
                *token,
                "{name} must pass through unchanged in dark appearance"
            );
            for mode in ALL_MODES {
                assert_eq!(
                    ratatui_color_for(*token, mode, Appearance::Dark),
                    legacy_ratatui_color(*token, mode),
                    "{name} ratatui color drifted in {mode:?}"
                );
                assert_eq!(
                    format!(
                        "{}",
                        Fg::with_mode_and_appearance(*token, mode, Appearance::Dark)
                    ),
                    legacy_fg(*token, mode),
                    "{name} fg escape drifted in {mode:?}"
                );
                assert_eq!(
                    format!(
                        "{}",
                        Bg::with_mode_and_appearance(*token, mode, Appearance::Dark)
                    ),
                    legacy_bg(*token, mode),
                    "{name} bg escape drifted in {mode:?}"
                );
            }
        }
    }

    #[test]
    fn light_appearance_remaps_every_adapter() {
        use ratatui::style::Color;
        // PRIMARY is brand::PURPLE_3 (#C5BDED), which maps to #493B84.
        assert_eq!(
            ratatui_color_for(PRIMARY, TerminalMode::TrueColor, Appearance::Light),
            Color::Rgb(0x49, 0x3B, 0x84),
        );
        assert_eq!(
            format!(
                "{}",
                Fg::with_mode_and_appearance(PRIMARY, TerminalMode::TrueColor, Appearance::Light)
            ),
            "\x1b[38;2;73;59;132m",
        );
        assert_eq!(
            format!(
                "{}",
                Bg::with_mode_and_appearance(SURFACE, TerminalMode::TrueColor, Appearance::Light)
            ),
            // SURFACE is brand::SURFACE_1 (#0F0A14) -> #F5F6F9.
            "\x1b[48;2;245;246;249m",
        );
        // The 256-color downgrade runs on the remapped value, not the dark one.
        assert_eq!(
            ratatui_color_for(PRIMARY, TerminalMode::Indexed256, Appearance::Light),
            Color::Indexed(nearest_256(brand_light::PURPLE_3)),
        );
        assert_ne!(
            ratatui_color_for(PRIMARY, TerminalMode::Indexed256, Appearance::Light),
            ratatui_color_for(PRIMARY, TerminalMode::Indexed256, Appearance::Dark),
        );
    }

    /// Tokens sharing one brand value are remapped together — the mapping is
    /// keyed on the value, not the semantic name.
    #[test]
    fn value_keyed_mapping_moves_aliases_together() {
        assert_eq!(PRIMARY_STRONG, AGENT_LABEL, "precondition: both PURPLE_2");
        assert_eq!(SYNTAX_KEYWORD, AGENT_LABEL, "precondition: both PURPLE_2");
        for token in [PRIMARY_STRONG, AGENT_LABEL, SYNTAX_KEYWORD] {
            assert_eq!(
                for_appearance(token, Appearance::Light),
                brand_light::PURPLE_2
            );
        }
    }

    #[test]
    fn no_color_mode_never_applies_the_light_palette() {
        use ratatui::style::Color;
        for (name, token) in ALL_BRAND_TOKENS {
            // No escape exists to remap, so both appearances must agree and
            // both must stay empty.
            assert_eq!(
                ratatui_color_for(*token, TerminalMode::NoColor, Appearance::Light),
                Color::Reset,
                "{name}"
            );
            for appearance in [Appearance::Dark, Appearance::Light] {
                assert_eq!(
                    format!(
                        "{}",
                        Fg::with_mode_and_appearance(*token, TerminalMode::NoColor, appearance)
                    ),
                    "",
                    "{name} fg in {appearance:?}"
                );
                assert_eq!(
                    format!(
                        "{}",
                        Bg::with_mode_and_appearance(*token, TerminalMode::NoColor, appearance)
                    ),
                    "",
                    "{name} bg in {appearance:?}"
                );
            }
        }
    }

    // ── Contrast (DESIGN.md section 3: "all text must meet WCAG AA minimums") ──

    /// WCAG 2.1 relative luminance.
    fn relative_luminance(c: Rgb) -> f64 {
        fn channel(v: u8) -> f64 {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }

    /// WCAG 2.1 contrast ratio, in 1.0..=21.0.
    fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// WCAG AA for normal-size text. Terminal cells are never "large text",
    /// so the 3:1 large-text allowance never applies here.
    const AA_NORMAL: f64 = 4.5;

    /// The faint tier's floor.
    ///
    /// `TEXT_FAINT` is deliberately below AA in *both* appearances. The dark
    /// token has shipped at 3.67:1 on `SURFACE_0` since the theme module
    /// landed, and holding only the light one to 4.5 would invent a stricter
    /// bar for one appearance — and collapse faint into muted, since the
    /// alpha needed (0.58) lands within ~2% luminance of `TEXT_MUTED` and
    /// destroys the tier. Both appearances answer to this same bar instead.
    const FAINT_FLOOR: f64 = 3.0;

    /// Surfaces text is actually drawn on. `SURFACE_3` is excluded on
    /// purpose: its only semantic alias is `SCROLL_TRACK`, and its only use
    /// is the scrollbar track style in `tui/chat/render.rs` — recessed
    /// chrome, never a text ground.
    fn text_surfaces(appearance: Appearance) -> [(&'static str, Rgb); 3] {
        [
            ("SURFACE_0", for_appearance(brand::SURFACE_0, appearance)),
            ("SURFACE_1", for_appearance(brand::SURFACE_1, appearance)),
            ("SURFACE_2", for_appearance(brand::SURFACE_2, appearance)),
        ]
    }

    /// Tokens rendered as text, with the bar each must clear.
    const TEXT_TOKENS: &[(&str, Rgb, f64)] = &[
        ("TEXT", brand::TEXT, AA_NORMAL),
        ("TEXT_MUTED", brand::TEXT_MUTED, AA_NORMAL),
        ("TEXT_FAINT", brand::TEXT_FAINT, FAINT_FLOOR),
        ("PURPLE_1", brand::PURPLE_1, AA_NORMAL),
        ("PURPLE_2", brand::PURPLE_2, AA_NORMAL),
        ("PURPLE_3", brand::PURPLE_3, AA_NORMAL),
        ("ACCENT_BLUE", brand::ACCENT_BLUE, AA_NORMAL),
        ("DANGER", brand::DANGER, AA_NORMAL),
        ("SUCCESS", brand::SUCCESS, AA_NORMAL),
    ];

    /// The one pre-existing sub-AA pair, recorded as data so a *new* one
    /// cannot slip in behind a blanket-weakened bar.
    ///
    /// `brand::PURPLE_1` (#7A6DAA) is `USER_LABEL`. It measures 4.60 / 4.28 /
    /// 3.83 against dark `SURFACE_0` / `SURFACE_1` / `SURFACE_2`, so it has
    /// been below AA on the two panel surfaces since the theme module
    /// landed. Raising it means changing a *dark* brand value, which would
    /// break the byte-identical dark path this module asserts, so it is
    /// tracked separately rather than fixed here. The light appearance has
    /// no entries.
    const LEGACY_SUB_AA: &[(Appearance, Rgb)] = &[(Appearance::Dark, brand::PURPLE_1)];

    #[test]
    fn text_tokens_clear_their_contrast_bar_in_both_appearances() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            for (token_name, token, bar) in TEXT_TOKENS {
                if LEGACY_SUB_AA.contains(&(appearance, *token)) {
                    continue;
                }
                let fg = for_appearance(*token, appearance);
                for (surface_name, surface) in text_surfaces(appearance) {
                    let ratio = contrast_ratio(fg, surface);
                    assert!(
                        ratio >= *bar,
                        "{appearance:?} {token_name} on {surface_name}: {ratio:.2}:1 \
                         is below the {bar:.1}:1 bar",
                    );
                }
            }
        }
    }

    /// The exception list must stay exactly one entry, and that entry must
    /// still be a real exception — otherwise it rots into a silent carve-out.
    #[test]
    fn legacy_sub_aa_exception_list_is_exactly_the_known_pair() {
        assert_eq!(
            LEGACY_SUB_AA,
            &[(Appearance::Dark, brand::PURPLE_1)],
            "the sub-AA exception list changed; a new sub-AA token needs a \
             decision, not an entry here",
        );
        for (appearance, token) in LEGACY_SUB_AA {
            let fg = for_appearance(*token, *appearance);
            let worst = text_surfaces(*appearance)
                .into_iter()
                .map(|(_, surface)| contrast_ratio(fg, surface))
                .fold(f64::INFINITY, f64::min);
            assert!(
                worst < AA_NORMAL,
                "{appearance:?} {token:?} now measures {worst:.2}:1 and clears AA — \
                 drop it from LEGACY_SUB_AA",
            );
        }
    }

    #[test]
    fn contrast_ratio_matches_known_wcag_reference_values() {
        // Guards the formula itself: these three are fixed points of WCAG 2.1.
        let white = rgb_from_hex("#FFFFFF");
        let black = rgb_from_hex("#000000");
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.001);
        // #767676 on white is the canonical "just passes AA" grey.
        assert!((contrast_ratio(rgb_from_hex("#767676"), white) - 4.54).abs() < 0.01);
    }

    /// Prints the measured ratio table. Not an assertion — the bars are
    /// enforced by `text_tokens_clear_their_contrast_bar_in_both_appearances`
    /// — but it is how the numbers quoted in the CSS comments and in brand
    /// review get regenerated after any palette change:
    ///
    /// ```text
    /// cargo test -p coven-cli --bin coven contrast_report -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "reporting helper, not a gate"]
    fn contrast_report() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            println!("\n{appearance:?} (ratios vs SURFACE_0 / SURFACE_1 / SURFACE_2)");
            for (token_name, token, bar) in TEXT_TOKENS {
                let fg = for_appearance(*token, appearance);
                let ratios: Vec<String> = text_surfaces(appearance)
                    .into_iter()
                    .map(|(_, surface)| format!("{:>5.2}", contrast_ratio(fg, surface)))
                    .collect();
                let worst = text_surfaces(appearance)
                    .into_iter()
                    .map(|(_, surface)| contrast_ratio(fg, surface))
                    .fold(f64::INFINITY, f64::min);
                let verdict = if LEGACY_SUB_AA.contains(&(appearance, *token)) {
                    "LEGACY EXCEPTION"
                } else if worst >= *bar {
                    "ok"
                } else {
                    "FAIL"
                };
                println!(
                    "  {token_name:<12} #{:02X}{:02X}{:02X}  {}  bar {bar:.1}  {verdict}",
                    fg.r,
                    fg.g,
                    fg.b,
                    ratios.join("  "),
                );
            }
        }
    }
}
