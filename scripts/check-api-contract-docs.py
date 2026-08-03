#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
CONTRACT_DOCS = (
    "docs/API-CONTRACT.md",
    "docs/API.md",
    "docs/reference/api-contract.md",
    "docs/reference/api.md",
    "docs/daemon/socket-api.md",
    "docs/daemon/capabilities-handshake.md",
    "docs/ARCHITECTURE.md",
)
LIFECYCLE_DOCS = (
    "docs/API-CONTRACT.md",
    "docs/SESSION-LIFECYCLE.md",
    "docs/sessions/lifecycle.md",
)
STATUSES = ("created", "running", "idle", "completed", "failed", "killed", "orphaned")
TERMINAL = {
    "created": "No",
    "running": "No",
    "idle": "No",
    "completed": "Yes",
    "failed": "Yes",
    "killed": "Yes",
    "orphaned": "Yes",
}
LEGACY_ROUTE = "/api/v1/api-version"
LITERAL_V1 = re.compile(r"(?<![./A-Za-z0-9_])v1(?![A-Za-z0-9_])", re.IGNORECASE)
ROUTE_CLAIM = r"(?:(?:compatibility|named-contract)[-\s]+handshake|proof)"
DIRECT_POSITIVE_ROUTE_CLAIM = re.compile(
    rf"{re.escape(LEGACY_ROUTE)}"
    r"(?:(?!\b(?:and|but)\b).){0,160}?"
    r"\b(?:is|are|serves\s+as|acts\s+as)\s+"
    rf"(?!(?:not|never|no)\b)(?:(?!\b(?:and|but)\b).){{0,120}}?\b{ROUTE_CLAIM}\b",
    re.IGNORECASE,
)
CONTINUED_POSITIVE_ROUTE_CLAIM = re.compile(
    r"\b(?:and|but)\s+(?:(?:it|this\s+route)\s+)?"
    r"(?:is|are|serves\s+as|acts\s+as)\s+"
    rf"(?!(?:not|never|no)\b)(?:(?!\b(?:and|but)\b).){{0,120}}?\b{ROUTE_CLAIM}\b",
    re.IGNORECASE,
)


def legacy_route_explained(text: str) -> bool:
    for paragraph in re.split(r"\n\s*\n", text):
        lowered = paragraph.lower()
        if (
            LEGACY_ROUTE in paragraph
            and "legacy" in lowered
            and "route-family" in lowered
            and LITERAL_V1.search(paragraph)
        ):
            return True
    return False


def presents_legacy_route_as_named_contract(text: str) -> bool:
    for paragraph in re.split(r"\n\s*\n", text):
        if LEGACY_ROUTE not in paragraph:
            continue
        normalized = re.sub(r"\s+", " ", paragraph)
        statements = re.split(r"(?<=[.!?])\s+(?=[A-Z`])", normalized)
        for statement in statements:
            if LEGACY_ROUTE not in statement:
                continue
            if DIRECT_POSITIVE_ROUTE_CLAIM.search(statement):
                return True
            if CONTINUED_POSITIVE_ROUTE_CLAIM.search(statement):
                return True
    return False


def validate_documents(documents: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for path in CONTRACT_DOCS:
        text = documents[path]
        if "/api/v1/health" not in text or "coven.daemon.v1" not in text:
            errors.append(f"{path}: missing health handshake")
        lowered = text.lower()
        if "capabilit" not in lowered or not any(
            term in lowered for term in ("never grant permission", "not authorization")
        ):
            errors.append(f"{path}: capabilities versus authorization is missing")
        if not legacy_route_explained(text):
            errors.append(f"{path}: legacy route explanation is missing")
        if presents_legacy_route_as_named_contract(text):
            errors.append(f"{path}: legacy route presented as named-contract handshake")

    for path in LIFECYCLE_DOCS:
        text = documents[path]
        for status in STATUSES:
            if f"`{status}`" not in text:
                errors.append(f"{path}: missing lifecycle status {status}")
                continue
            terminal = TERMINAL[status]
            pattern = rf"\|\s*`{status}`\s*\|\s*{terminal}\b"
            if not re.search(pattern, text):
                errors.append(
                    f"{path}: incorrect terminal classification for {status}"
                )
        lowered = text.lower()
        if "not proof of acknowledged process termination" not in lowered:
            errors.append(f"{path}: killed acknowledgement boundary is missing")
        if not all(
            term in lowered
            for term in ("synthetic", "`active`", "not a harness-session state")
        ):
            errors.append(f"{path}: synthetic active distinction is missing")
        if "stored separately in `archived_at`" not in lowered:
            errors.append(f"{path}: archive separation is missing")

    contract = documents["docs/API-CONTRACT.md"]
    lowered_contract = contract.lower()
    if not all(
        term in lowered_contract for term in ("stale unowned", "recover", "`failed`")
    ):
        errors.append("docs/API-CONTRACT.md: stale created recovery is missing")
    return errors


def main() -> int:
    paths = sorted(set(CONTRACT_DOCS + LIFECYCLE_DOCS))
    documents: dict[str, str] = {}
    for relative in paths:
        try:
            documents[relative] = (ROOT / relative).read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            print(
                f"{relative}: unable to read ({type(error).__name__})",
                file=sys.stderr,
            )
            return 1
    errors = validate_documents(documents)

    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
