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
    "packages/openclaw-coven/README.md",
    "docs/OPERATIONAL-MODEL.md",
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
HEALTH_ROUTE = "/api/v1/health"
LITERAL_V1 = re.compile(r'(?:"v1"|\'v1\'|`v1`)')
NAMED_CONTRACT = re.compile(
    r"\bcoven\.daemon\.v1\b|\bnamed[-\s]+(?:compatibility[-\s]+)?contract\b",
    re.IGNORECASE,
)
CONTRACT_CLAIM = re.compile(
    r"\b(?:"
    r"compatib(?:ility|le)|handshake|proof|prov(?:e|es|ed|ing)|"
    r"support(?:s|ed|ing)?|negotiat(?:e|es|ed|ing)|verif(?:y|ies|ied|ying)"
    r")\b",
    re.IGNORECASE,
)
NEGATION = re.compile(
    r"\b(?:never|no|cannot)\b|\bnot\b(?!\s+only\b)|"
    r"\bcan(?:not|['’]t)\b|\b(?:fails?|failed)\s+to\b|"
    r"\bmust\s+not\b|\bdo(?:es|did)\s+not\b",
    re.IGNORECASE,
)
CLAUSE_BOUNDARY = re.compile(
    r"\s*(?:[;,]|\b(?:and|but|however|while|although|though|yet)\b)\s*",
    re.IGNORECASE,
)
ROUTE_CONTINUATION = re.compile(
    r"^(?:it|its)\b|\b(?:this|the)\s+(?:route|endpoint|response)\b",
    re.IGNORECASE,
)
SUPPORTED_API_VERSIONS = re.compile(r"\bsupportedApiVersions\b", re.IGNORECASE)
HEALTH_REFERENCE = re.compile(
    rf"{re.escape(HEALTH_ROUTE)}|\bhealth(?:\s+response)?\b",
    re.IGNORECASE,
)
REMOVAL_GUIDANCE = re.compile(
    r"\b(?:has|have|had|is|are|was|were)\s+(?:been\s+)?removed\b|"
    r"\bremoved\s+from\b",
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
    # This is deliberately bounded grammar, not general NLP. A sentence naming
    # the legacy route is in scope, as is one immediate sentence that explicitly
    # refers back with a pronoun or "this route". Later prose and explicit health
    # sentences are independent. Coordinating clauses keep negation and health
    # exclusions local, including statements that mention both routes.
    for paragraph in re.split(r"\n\s*\n", text):
        if LEGACY_ROUTE not in paragraph:
            continue
        normalized = re.sub(r"\s+", " ", paragraph)
        statements = re.split(r"(?<=[.!?])\s+(?=[A-Z`|])", normalized)
        scoped_indexes: set[int] = set()
        for index, statement in enumerate(statements):
            if LEGACY_ROUTE not in statement:
                continue
            scoped_indexes.add(index)
            if index + 1 >= len(statements):
                continue
            continuation = statements[index + 1]
            if HEALTH_ROUTE not in continuation and ROUTE_CONTINUATION.search(
                continuation
            ):
                scoped_indexes.add(index + 1)

        for index in sorted(scoped_indexes):
            statement = statements[index]
            for clause in CLAUSE_BOUNDARY.split(statement):
                if HEALTH_ROUTE in clause and LEGACY_ROUTE not in clause:
                    continue
                if not NAMED_CONTRACT.search(clause):
                    continue
                for claim in CONTRACT_CLAIM.finditer(clause):
                    if not NEGATION.search(clause[: claim.start()]):
                        return True
    return False


def health_advertises_supported_api_versions(text: str) -> bool:
    # Scope follows explicit health references plus one immediate route-pronoun
    # continuation. Each field-bearing clause is then classified independently,
    # so legacy-route examples and explicit removal guidance remain valid.
    for paragraph in re.split(r"\n\s*\n", text):
        if not SUPPORTED_API_VERSIONS.search(paragraph):
            continue
        paragraph = re.sub(r"\n(?=\s*(?:[-*]\s+|\|))", ". ", paragraph)
        normalized = re.sub(r"\s+", " ", paragraph)
        statements = re.split(r"(?<=[.!?;])\s+(?=[A-Z`|*\-])", normalized)
        for index, statement in enumerate(statements):
            if not SUPPORTED_API_VERSIONS.search(statement):
                continue
            health_scoped = bool(HEALTH_REFERENCE.search(statement))
            if not health_scoped and index > 0:
                health_scoped = bool(
                    HEALTH_REFERENCE.search(statements[index - 1])
                    and ROUTE_CONTINUATION.search(statement)
                )
            if not health_scoped:
                continue
            if LEGACY_ROUTE in statement and not HEALTH_REFERENCE.search(statement):
                continue

            for clause in CLAUSE_BOUNDARY.split(statement):
                field = SUPPORTED_API_VERSIONS.search(clause)
                if not field:
                    continue
                if LEGACY_ROUTE in clause and not HEALTH_REFERENCE.search(clause):
                    continue
                if NEGATION.search(clause[: field.start()]):
                    continue
                if REMOVAL_GUIDANCE.search(clause):
                    continue
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
        if health_advertises_supported_api_versions(text):
            errors.append(f"{path}: health must not advertise supportedApiVersions")

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
