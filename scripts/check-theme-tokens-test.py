#!/usr/bin/env python3
"""Tests for check-theme-tokens.py."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "check_theme_tokens", ROOT / "scripts" / "check-theme-tokens.py"
)
assert spec and spec.loader
check_theme_tokens = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_theme_tokens)


class ThemeTokenGuardTests(unittest.TestCase):
    def test_hand_rolled_rgb_struct_is_blocked(self) -> None:
        text = "const MINE: Rgb = Rgb { r: 1, g: 2, b: 3 };"

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 1, "raw_rgb_struct")])

    def test_inline_ratatui_color_is_blocked(self) -> None:
        text = "let c = Color::Rgb(0x9A, 0x8E, 0xCD);"

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 1, "raw_ratatui_color")])

    def test_aliased_ratatui_color_is_blocked(self) -> None:
        text = "let c = RatColor::Rgb(1, 2, 3);"

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 1, "raw_ratatui_color")])

    def test_hand_assembled_sgr_escape_is_blocked(self) -> None:
        text = 'write!(f, "\\x1b[38;2;{r};{g};{b}m")'

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 1, "raw_ansi_truecolor")])

    def test_escape_assertions_inside_a_test_module_are_allowed(self) -> None:
        # A test asserting that a rendered frame carries a token's escape is
        # the invariant working, not breaking.
        text = "\n".join(
            [
                "fn render() -> String { theme::fg(theme::PRIMARY).to_string() }",
                "#[cfg(test)]",
                "mod tests {",
                '    assert!(frame.contains("\\x1b[38;2;154;142;205m"));',
                "}",
            ]
        )

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [])

    def test_hand_assembled_escape_in_production_code_is_still_blocked(self) -> None:
        text = "\n".join(
            [
                'print!("\\x1b[38;2;255;0;0mred");',
                "#[cfg(test)]",
                "mod tests {}",
            ]
        )

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 1, "raw_ansi_truecolor")])

    def test_rgb_struct_is_blocked_even_inside_tests(self) -> None:
        # Only the escape rule is test-exempt; a hand-rolled token is a
        # forked palette wherever it lives.
        text = "#[cfg(test)]\nmod tests {\n let c = Rgb { r: 1, g: 2, b: 3 };\n}"

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 3, "raw_rgb_struct")])

    def test_prose_about_escapes_in_a_comment_is_allowed(self) -> None:
        # Verbatim shape of the doc comment on `observe.rs::view_text`, which
        # quotes an escape to explain what leaks into a ratatui transcript.
        text = "\n".join(
            [
                "/// ratatui renders the bytes it is handed without interpreting",
                "/// ANSI -- an escape here reaches the user as literal",
                "/// `[38;2;154;142;205m` garbage.",
                "//! module note about Color::Rgb(1, 2, 3) usage",
                "// let c = Rgb { r: 1, g: 2, b: 3 };",
                "pub fn view_text() {}",
            ]
        )

        hits = check_theme_tokens.scan_text(text, "crates/x/src/observe.rs")

        self.assertEqual(hits, [])

    def test_a_trailing_comment_does_not_shield_real_code(self) -> None:
        text = "let c = Color::Rgb(1, 2, 3); // brand violet"

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [("crates/x/src/ui.rs", 1, "raw_ratatui_color")])

    def test_semantic_token_use_is_allowed(self) -> None:
        text = "\n".join(
            [
                "let style = theme::ratatui_style(theme::PRIMARY);",
                "let tint = theme::brand::PURPLE_1.lerp(theme::brand::PURPLE_3, t);",
                "push_line(&mut frame, art, theme::Fg::with_mode(tint, mode), reset, w);",
            ]
        )

        hits = check_theme_tokens.scan_text(text, "crates/x/src/ui.rs")

        self.assertEqual(hits, [])

    def test_theme_module_is_the_one_allowed_file(self) -> None:
        # The guard's whole premise: theme.rs mirrors the canonical CSS and is
        # covered by its own drift test, so it may name channel values.
        self.assertIn(
            Path("crates/coven-cli/src/theme.rs"), check_theme_tokens.ALLOWED
        )

    def test_the_live_tree_satisfies_the_invariant(self) -> None:
        self.assertEqual(check_theme_tokens.main(), 0)


if __name__ == "__main__":
    unittest.main()
