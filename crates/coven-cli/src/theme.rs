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
}
