#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import re
import sys
from urllib.parse import unquote


ROOT = pathlib.Path(__file__).resolve().parents[1]
POLICY_PATH = ROOT / "SECURITY.md"
README_PATH = ROOT / "README.md"

POLICY_HEADING = re.compile(
    r"^#{1,6}[ \t]+Security Policy[ \t]*$",
    re.IGNORECASE | re.MULTILINE,
)
MARKDOWN_LINK = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
REQUIRED_SECTIONS = (
    "## 1. Supported surfaces and security status",
    "## 2. Enforced properties today",
    "## 3. Residual risk and safe configuration",
    "## 4. Reporting a vulnerability",
    "## 5. Design goals vs guarantees",
)
PRIVATE_ADVISORY_URL = (
    "https://github.com/OpenCoven/coven/security/advisories/new"
)
README_ADVISORY_URL = "https://github.com/OpenCoven/coven/security/advisories"
WINDOWS_ABSOLUTE_PATH = re.compile(r"^[A-Za-z]:[\\/]")
RETIRED_SCOPE = re.compile(
    r"\b(?:OpenCoven Security Disclosure Addendum|"
    r"organization-wide OpenCoven security addendum|OpenTrust)\b|"
    r"\b(?:inherits?|adopts?)[^.]{0,80}\borganization-wide\b"
    r"[^.]{0,80}\bsecurity\b[^.]{0,30}\b(?:policy|addendum)\b",
    re.IGNORECASE,
)
PERSONAL_REPORTING_CHANNEL = re.compile(
    r"\bDiscord\b[^\n]{0,120}(?:\bDM\b|\bdirect message\b)|"
    r"https?://(?:www\.)?discord(?:app)?\.com/users/",
    re.IGNORECASE,
)
RESPONSE_TIME_COMMITMENT = re.compile(
    r"\b(?:acknowledge|address|remediate|resolve|respond|reply|triage)"
    r"[^.]{0,80}"
    r"\bwithin\s+\d+\s+(?:hours?|days?|weeks?)\b",
    re.IGNORECASE,
)
SCOPE_CONTRACT = (
    "this policy covers Coven the runtime/daemon/CLI and the code in this repository."
)
BROAD_SCOPE = re.compile(
    r"\b(?:all|every)\s+OpenCoven repositories?\b|"
    r"\b(?:plus|and)\s+the\s+rest\s+of\s+the\s+OpenCoven\s+organization\b",
    re.IGNORECASE,
)
PUBLIC_REPORT_WARNING = (
    "**Do not open a public GitHub issue for security vulnerabilities.**"
)


def relative_links(content: str) -> list[str]:
    links = []
    for match in MARKDOWN_LINK.finditer(content):
        target = match.group(1).strip().strip("<>")
        if not target or target.startswith("#"):
            continue
        if WINDOWS_ABSOLUTE_PATH.match(target):
            links.append(target)
            continue
        if target.lower().startswith("file:"):
            links.append(target)
            continue
        if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", target):
            continue
        links.append(unquote(target.split("#", 1)[0].split("?", 1)[0]))
    return links


def validate_policy(
    policy: str,
    readme: str,
    root: pathlib.Path = ROOT,
) -> list[str]:
    errors = []
    normalized_policy = re.sub(r"(?m)^>\s?", "", policy)
    normalized_policy = re.sub(r"\s+", " ", normalized_policy)

    headings = POLICY_HEADING.findall(policy)
    if headings != ["# Security Policy"]:
        errors.append(
            "SECURITY.md must contain exactly one '# Security Policy' heading"
        )

    for section in REQUIRED_SECTIONS:
        if section not in policy:
            errors.append(f"SECURITY.md is missing required section: {section}")

    if RETIRED_SCOPE.search(policy):
        errors.append("SECURITY.md contains retired scope language")
    if SCOPE_CONTRACT not in normalized_policy or BROAD_SCOPE.search(normalized_policy):
        errors.append("SECURITY.md must remain scoped to Coven and this repository")
    if PERSONAL_REPORTING_CHANNEL.search(policy):
        errors.append("SECURITY.md contains a personal reporting channel")
    if RESPONSE_TIME_COMMITMENT.search(normalized_policy):
        errors.append("SECURITY.md contains an unsupported response-time commitment")
    if PRIVATE_ADVISORY_URL not in policy:
        errors.append("SECURITY.md must name the repository's private advisory intake")
    if PUBLIC_REPORT_WARNING not in policy:
        errors.append("SECURITY.md must forbid public vulnerability reports")
    if not re.search(
        rf"\*\*Primary path:\*\*[^.]{{0,300}}{re.escape(PRIVATE_ADVISORY_URL)}",
        normalized_policy,
    ):
        errors.append("SECURITY.md must keep private advisories as the primary path")

    for target in relative_links(policy):
        if (
            pathlib.PurePath(target).is_absolute()
            or WINDOWS_ABSOLUTE_PATH.match(target)
            or target.startswith("\\")
            or target.lower().startswith("file:")
        ):
            errors.append(
                f"SECURITY.md relative link must stay within the repository: {target}"
            )
            continue

        resolved_root = root.resolve()
        resolved_target = (root / target).resolve()
        if not resolved_target.is_relative_to(resolved_root):
            errors.append(
                f"SECURITY.md relative link must stay within the repository: {target}"
            )
        elif not resolved_target.is_file():
            errors.append(
                f"SECURITY.md relative link does not resolve: {target}"
            )

    if "(SECURITY.md)" not in readme:
        errors.append("README.md must link to SECURITY.md")
    if README_ADVISORY_URL not in readme:
        errors.append("README.md must link to the public Security Advisories page")

    return errors


def main() -> int:
    errors = validate_policy(
        POLICY_PATH.read_text(encoding="utf-8"),
        README_PATH.read_text(encoding="utf-8"),
    )
    if not errors:
        return 0

    print("Security policy check failed:", file=sys.stderr)
    for error in errors:
        print(f"- {error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
