#!/usr/bin/env python3
"""Keep terminal colors behind the semantic theme layer.

`docs/design/cast-tui-contract.md` section 2.4 requires every accent color to
come from a semantic token in `crates/coven-cli/src/theme.rs`, and the Phase 2
done-when checklist states the invariant as `rg "Rgb \\{" crates/coven-cli/src/tui`
returning nothing. Until now nothing enforced it, so a single inline
`Color::Rgb(0x9A, 0x8E, 0xCD)` could quietly fork the palette and drift from
`brand/ui/color-tokens.css` without the drift test noticing.

theme.rs is the one file allowed to name raw channel values: it is the mirror
of the canonical CSS and is itself covered by `brand_tokens_mirror_color_tokens_css`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_ROOT = ROOT / "crates"

# The single file permitted to spell out channel values.
ALLOWED = {Path("crates/coven-cli/src/theme.rs")}

PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    # `Rgb { r: .., g: .., b: .. }` — a hand-rolled brand token.
    ("raw_rgb_struct", re.compile(r"\bRgb\s*\{")),
    # `Color::Rgb(..)` / `RatColor::Rgb(..)` — a ratatui color built from
    # channels instead of routed through `theme::ratatui_color`.
    ("raw_ratatui_color", re.compile(r"\b(?:Rat)?Color::Rgb\s*\(")),
    # A literal SGR truecolor escape assembled by hand.
    ("raw_ansi_truecolor", re.compile(r"38;2;|48;2;")),
)


# Patterns that only make sense against production code. A unit test may
# legitimately assert that a rendered frame contains `\x1b[38;2;154;142;205m`
# — that test is *verifying* a semantic token reached the terminal, which is
# the invariant working, not breaking.
TEST_ONLY_EXEMPT = {"raw_ansi_truecolor"}

TEST_MODULE_MARKER = "#[cfg(test)]"


def scan_text(text: str, path: str) -> list[tuple[str, int, str]]:
    lines = text.splitlines()
    # Rust convention in this repo puts the test module at the end of the
    # file. Everything from the first `#[cfg(test)]` onward is test code.
    test_start = next(
        (i for i, line in enumerate(lines) if line.strip() == TEST_MODULE_MARKER),
        len(lines),
    )

    hits: list[tuple[str, int, str]] = []
    for lineno, line in enumerate(lines, start=1):
        # A comment cannot fork the palette, and prose about color is exactly
        # where these patterns appear innocently -- the doc comment on
        # `observe.rs::view_text` quotes `[38;2;154;142;205m` to explain what
        # leaks into a ratatui transcript. Only whole-line comments are
        # skipped, so `Color::Rgb(1, 2, 3); // note` is still caught.
        if line.lstrip().startswith("//"):
            continue
        in_tests = lineno > test_start
        for name, pattern in PATTERNS:
            if in_tests and name in TEST_ONLY_EXEMPT:
                continue
            if pattern.search(line):
                hits.append((path, lineno, name))
    return hits


def rust_sources() -> list[Path]:
    return sorted(p for p in SOURCE_ROOT.rglob("*.rs") if "target" not in p.parts)


def main() -> int:
    hits: list[tuple[str, int, str]] = []
    for path in rust_sources():
        rel = path.relative_to(ROOT)
        if rel in ALLOWED:
            continue
        text = path.read_text(encoding="utf-8", errors="surrogateescape")
        hits.extend(scan_text(text, str(rel)))

    if hits:
        print("Theme token guard blocked raw color values:")
        for path, lineno, name in hits:
            print(f"- {path}:{lineno}: {name}")
        print(
            "\nUse a semantic token from crates/coven-cli/src/theme.rs "
            "(or add one there, mirroring brand/ui/color-tokens.css)."
        )
        return 1

    print(f"Theme token guard passed: {len(rust_sources())} Rust files scanned.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
