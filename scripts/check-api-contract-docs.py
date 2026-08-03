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
HEALTH_GUIDANCE_DOCS = CONTRACT_DOCS + (
    "README.md",
    "docs/CLIENT-INTEGRATION.md",
    "docs/daemon/health.md",
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
EXPLICIT_FIELD_ABSENCE = re.compile(
    r"(?:"
    r"\b(?:excludes?|lacks?|omits?)\b[^.;]{0,120}\bsupportedApiVersions\b|"
    r"\b(?:(?:do|does|did)\s+not|(?:don|doesn|didn)['’]t)\s+"
    r"(?:include|return|contain|expose|advertise)\b"
    r"[^.;]{0,120}\bsupportedApiVersions\b|"
    r"\b(?:has|have|had)\s+(?:been\s+)?removed\b"
    r"[^.;]{0,120}\bsupportedApiVersions\b|"
    r"\bsupportedApiVersions\b[^.;]{0,120}"
    r"\b(?:is|are|was|were)\s+absent\s+from\b|"
    r"\bsupportedApiVersions\b[^.;]{0,120}"
    r"\b(?:is|are|was|were)\s+not\s+"
    r"(?:returned|included|contained|exposed|advertised)\s+by\b|"
    r"\bsupportedApiVersions\b[^.;]{0,120}"
    r"\b(?:has|have|had)\s+been\s+removed(?:\s+from)?\b|"
    r"\bsupportedApiVersions\b[^.;]{0,120}"
    r"\b(?:is|are|was|were)\s+(?:excluded|omitted|removed)\s+from\b"
    r")",
    re.IGNORECASE,
)
NEGATED_ABSENCE_VERB = re.compile(
    r"(?:"
    r"\b(?:not|never|cannot)\b|"
    r"\bcan['’]t\b|"
    r"\b(?:don|doesn|didn)['’]t\b|"
    r"\b(?:fails?|failed)\s+to\b|"
    r"\bno\s+longer\b"
    r")[^.;]{0,40}\b(?:excludes?|lacks?|omits?)\b",
    re.IGNORECASE,
)
HEALTH_FIELD_DECLARATION_VERB = re.compile(
    r"\b(?:returns?|contains?|includes?|exposes?|provides?)\b",
    re.IGNORECASE,
)
HEALTH_FIELD_DECLARATION_NOUN = re.compile(
    r"\b(?:response|fields?|body|payload|schema|data)\b",
    re.IGNORECASE,
)
PIPELESS_TABLE_BLOCK = re.compile(
    r"^\s*[^|\n]+\|[^|\n]+(?:\|[^|\n]+)*\s*\n"
    r"\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+(?:\n|$)"
)
STRUCTURAL_ROW_MARKER = "COVEN_DOC_STRUCTURAL_ROW"


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
    # Scope follows explicit health references, one immediate route-pronoun
    # continuation, and contiguous Markdown field rows introduced by either,
    # including a field block immediately following an explicit response-field
    # opener. Each field-bearing clause is then classified independently, so
    # legacy route examples and explicit removal guidance remain valid.
    pending_health_field_block = False
    for paragraph in re.split(r"\n\s*\n", text):
        starts_with_row = bool(
            re.match(r"^\s*(?:[-*]\s+|\|)", paragraph)
            or PIPELESS_TABLE_BLOCK.match(paragraph)
        )
        inherited_paragraph_scope = pending_health_field_block and starts_with_row
        pending_health_field_block = False
        if not SUPPORTED_API_VERSIONS.search(paragraph):
            if not starts_with_row:
                pending_health_field_block = opens_health_field_block(paragraph)
            continue
        paragraph = re.sub(
            r"\n(?=\s*(?:[-*]\s+|\|))",
            f". {STRUCTURAL_ROW_MARKER} ",
            paragraph,
        )
        if starts_with_row:
            paragraph = f"{STRUCTURAL_ROW_MARKER} {paragraph}"
        normalized = re.sub(r"\s+", " ", paragraph)
        statements = re.split(r"(?<=[.!?;])\s+(?=[A-Z`|*\-])", normalized)
        structural_health_scope = inherited_paragraph_scope
        for index, statement in enumerate(statements):
            if not SUPPORTED_API_VERSIONS.search(statement):
                if not statement.startswith(STRUCTURAL_ROW_MARKER):
                    structural_health_scope = bool(
                        HEALTH_REFERENCE.search(statement)
                    )
                continue
            is_structural_row = statement.startswith(STRUCTURAL_ROW_MARKER)
            inherited_health_scope = is_structural_row and structural_health_scope
            health_scoped = structural_health_scope if is_structural_row else bool(
                HEALTH_REFERENCE.search(statement)
            )
            if not is_structural_row:
                if not health_scoped and index > 0:
                    health_scoped = bool(
                        HEALTH_REFERENCE.search(statements[index - 1])
                        and ROUTE_CONTINUATION.search(statement)
                    )
                structural_health_scope = health_scoped
            if not health_scoped:
                continue

            field_clauses = [
                clause
                for clause in CLAUSE_BOUNDARY.split(statement)
                if SUPPORTED_API_VERSIONS.search(clause)
            ]
            health_absence_is_explicit = any(
                field_absence_is_explicit(clause)
                and (
                    inherited_health_scope
                    or HEALTH_REFERENCE.search(clause)
                    or LEGACY_ROUTE not in clause
                )
                for clause in field_clauses
            )
            for clause in field_clauses:
                if field_absence_is_explicit(clause):
                    continue
                if LEGACY_ROUTE in clause and health_absence_is_explicit:
                    continue
                return True
        if not starts_with_row:
            pending_health_field_block = opens_health_field_block(paragraph)
    return False


def opens_health_field_block(paragraph: str) -> bool:
    normalized = re.sub(r"\s+", " ", paragraph).strip()
    if not HEALTH_REFERENCE.search(normalized):
        return False
    declaration_verb = HEALTH_FIELD_DECLARATION_VERB.search(normalized)
    declaration_noun = HEALTH_FIELD_DECLARATION_NOUN.search(normalized)
    return bool(
        (normalized.endswith(":") and (declaration_verb or declaration_noun))
        or (declaration_verb and declaration_noun)
    )


def field_absence_is_explicit(clause: str) -> bool:
    return bool(
        EXPLICIT_FIELD_ABSENCE.search(clause)
        and not NEGATED_ABSENCE_VERB.search(clause)
    )


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

    for path in HEALTH_GUIDANCE_DOCS:
        if health_advertises_supported_api_versions(documents[path]):
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
    paths = tuple(dict.fromkeys(CONTRACT_DOCS + LIFECYCLE_DOCS + HEALTH_GUIDANCE_DOCS))
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
