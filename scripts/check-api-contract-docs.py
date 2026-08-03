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
        for paragraph in re.split(r"\n\s*\n", text):
            if "/api/v1/api-version" not in paragraph or "coven.daemon.v1" not in paragraph:
                continue
            lowered = paragraph.lower()
            if "not" not in lowered or "proof" not in lowered:
                errors.append(
                    f"{path}: legacy route presented as named-contract handshake"
                )

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
    if "not proof of acknowledged process termination" not in lowered_contract:
        errors.append("docs/API-CONTRACT.md: killed acknowledgement boundary is missing")
    if not all(
        term in lowered_contract
        for term in ("synthetic", "`active`", "not a harness-session state")
    ):
        errors.append("docs/API-CONTRACT.md: synthetic active distinction is missing")
    if "stored separately in `archived_at`" not in contract:
        errors.append("docs/API-CONTRACT.md: archive separation is missing")
    return errors


def main() -> int:
    paths = sorted(set(CONTRACT_DOCS + LIFECYCLE_DOCS))
    documents = {
        relative: (ROOT / relative).read_text(encoding="utf-8")
        for relative in paths
    }
    errors = validate_documents(documents)

    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
