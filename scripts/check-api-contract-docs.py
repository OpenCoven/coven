#!/usr/bin/env python3
from __future__ import annotations

import json
import pathlib
import re
import sys
from typing import NamedTuple


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
COVEN_DAEMON_API_VERSION = "coven.daemon.v1"
EXECUTION_BINDING_CONTRACT = "psyche.execution_binding.v1"
REQUEST_ADOPTION_CONTRACT = "psyche.request_adoption.v1"
HEALTH_CAPABILITY_FIELDS = (
    "sessions",
    "events",
    "travel",
    "scheduler",
    "hub",
    "executorDispatch",
    "eventCursor",
    "structuredErrors",
    "sessionHandoff",
    "sessionLaunchPolicy",
    "afs",
    "afsMount",
    "afsCommit",
    "afsCommitDryRun",
    "executionBindingContracts",
    "requestAdoptionContracts",
)
O3_REQUIRED_LITERALS = (
    "/api/v1/adopted-sessions",
    "/api/v1/sessions/:id/adopted-input",
    REQUEST_ADOPTION_CONTRACT,
    "requestAdoptionContracts",
    "request_adoption_required",
    "request_adoption_invalid",
    "request_adoption_unsupported",
    "request_adoption_conflict",
)
SESSION_LAUNCH_POLICY_DOCS = (
    "docs/API-CONTRACT.md",
    "docs/reference/api.md",
    "docs/reference/api-contract.md",
    "docs/daemon/socket-api.md",
    "docs/daemon/capabilities-handshake.md",
)
HEALTH_CAPABILITY_EXAMPLE_DOCS = (
    "docs/API-CONTRACT.md",
    "docs/reference/api-contract.md",
    "docs/daemon/socket-api.md",
)
HEALTH_CAPABILITY_LIST_DOCS = (
    "docs/API-CONTRACT.md",
    "docs/reference/api.md",
    "docs/reference/api-contract.md",
    "docs/daemon/socket-api.md",
)
HEALTH_CAPABILITY_COUNT_DOCS = (
    "docs/reference/api.md",
    "docs/reference/api-contract.md",
    "docs/daemon/socket-api.md",
)
STRUCTURED_O3_DOCS = (
    "docs/reference/api.md",
    "docs/reference/api-contract.md",
    "docs/daemon/socket-api.md",
)
PACKAGE_README_DOCS = (
    "packages/openclaw-coven/README.md",
    "packages/cli/README.md",
)
O3_ADOPTED_ROUTES = (
    "/api/v1/adopted-sessions",
    "/api/v1/sessions/:id/adopted-input",
)
O3_ADOPTION_ERRORS = (
    "request_adoption_required",
    "request_adoption_invalid",
    "request_adoption_unsupported",
    "request_adoption_conflict",
)
EXPECTED_REQUEST_ADOPTION_KEYS = ("contract", "key", "requestDigest")
REQUEST_ADOPTION_KEY = re.compile(r"[A-Za-z0-9._:/-]{1,255}", re.ASCII)
REQUEST_ADOPTION_DIGEST = re.compile(r"sha256:[0-9a-f]{64}", re.ASCII)
O3_ERROR_STATUSES = {
    "request_adoption_required": "400",
    "request_adoption_invalid": "400",
    "request_adoption_unsupported": "400",
    "request_adoption_conflict": "409",
    "event_preflight_failed": "500",
    "launch_failed": "500",
    "session_not_live": "409",
    "send_input_failed": "500",
    "input_coordination_failed": "500",
    "event_persistence_failed": "500",
    "input_lease_release_failed": "500",
}
REQUEST_ADOPTION_RULE_CELLS = {
    "contract": "Must equal `psyche.request_adoption.v1` byte-for-byte.",
    "key": (
        "1 to 255 ASCII bytes; every byte must match "
        "`[A-Za-z0-9._:/-]`."
    ),
    "requestDigest": (
        "Exactly `sha256:` followed by 64 lowercase hexadecimal characters "
        "(71 ASCII bytes total)."
    ),
}
REQUEST_ADOPTION_CLOSED_SHAPE_CLAIM = (
    "The object is closed: all three members are required, and any missing, "
    "unknown, or extra member is `request_adoption_invalid`."
)
REQUEST_ADOPTION_BYTE_PRESERVATION_CLAIM = (
    "Coven performs no trimming, case folding, Unicode normalization, or "
    "semantic interpretation. Accepted values are stored and compared "
    "byte-for-byte. Psyche owns canonical request serialization and digest "
    "computation; Coven checks syntax and equality only. Request adoption is "
    "neither authentication nor content attestation."
)
O3_STATIC_FIELD_PATHS = (
    ("Missing adoption", "requestAdoption"),
    ("Non-object adoption", "requestAdoption"),
    ("Malformed/non-string contract", "requestAdoption.contract"),
    ("Malformed key", "requestAdoption.key"),
    ("Malformed digest", "requestAdoption.requestDigest"),
    ("Different key", "executionBinding.attemptId"),
    ("Adoption on an unbound", "executionBinding"),
    ("Adoption on a legacy bound", "requestAdoption"),
)
LIFECYCLE_RECOVERY_CLAIMS = {
    "docs/API-CONTRACT.md": (
        "Lifecycle, ambiguity, and retention",
        3,
        "Generic stale-created recovery",
    ),
    "docs/SESSION-LIFECYCLE.md": (
        "Orphan recovery",
        2,
        "Generic stale-created recovery",
    ),
    "docs/sessions/lifecycle.md": (
        "Orphan recovery",
        2,
        "Marks only stale unowned",
    ),
}
EXPECTED_HEALTH_CAPABILITIES = {
    "sessions": True,
    "events": True,
    "travel": True,
    "scheduler": True,
    "hub": True,
    "executorDispatch": True,
    "eventCursor": "sequence",
    "structuredErrors": True,
    "sessionHandoff": True,
    "sessionLaunchPolicy": True,
    "afs": True,
    "afsMount": False,
    "afsCommit": True,
    "afsCommitDryRun": True,
    "executionBindingContracts": [EXECUTION_BINDING_CONTRACT],
    "requestAdoptionContracts": [REQUEST_ADOPTION_CONTRACT],
}
EXPECTED_HEALTH_FIELDS = {
    "ok": True,
    "apiVersion": "coven.daemon.v1",
}
EXPECTED_HEALTH_CONTRACTS = {
    field: EXPECTED_HEALTH_CAPABILITIES[field]
    for field in ("executionBindingContracts", "requestAdoptionContracts")
}
ADOPTED_INPUT_FIRST_RESULT = {
    "adopted": True,
    "replayed": False,
    "delivery": "not_asserted",
}
ADOPTED_INPUT_REPLAY_RESULT = {
    "adopted": True,
    "replayed": True,
    "delivery": "not_asserted",
}
ADOPTED_OPERATION_EXPECTATIONS = {
    "/api/v1/adopted-sessions": (
        201,
        "SessionRecord",
        200,
        "SessionRecord",
    ),
    "/api/v1/sessions/:id/adopted-input": (
        202,
        ADOPTED_INPUT_FIRST_RESULT,
        200,
        ADOPTED_INPUT_REPLAY_RESULT,
    ),
}
HEALTH_EXAMPLE_SECTIONS = {
    "docs/reference/api-contract.md": "Negotiation",
    "docs/daemon/socket-api.md": "Handshake",
}
HEALTH_CAPABILITY_LISTS = {
    "docs/reference/api.md": (
        "Contract and discovery",
        "The health `capabilities` object currently contains",
    ),
    "docs/reference/api-contract.md": (
        "Negotiation",
        "The health `capabilities` object contains",
    ),
    "docs/daemon/socket-api.md": (
        "Handshake",
        "This example contains all",
    ),
}


class O3NegotiationSurface(NamedTuple):
    heading: str
    level: int
    markers: tuple[str, ...]
    api_version_literal: str
    literal_claim: str
    owns_proof_boundary: bool
    owns_no_fallback: bool


O3_NEGOTIATION_SURFACES = {
    "docs/API-CONTRACT.md": O3NegotiationSurface(
        "Health negotiation and fail-closed clients",
        3,
        ("Before every adopted launch or input",),
        COVEN_DAEMON_API_VERSION,
        REQUEST_ADOPTION_CONTRACT,
        True,
        True,
    ),
    "docs/reference/api.md": O3NegotiationSurface(
        "Sessions and events",
        2,
        ("Before either adopted POST",),
        COVEN_DAEMON_API_VERSION,
        REQUEST_ADOPTION_CONTRACT,
        True,
        True,
    ),
    "docs/reference/api-contract.md": O3NegotiationSurface(
        "Negotiation",
        2,
        ("Before either adopted route",),
        COVEN_DAEMON_API_VERSION,
        REQUEST_ADOPTION_CONTRACT,
        True,
        True,
    ),
    "docs/daemon/socket-api.md": O3NegotiationSurface(
        "Endpoints",
        2,
        ("Before either adopted POST",),
        COVEN_DAEMON_API_VERSION,
        REQUEST_ADOPTION_CONTRACT,
        True,
        True,
    ),
    "packages/openclaw-coven/README.md": O3NegotiationSurface(
        "Adopted client methods",
        3,
        ("Before either mutation",),
        COVEN_DAEMON_API_VERSION,
        REQUEST_ADOPTION_CONTRACT,
        True,
        True,
    ),
}

O3_API_VERSION_GATE_FRAGMENTS = (
    "First it requires health.apiVersion to be the exact string "
    "coven.daemon.v1.",
)
O3_REQUEST_ADOPTION_GATE_FRAGMENTS = (
    "Only after that check passes does it require "
    "health.capabilities.requestAdoptionContracts to be an array containing "
    "the exact psyche.request_adoption.v1 string.",
)
O3_PROOF_BOUNDARY_FRAGMENTS = (
    "every adopted request must still carry a complete, exact O2 "
    "executionBinding proof",
)
O3_NO_FALLBACK_FRAGMENTS = (
    "Any health, API-version, or capability failure sends zero POST requests "
    "and never falls back to a legacy mutation.",
)

O3_NEGOTIATION_REQUIRED_FRAGMENTS = {
    "docs/API-CONTRACT.md": {
        "api_version": O3_API_VERSION_GATE_FRAGMENTS,
        "gate": O3_REQUEST_ADOPTION_GATE_FRAGMENTS,
        "proof": O3_PROOF_BOUNDARY_FRAGMENTS,
        "no_fallback": O3_NO_FALLBACK_FRAGMENTS,
    },
    "docs/reference/api.md": {
        "api_version": O3_API_VERSION_GATE_FRAGMENTS,
        "gate": O3_REQUEST_ADOPTION_GATE_FRAGMENTS,
        "proof": O3_PROOF_BOUNDARY_FRAGMENTS,
        "no_fallback": O3_NO_FALLBACK_FRAGMENTS,
    },
    "docs/reference/api-contract.md": {
        "api_version": O3_API_VERSION_GATE_FRAGMENTS,
        "gate": O3_REQUEST_ADOPTION_GATE_FRAGMENTS,
        "proof": O3_PROOF_BOUNDARY_FRAGMENTS,
        "no_fallback": O3_NO_FALLBACK_FRAGMENTS,
    },
    "docs/daemon/socket-api.md": {
        "api_version": O3_API_VERSION_GATE_FRAGMENTS,
        "gate": O3_REQUEST_ADOPTION_GATE_FRAGMENTS,
        "proof": O3_PROOF_BOUNDARY_FRAGMENTS,
        "no_fallback": O3_NO_FALLBACK_FRAGMENTS,
    },
    "packages/openclaw-coven/README.md": {
        "api_version": O3_API_VERSION_GATE_FRAGMENTS
        + (
            "A missing, null, non-string, wrong-case, near-match, or otherwise "
            "unsupported health.apiVersion fails closed on its own, before the "
            "capability is even inspected",
        ),
        "gate": O3_REQUEST_ADOPTION_GATE_FRAGMENTS
        + (
            "a valid health.apiVersion with a missing, malformed, or unsupported "
            "capability advertisement fails on the capability check instead",
        ),
        "proof": O3_PROOF_BOUNDARY_FRAGMENTS,
        "no_fallback": O3_NO_FALLBACK_FRAGMENTS,
    },
}


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


def markdown_sections(text: str, heading: str, *, level: int = 2) -> list[str]:
    """Return exact heading bodies, each bounded by its next peer or parent."""
    lines = text.splitlines()
    marker = f"{'#' * level} {heading}"
    starts = [index + 1 for index, line in enumerate(lines) if line == marker]
    boundary = re.compile(rf"^#{{1,{level}}}\s+")
    sections: list[str] = []
    for start in starts:
        end = len(lines)
        for index in range(start, len(lines)):
            if boundary.match(lines[index]):
                end = index
                break
        sections.append("\n".join(lines[start:end]))
    return sections


def markdown_section(text: str, heading: str, *, level: int = 2) -> str | None:
    """Return a unique exact Markdown heading body, failing closed on ambiguity."""
    sections = markdown_sections(text, heading, level=level)
    return sections[0] if len(sections) == 1 else None


def markdown_paragraphs(text: str, marker: str) -> list[str]:
    """Return paragraphs from each line containing marker."""
    lines = text.splitlines()
    paragraphs: list[str] = []
    for start, line in enumerate(lines):
        if marker not in line:
            continue
        end = start + 1
        while end < len(lines) and lines[end].strip():
            end += 1
        paragraphs.append("\n".join(lines[start:end]))
    return paragraphs


def markdown_paragraph(text: str, marker: str) -> str | None:
    """Return one marked paragraph, failing closed on duplicates."""
    paragraphs = markdown_paragraphs(text, marker)
    return paragraphs[0] if len(paragraphs) == 1 else None


def markdown_list_items(text: str, marker: str) -> list[str]:
    """Return marked dash-list items and their wrapped continuation lines."""
    lines = text.splitlines()
    items: list[str] = []
    for start, line in enumerate(lines):
        if not line.lstrip().startswith("- ") or marker not in line:
            continue
        end = start + 1
        while end < len(lines):
            candidate = lines[end]
            if not candidate.strip() or candidate.lstrip().startswith("- "):
                break
            end += 1
        items.append("\n".join(lines[start:end]))
    return items


def markdown_list_item(text: str, marker: str) -> str | None:
    """Return one marked dash-list item, failing closed on duplicates."""
    items = markdown_list_items(text, marker)
    return items[0] if len(items) == 1 else None


def fenced_code_blocks(text: str, language: str) -> list[str]:
    """Return fenced blocks whose opening fence has the exact language."""
    lines = text.splitlines()
    opening = re.compile(r"^[ \t]*```([A-Za-z0-9_+-]+)[ \t]*$")
    closing = re.compile(r"^[ \t]*```[ \t]*$")
    blocks: list[str] = []
    index = 0
    while index < len(lines):
        match = opening.fullmatch(lines[index])
        if match is None or match.group(1) != language:
            index += 1
            continue
        end = index + 1
        while end < len(lines) and not closing.fullmatch(lines[end]):
            end += 1
        if end == len(lines):
            break
        blocks.append("\n".join(lines[index + 1 : end]))
        index = end + 1
    return blocks


def fenced_code_block(text: str, language: str) -> str | None:
    """Return one exact-language fenced block, failing closed on duplicates."""
    blocks = fenced_code_blocks(text, language)
    return blocks[0] if len(blocks) == 1 else None


def http_json_examples(text: str, request_line: str) -> list[str]:
    """Return JSON fences paired with an exact HTTP request fence."""
    pattern = re.compile(
        r"```http[ \t]*\n[ \t]*"
        + re.escape(request_line)
        + r"[ \t]*\n```[ \t]*\n(?:[ \t]*\n)*```json[ \t]*\n"
        + r"(.*?)\n```",
        re.DOTALL,
    )
    return pattern.findall(text)


def http_json_example(text: str, request_line: str) -> str | None:
    """Return one paired JSON fence, failing closed on duplicates."""
    matches = http_json_examples(text, request_line)
    return matches[0] if len(matches) == 1 else None


def json_examples_after_marker(text: str, marker: str) -> list[str]:
    """Return JSON fences immediately following an exact marker line."""
    pattern = re.compile(
        r"^"
        + re.escape(marker)
        + r"[ \t]*\n(?:[ \t]*\n)*```json[ \t]*\n(.*?)\n```[ \t]*$",
        re.MULTILINE | re.DOTALL,
    )
    return pattern.findall(text)


def json_example_after_marker(text: str, marker: str) -> str | None:
    """Return one marked JSON fence, failing closed on duplicates."""
    matches = json_examples_after_marker(text, marker)
    return matches[0] if len(matches) == 1 else None


class MarkdownTableRow(NamedTuple):
    cells: tuple[str, ...]
    source: str


class MarkdownTable(NamedTuple):
    headers: tuple[str, ...]
    rows: tuple[MarkdownTableRow, ...]


def split_markdown_table_row(line: str) -> tuple[str, ...] | None:
    stripped = line.strip()
    if not stripped.startswith("|") or not stripped.endswith("|"):
        return None
    cells = re.split(r"(?<!\\)\|", stripped[1:-1])
    return tuple(cell.replace(r"\|", "|").strip() for cell in cells)


def markdown_cell_value(cell: str) -> str:
    """Remove whole-cell code formatting while preserving internal semantics."""
    value = re.sub(r"\s+", " ", cell.strip())
    if value.startswith("`") and value.endswith("`") and value.count("`") == 2:
        return value[1:-1].strip()
    return value


def normalized_markdown_text(text: str, *, lowercase: bool = False) -> str:
    """Normalize wrapping and inline-code delimiters without changing words."""
    value = re.sub(r"\s+", " ", text.replace("`", "")).strip()
    return value.lower() if lowercase else value


def markdown_tables(text: str) -> list[MarkdownTable]:
    """Parse ordinary leading/trailing-pipe Markdown tables."""
    lines = text.splitlines()
    separator = re.compile(r"^:?-{3,}:?$")
    tables: list[MarkdownTable] = []
    index = 0
    while index + 1 < len(lines):
        headers = split_markdown_table_row(lines[index])
        separators = split_markdown_table_row(lines[index + 1])
        if (
            headers is None
            or separators is None
            or len(headers) != len(separators)
            or not all(separator.fullmatch(cell.replace(" ", "")) for cell in separators)
        ):
            index += 1
            continue

        rows: list[MarkdownTableRow] = []
        end = index + 2
        while end < len(lines):
            cells = split_markdown_table_row(lines[end])
            if cells is None or len(cells) != len(headers):
                break
            rows.append(MarkdownTableRow(cells=cells, source=lines[end]))
            end += 1
        tables.append(MarkdownTable(headers=headers, rows=tuple(rows)))
        index = end
    return tables


def matching_markdown_tables(
    text: str, headers: tuple[str, ...]
) -> list[MarkdownTable]:
    expected = tuple(markdown_cell_value(header) for header in headers)
    return [
        table
        for table in markdown_tables(text)
        if tuple(markdown_cell_value(header) for header in table.headers) == expected
    ]


def markdown_table(text: str, headers: tuple[str, ...]) -> MarkdownTable | None:
    """Return one table with exact headers, failing closed on duplicates."""
    tables = matching_markdown_tables(text, headers)
    return tables[0] if len(tables) == 1 else None


def markdown_table_rows(
    table: MarkdownTable, expected_cells: dict[str, str]
) -> list[MarkdownTableRow]:
    header_indexes = {
        markdown_cell_value(header): index
        for index, header in enumerate(table.headers)
    }
    if not set(expected_cells).issubset(header_indexes):
        return []
    return [
        row
        for row in table.rows
        if all(
            markdown_cell_value(row.cells[header_indexes[header]]) == expected
            for header, expected in expected_cells.items()
        )
    ]


def markdown_table_rows_containing(
    table: MarkdownTable, expected_cells: dict[str, str]
) -> list[MarkdownTableRow]:
    """Return rows whose named cells contain each expected marker."""
    header_indexes = {
        markdown_cell_value(header): index
        for index, header in enumerate(table.headers)
    }
    if not set(expected_cells).issubset(header_indexes):
        return []
    return [
        row
        for row in table.rows
        if all(
            marker in markdown_cell_value(row.cells[header_indexes[header]])
            for header, marker in expected_cells.items()
        )
    ]


def markdown_table_cell(
    table: MarkdownTable, row: MarkdownTableRow, header: str
) -> str:
    index = tuple(markdown_cell_value(value) for value in table.headers).index(header)
    return row.cells[index]


def markdown_table_row(text: str, literal: str) -> str | None:
    """Return a pipe-table row containing an exact literal."""
    for line in text.splitlines():
        if line.lstrip().startswith("|") and literal in line:
            return line
    return None


def markdown_route_row(text: str, method: str, route: str) -> str | None:
    """Return a route row from either split Method/Path or combined-route tables."""
    for line in text.splitlines():
        if not line.lstrip().startswith("|"):
            continue
        cells = [
            cell.strip().strip("`").strip()
            for cell in line.strip().strip("|").split("|")
        ]
        if len(cells) >= 2 and cells[0] == method and cells[1] == route:
            return line
        if cells and cells[0] == f"{method} {route}":
            return line
    return None


def require_markdown_section(
    documents: dict[str, str],
    path: str,
    heading: str,
    *,
    level: int,
    errors: list[str],
) -> str | None:
    sections = markdown_sections(documents[path], heading, level=level)
    if len(sections) != 1:
        errors.append(f"{path}: expected one {heading} section (found {len(sections)})")
        return None
    return sections[0]


def require_markdown_table(
    section: str,
    headers: tuple[str, ...],
    *,
    path: str,
    label: str,
    errors: list[str],
) -> MarkdownTable | None:
    tables = matching_markdown_tables(section, headers)
    if len(tables) != 1:
        errors.append(f"{path}: expected one {label} table (found {len(tables)})")
        return None
    return tables[0]


def require_marked_paragraph(
    section: str,
    marker: str,
    *,
    path: str,
    label: str,
    errors: list[str],
) -> str | None:
    paragraphs = markdown_paragraphs(section, marker)
    if len(paragraphs) != 1:
        errors.append(
            f"{path}: expected one {label} paragraph for {marker!r} "
            f"(found {len(paragraphs)})"
        )
        return None
    return paragraphs[0]


def strict_inline_json(cell: str) -> object | None:
    """Parse a whole-cell inline-code JSON value, failing closed otherwise."""
    match = re.fullmatch(r"\s*`([^`\n]+)`\s*", cell)
    if match is None:
        return None
    try:
        return strict_json_loads(match.group(1))
    except json.JSONDecodeError:
        return None


class DuplicateJSONKeyError(json.JSONDecodeError):
    """Raised when a JSON object repeats a key at any nesting depth.

    Subclasses ``json.JSONDecodeError`` so every existing call site that
    already catches ``json.JSONDecodeError`` (and reports ``error.msg``,
    ``error.lineno``, and ``error.colno``) keeps working unchanged for this
    failure mode too, without needing a second except clause.
    """

    def __init__(self, key: str, doc: str) -> None:
        self.key = key
        super().__init__(f"duplicate key {key!r}", doc, 0)


class NonFiniteJSONConstantError(json.JSONDecodeError):
    """Raised for JavaScript numeric constants that RFC 8259 excludes."""

    def __init__(self, constant: str, doc: str) -> None:
        self.constant = constant
        super().__init__(f"non-finite JSON constant {constant!r}", doc, 0)


def strict_json_loads(text: str) -> object:
    """Decode strict JSON, rejecting duplicates and non-finite constants.

    Plain ``json.loads`` silently keeps only the last value for a repeated
    object key, which lets contradictory closed-shape JSON examples (for
    example two conflicting ``requestAdoptionContracts`` entries, or a
    duplicated ``adopted``/``replayed``/``delivery`` field) parse
    successfully. This is the single loader every JSON-shaped check in this
    script must use instead of ``json.loads`` directly, so duplicate keys and
    non-finite constants fail closed everywhere: the canonical health block,
    synchronized health examples, the capability-table Description cell, and
    adopted result/status table cells. Diagnostics name only the offending key
    or constant, never the (possibly sensitive) values involved.
    """

    def reject_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
        seen: set[str] = set()
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in seen:
                raise DuplicateJSONKeyError(key, text)
            seen.add(key)
            result[key] = value
        return result

    def reject_non_finite(constant: str) -> object:
        raise NonFiniteJSONConstantError(constant, text)

    return json.loads(
        text,
        object_pairs_hook=reject_duplicates,
        parse_constant=reject_non_finite,
    )


def expected_json_text(value: object) -> str:
    return json.dumps(value, separators=(",", ":"))


def strict_json_equal(actual: object, expected: object) -> bool:
    """Compare decoded JSON without Python's bool/int equality aliasing."""
    if type(actual) is not type(expected):
        return False
    if isinstance(expected, dict):
        return actual.keys() == expected.keys() and all(
            strict_json_equal(actual[key], value)
            for key, value in expected.items()
        )
    if isinstance(expected, list):
        return len(actual) == len(expected) and all(
            strict_json_equal(actual_value, expected_value)
            for actual_value, expected_value in zip(actual, expected)
        )
    return actual == expected


def validate_health_json_example(
    example: str | None,
    *,
    path: str,
    label: str,
    errors: list[str],
) -> None:
    if example is None:
        errors.append(f"{path}: {label} JSON fence is missing or ambiguous")
        return
    try:
        payload = strict_json_loads(example)
    except json.JSONDecodeError as error:
        errors.append(
            f"{path}: {label} JSON is invalid "
            f"(line {error.lineno}, column {error.colno}: {error.msg})"
        )
        return

    if isinstance(payload, dict):
        for field, expected in EXPECTED_HEALTH_FIELDS.items():
            if field not in payload:
                errors.append(f"{path}: {label} missing {field}")
            elif not strict_json_equal(payload[field], expected):
                errors.append(
                    f"{path}: {label} {field} must equal "
                    f"{expected_json_text(expected)}"
                )

    capabilities = payload.get("capabilities") if isinstance(payload, dict) else None
    if not isinstance(capabilities, dict):
        errors.append(f"{path}: {label} capabilities object is missing or invalid")
        return

    for field in HEALTH_CAPABILITY_FIELDS:
        if field not in capabilities:
            errors.append(f"{path}: {label} missing {field}")
    if len(capabilities) != len(HEALTH_CAPABILITY_FIELDS):
        errors.append(
            f"{path}: {label} capabilities must contain exactly "
            f"{len(HEALTH_CAPABILITY_FIELDS)} fields (found {len(capabilities)})"
        )

    for field, expected in EXPECTED_HEALTH_CAPABILITIES.items():
        if field in capabilities and not strict_json_equal(
            capabilities[field], expected
        ):
            errors.append(
                f"{path}: {label} {field} must equal {expected_json_text(expected)}"
            )


def validate_capability_value_table(
    documents: dict[str, str], errors: list[str]
) -> None:
    path = "docs/API-CONTRACT.md"
    section = require_markdown_section(
        documents,
        path,
        "Capability fields",
        level=3,
        errors=errors,
    )
    if section is None:
        return
    table = require_markdown_table(
        section,
        ("Field", "Type", "Description"),
        path=path,
        label="capability fields",
        errors=errors,
    )
    if table is None:
        return

    for field, expected in EXPECTED_HEALTH_CONTRACTS.items():
        rows = markdown_table_rows(table, {"Field": field})
        if not rows:
            errors.append(f"{path}: capability table missing {field} row")
            continue
        if len(rows) != 1:
            errors.append(f"{path}: capability table has ambiguous {field} row")
            continue
        row = rows[0]
        type_cell = markdown_cell_value(markdown_table_cell(table, row, "Type"))
        if type_cell != "string array":
            errors.append(
                f"{path}: capability table {field} Type cell must equal string array"
            )

        description = markdown_table_cell(table, row, "Description")
        documented = re.findall(r"\bCurrently\s+`([^`]+)`", description)
        parsed: object | None = None
        if len(documented) == 1:
            try:
                parsed = strict_json_loads(documented[0])
            except json.JSONDecodeError:
                pass
        if not strict_json_equal(parsed, expected):
            errors.append(
                f"{path}: capability table {field} Description cell must document "
                f"exact current value {expected_json_text(expected)}"
            )


def whole_cell_code_expression(cell: str) -> str | None:
    """Return one normalized whole-cell code expression, or plain extracted code."""
    normalized = re.sub(r"\s+", " ", cell).strip()
    match = re.fullmatch(r"`([^`\n]+)`", normalized)
    if match is not None:
        return re.sub(r"\s+", " ", match.group(1)).strip()
    if "`" in normalized:
        return None
    return normalized


def status_result_matches(
    cell: str,
    expected_status: int,
    expected_result: str | dict[str, object],
    *,
    shape_reference: str | None = None,
) -> bool:
    normalized = re.sub(r"\s+", " ", cell).strip()
    plain = normalized.replace("`", "").strip()
    status = re.match(r"^([1-5]\d{2})(?=\s|$)", plain)
    if status is None:
        return False
    statuses = re.findall(r"(?<![\d.])[1-5]\d{2}(?![\d.])", plain)
    if statuses != [str(expected_status)]:
        return False
    if isinstance(expected_result, str) and expected_result == "SessionRecord":
        expression = whole_cell_code_expression(cell)
        if expression == f"{expected_status} SessionRecord":
            return True
        return normalized in {
            f"`{expected_status}` with the full `SessionRecord`.",
            f"`{expected_status}` with the current persisted `SessionRecord`.",
        }
    if shape_reference is not None:
        phrase = (
            "exact first-adoption shape below"
            if shape_reference == "first"
            else "exact replay shape below"
        )
        return normalized == f"`{expected_status}` with the {phrase}."

    expression = whole_cell_code_expression(cell)
    if expression is None:
        return False
    match = re.fullmatch(rf"{expected_status}\s+(.+)", expression)
    if match is None:
        return False
    try:
        actual = strict_json_loads(match.group(1))
    except (DuplicateJSONKeyError, NonFiniteJSONConstantError):
        raise
    except json.JSONDecodeError:
        return False
    return strict_json_equal(actual, expected_result)


def labeled_status_results(cell: str) -> dict[str, list[str]]:
    claims = {"first": [], "replay": []}
    normalized = re.sub(r"\s+", " ", cell).strip()
    pattern = re.compile(
        r"^(?:(?:Adopt and launch a bound session|Adopt bound input):\s*)?"
        r"`([^`\n]+)`\s+first(?:\s+adoption)?\s*[;,]\s*"
        r"`([^`\n]+)`\s+(?:exact\s+)?replay\.?$",
        re.IGNORECASE,
    )
    match = pattern.fullmatch(normalized)
    if match is not None:
        claims["first"].append(match.group(1))
        claims["replay"].append(match.group(2))
    return claims


def append_status_result_matches(
    *,
    cell: str,
    status: int,
    result: str | dict[str, object],
    shape_reference: str | None,
    path: str,
    display: str,
    phase: str,
    errors: list[str],
) -> None:
    """Evaluate one status/result cell, failing closed on duplicate JSON keys.

    A duplicate object key inside the cell's embedded JSON gets its own
    diagnostic naming the key, instead of collapsing into the generic
    "status/result is incorrect" message used for other mismatches.
    """
    label = "first-adoption" if phase == "first" else "exact-replay"
    try:
        matches = status_result_matches(cell, status, result, shape_reference=shape_reference)
    except DuplicateJSONKeyError as error:
        errors.append(
            f"{path}: {display} {label} result JSON has duplicate key {error.key!r}"
        )
        return
    except NonFiniteJSONConstantError as error:
        errors.append(
            f"{path}: {display} {label} result JSON is invalid "
            f"(line {error.lineno}, column {error.colno}: {error.msg})"
        )
        return
    if not matches:
        errors.append(f"{path}: {display} {label} status/result is incorrect")


def append_status_result_errors(
    *,
    path: str,
    display: str,
    first_cell: str,
    replay_cell: str,
    route: str,
    errors: list[str],
    shape_references: bool = False,
) -> None:
    first_status, first_result, replay_status, replay_result = (
        ADOPTED_OPERATION_EXPECTATIONS[route]
    )
    append_status_result_matches(
        cell=first_cell,
        status=first_status,
        result=first_result,
        shape_reference="first" if shape_references and isinstance(first_result, dict) else None,
        path=path,
        display=display,
        phase="first",
        errors=errors,
    )
    append_status_result_matches(
        cell=replay_cell,
        status=replay_status,
        result=replay_result,
        shape_reference=(
            "replay" if shape_references and isinstance(replay_result, dict) else None
        ),
        path=path,
        display=display,
        phase="replay",
        errors=errors,
    )


def append_combined_status_result_errors(
    *,
    path: str,
    display: str,
    cell: str,
    route: str,
    errors: list[str],
) -> None:
    claims = labeled_status_results(cell)
    first_status, first_result, replay_status, replay_result = (
        ADOPTED_OPERATION_EXPECTATIONS[route]
    )
    if len(claims["first"]) != 1:
        errors.append(
            f"{path}: {display} first-adoption status/result is incorrect"
        )
    else:
        append_status_result_matches(
            cell=claims["first"][0],
            status=first_status,
            result=first_result,
            shape_reference=None,
            path=path,
            display=display,
            phase="first",
            errors=errors,
        )
    if len(claims["replay"]) != 1:
        errors.append(f"{path}: {display} exact-replay status/result is incorrect")
    else:
        append_status_result_matches(
            cell=claims["replay"][0],
            status=replay_status,
            result=replay_result,
            shape_reference=None,
            path=path,
            display=display,
            phase="replay",
            errors=errors,
        )


def validate_separate_adopted_table(
    documents: dict[str, str],
    errors: list[str],
    *,
    path: str,
    heading: str,
    level: int,
    headers: tuple[str, ...],
    rows: dict[str, tuple[str, dict[str, str], dict[str, str]]],
    first_header: str,
    replay_header: str,
    table_label: str,
    row_kind: str,
    shape_references: bool = False,
) -> tuple[MarkdownTable | None, dict[str, MarkdownTableRow]]:
    section = require_markdown_section(
        documents, path, heading, level=level, errors=errors
    )
    if section is None:
        return None, {}
    table = require_markdown_table(
        section,
        headers,
        path=path,
        label=table_label,
        errors=errors,
    )
    if table is None:
        return None, {}

    found: dict[str, MarkdownTableRow] = {}
    for route, (display, criteria, exact_cells) in rows.items():
        matches = markdown_table_rows(table, criteria)
        if not matches:
            if row_kind == "method":
                errors.append(f"{path}: adopted method table missing {display}")
            else:
                errors.append(f"{path}: adopted route table missing POST {route}")
            continue
        if len(matches) != 1:
            if row_kind == "method":
                errors.append(
                    f"{path}: adopted method table has ambiguous {display} row"
                )
            else:
                errors.append(
                    f"{path}: adopted route table has ambiguous POST {route} row"
                )
            continue
        row = matches[0]
        found[route] = row
        for header, expected in exact_cells.items():
            actual = markdown_cell_value(markdown_table_cell(table, row, header))
            if actual != expected:
                errors.append(
                    f"{path}: {display} {header} cell must equal {expected}"
                )
        append_status_result_errors(
            path=path,
            display=display,
            first_cell=markdown_table_cell(table, row, first_header),
            replay_cell=markdown_table_cell(table, row, replay_header),
            route=route,
            errors=errors,
            shape_references=shape_references,
        )
    return table, found


def validate_combined_adopted_table(
    documents: dict[str, str],
    errors: list[str],
    *,
    path: str,
    heading: str,
    level: int,
    headers: tuple[str, ...],
    rows: dict[str, dict[str, str]],
    success_header: str,
    table_label: str,
) -> tuple[MarkdownTable | None, dict[str, MarkdownTableRow]]:
    section = require_markdown_section(
        documents, path, heading, level=level, errors=errors
    )
    if section is None:
        return None, {}
    table = require_markdown_table(
        section,
        headers,
        path=path,
        label=table_label,
        errors=errors,
    )
    if table is None:
        return None, {}

    found: dict[str, MarkdownTableRow] = {}
    for route, criteria in rows.items():
        matches = markdown_table_rows(table, criteria)
        if not matches:
            errors.append(f"{path}: adopted route table missing POST {route}")
            continue
        if len(matches) != 1:
            errors.append(
                f"{path}: adopted route table has ambiguous POST {route} row"
            )
            continue
        row = matches[0]
        found[route] = row
        append_combined_status_result_errors(
            path=path,
            display=route,
            cell=markdown_table_cell(table, row, success_header),
            route=route,
            errors=errors,
        )
    return table, found


def validate_canonical_adopted_input_results(
    documents: dict[str, str], errors: list[str]
) -> None:
    path = "docs/API-CONTRACT.md"
    section = require_markdown_section(
        documents,
        path,
        "Adopted routes, compatibility, and responses",
        level=3,
        errors=errors,
    )
    if section is None:
        return
    examples = (
        (
            "The first successful adopted-input response is exactly:",
            "first-adoption",
            ADOPTED_INPUT_FIRST_RESULT,
        ),
        (
            "An exact adopted-input replay is exactly:",
            "exact-replay",
            ADOPTED_INPUT_REPLAY_RESULT,
        ),
    )
    for marker, label, expected in examples:
        example = json_example_after_marker(section, marker)
        if example is None:
            errors.append(
                f"{path}: canonical adopted-input {label} JSON is missing or ambiguous"
            )
            continue
        try:
            payload = strict_json_loads(example)
        except json.JSONDecodeError as error:
            errors.append(
                f"{path}: canonical adopted-input {label} JSON is invalid "
                f"(line {error.lineno}, column {error.colno}: {error.msg})"
            )
            continue
        if not strict_json_equal(payload, expected):
            errors.append(
                f"{path}: canonical adopted-input {label} JSON must equal "
                f"{expected_json_text(expected)}"
            )


def validate_canonical_request_adoption_example(
    documents: dict[str, str], errors: list[str]
) -> None:
    path = "docs/API-CONTRACT.md"
    section = require_markdown_section(
        documents,
        path,
        "Closed request shape and byte rules",
        level=3,
        errors=errors,
    )
    if section is None:
        return
    marker = "Adopted requests carry this exact object under `requestAdoption`:"
    example = json_example_after_marker(section, marker)
    if example is None:
        errors.append(
            f"{path}: canonical requestAdoption JSON is missing or ambiguous"
        )
        return
    try:
        payload = strict_json_loads(example)
    except json.JSONDecodeError as error:
        errors.append(
            f"{path}: canonical requestAdoption JSON is invalid "
            f"(line {error.lineno}, column {error.colno}: {error.msg})"
        )
        return

    if not isinstance(payload, dict) or set(payload) != set(
        EXPECTED_REQUEST_ADOPTION_KEYS
    ):
        errors.append(
            f"{path}: canonical requestAdoption JSON must contain exactly "
            "contract, key, and requestDigest"
        )
        return

    if not strict_json_equal(payload["contract"], REQUEST_ADOPTION_CONTRACT):
        errors.append(
            f"{path}: canonical requestAdoption contract must equal "
            f"{REQUEST_ADOPTION_CONTRACT}"
        )
    key = payload["key"]
    if not isinstance(key, str) or REQUEST_ADOPTION_KEY.fullmatch(key) is None:
        errors.append(
            f"{path}: canonical requestAdoption key must be representative valid ASCII"
        )
    digest = payload["requestDigest"]
    if (
        not isinstance(digest, str)
        or REQUEST_ADOPTION_DIGEST.fullmatch(digest) is None
    ):
        errors.append(
            f"{path}: canonical requestAdoption requestDigest must be "
            "sha256: plus 64 lowercase hexadecimal characters"
        )


def validate_canonical_request_adoption_rules(
    documents: dict[str, str], errors: list[str]
) -> None:
    path = "docs/API-CONTRACT.md"
    section = require_markdown_section(
        documents,
        path,
        "Closed request shape and byte rules",
        level=3,
        errors=errors,
    )
    if section is None:
        return

    table = require_markdown_table(
        section,
        ("Field", "Exact rule"),
        path=path,
        label="requestAdoption rules",
        errors=errors,
    )
    if table is not None:
        expected_fields = set(REQUEST_ADOPTION_RULE_CELLS)
        actual_fields = [
            markdown_cell_value(markdown_table_cell(table, row, "Field"))
            for row in table.rows
        ]
        if (
            len(actual_fields) != len(expected_fields)
            or set(actual_fields) != expected_fields
        ):
            errors.append(
                f"{path}: requestAdoption rule table must contain exactly "
                "contract, key, and requestDigest"
            )
        for field, expected_rule in REQUEST_ADOPTION_RULE_CELLS.items():
            rows = markdown_table_rows(table, {"Field": field})
            if len(rows) != 1:
                errors.append(
                    f"{path}: requestAdoption rule table must contain one "
                    f"{field} row (found {len(rows)})"
                )
                continue
            actual_rule = markdown_cell_value(
                markdown_table_cell(table, rows[0], "Exact rule")
            )
            if actual_rule != expected_rule:
                errors.append(
                    f"{path}: requestAdoption {field} Exact rule cell must equal "
                    f"{expected_rule}"
                )

    closed_claim = require_marked_paragraph(
        section,
        "The object is closed:",
        path=path,
        label="canonical requestAdoption closed-shape claim",
        errors=errors,
    )
    if closed_claim is not None and normalized_markdown_text(
        closed_claim
    ) != normalized_markdown_text(REQUEST_ADOPTION_CLOSED_SHAPE_CLAIM):
        errors.append(
            f"{path}: canonical requestAdoption closed-shape claim must equal "
            f"{REQUEST_ADOPTION_CLOSED_SHAPE_CLAIM}"
        )

    byte_claim = require_marked_paragraph(
        section,
        "Coven performs",
        path=path,
        label="canonical requestAdoption byte-preservation claim",
        errors=errors,
    )
    if byte_claim is not None and normalized_markdown_text(
        byte_claim
    ) != normalized_markdown_text(REQUEST_ADOPTION_BYTE_PRESERVATION_CLAIM):
        errors.append(
            f"{path}: canonical requestAdoption byte-preservation claim must equal "
            f"{REQUEST_ADOPTION_BYTE_PRESERVATION_CLAIM}"
        )


def validate_canonical_o3_error_contract(
    documents: dict[str, str], errors: list[str]
) -> None:
    path = "docs/API-CONTRACT.md"
    matrix_section = require_markdown_section(
        documents, path, "O3 error matrix", level=3, errors=errors
    )
    if matrix_section is not None:
        table = require_markdown_table(
            matrix_section,
            ("Code", "Status", "Phase and condition", "Exact message and details"),
            path=path,
            label="O3 error matrix",
            errors=errors,
        )
        if table is not None:
            actual_codes = [
                markdown_cell_value(markdown_table_cell(table, row, "Code"))
                for row in table.rows
            ]
            if (
                len(actual_codes) != len(O3_ERROR_STATUSES)
                or set(actual_codes) != set(O3_ERROR_STATUSES)
            ):
                errors.append(
                    f"{path}: O3 error matrix must contain exactly the expected "
                    "adopted-operation codes"
                )
            for code, expected_status in O3_ERROR_STATUSES.items():
                rows = markdown_table_rows(table, {"Code": code})
                if len(rows) != 1:
                    errors.append(
                        f"{path}: O3 error matrix must contain one {code} row "
                        f"(found {len(rows)})"
                    )
                    continue
                status = markdown_cell_value(
                    markdown_table_cell(table, rows[0], "Status")
                )
                if status != expected_status:
                    errors.append(
                        f"{path}: O3 error {code} status must equal {expected_status}"
                    )

    paths_section = require_markdown_section(
        documents, path, "Metadata isolation and privacy", level=3, errors=errors
    )
    if paths_section is None:
        return
    paths_table = require_markdown_table(
        paths_section,
        ("Condition", "error.details.fields"),
        path=path,
        label="O3 static field paths",
        errors=errors,
    )
    if paths_table is None:
        return
    for condition_marker, expected_path in O3_STATIC_FIELD_PATHS:
        rows = markdown_table_rows_containing(
            paths_table, {"Condition": condition_marker}
        )
        if len(rows) != 1:
            errors.append(
                f"{path}: O3 static field paths must contain one "
                f"{condition_marker!r} row (found {len(rows)})"
            )
            continue
        actual = strict_inline_json(
            markdown_table_cell(paths_table, rows[0], "error.details.fields")
        )
        if not strict_json_equal(actual, [expected_path]):
            errors.append(
                f"{path}: O3 static field path for {condition_marker!r} "
                f"must equal {expected_json_text([expected_path])}"
            )


def recovery_excludes_adopted_and_reserved(text: str) -> bool:
    normalized = normalized_markdown_text(
        text.replace("-", " "), lowercase=True
    )
    statements = re.split(r"(?<=[.!?])\s+", normalized)
    launch_adoption = re.compile(
        r"\b(?:keyed\s+)?(?:launch\s+adoption|adopted\s+launch(?:\s+rows?)?)\b"
    )
    historical_reservation = re.compile(
        r"\bhistorical(?:\s+attempt)?\s+reservations?\b"
    )
    contradictory = re.compile(
        r"\bwithout\s+exclud|\b(?:is|are|was|were)\s+not\s+excluded\b|"
        r"\b(?:do|does|did)\s+not\s+exclude\b|"
        r"\b(?:includes?|process(?:es|ed|ing)?)\b|"
        r"\balso\s+(?:fails?|marks?|recovers?)\b"
    )
    active_exclusion = re.compile(
        r"\bgeneric\s+stale(?:\s+created)?\s+recovery\s+excludes\s+"
        r"(?:every\s+session\s+with\s+)?(?:a\s+)?(?:keyed\s+)?"
        r"(?:launch\s+adoption|adopted\s+launch(?:\s+rows?)?)\s+"
        r"(?:or|and)\s+historical(?:\s+attempt)?\s+reservations?\b"
    )
    excluded_selection = re.compile(
        r"\brows?\s+without\s+(?:keyed\s+)?launch\s+adoption"
        r"(?:\s+evidence)?\s+(?:or|and)\s+historical(?:\s+attempt)?"
        r"\s+reservation\s+evidence\b"
    )
    evidence_excludes_row = re.compile(
        r"\b(?:a\s+)?launch\s+adoption\s+(?:or|and)\s+historical"
        r"(?:\s+attempt)?\s+reservation\s+excludes?\s+the\s+row\s+from\s+"
        r"(?:that|generic\s+stale(?:\s+created)?)\s+recovery\b"
    )
    affirmative = False
    contradicted = False
    for statement in statements:
        if not (
            re.search(r"\bstale\b", statement)
            and re.search(
                r"\b(?:recover(?:y|s|ed|ing)?|marks?\s+only)\b",
                statement,
            )
            and launch_adoption.search(statement)
            and historical_reservation.search(statement)
        ):
            continue
        if contradictory.search(statement):
            contradicted = True
            continue
        if (
            active_exclusion.search(statement)
            or excluded_selection.search(statement)
            or evidence_excludes_row.search(statement)
        ):
            affirmative = True
    return affirmative and not contradicted


def has_stale_created_failure_recovery(
    text: str, *, created_context: bool = False
) -> bool:
    units: list[tuple[str, bool]] = []
    prose_lines: list[str] = []
    for line in text.lower().splitlines():
        if line.lstrip().startswith("|"):
            cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
            if cells and cells[0].strip("`") == "created":
                row_text = " ".join(cells[1:])
                units.extend(
                    (unit, True)
                    for unit in re.split(r"(?<=[.!?])\s+", row_text)
                )
            prose_lines.append("")
        else:
            prose_lines.append(line)
    for paragraph in re.split(r"\n\s*\n", "\n".join(prose_lines)):
        sentence_text = re.sub(r"\s*\n\s*", " ", paragraph)
        units.extend(
            (unit, False) for unit in re.split(r"(?<=[.!?])\s+", sentence_text)
        )

    for unit, created_row in units:
        recovery = re.search(r"\b(?:recover(?:s|ed)?|marks?|moves?)\b", unit)
        if not (
            recovery
            and re.search(r"\bstale\b", unit)
            and re.search(r"\bunowned\b", unit)
            and (
                created_context
                or created_row
                or re.search(r"\bcreated\b", unit)
            )
            and recovery_excludes_adopted_and_reserved(unit)
        ):
            continue
        recovery_clause = unit[recovery.start() :]
        targets = re.findall(
            r"(?:\b(?:as|to)\s+|(?:->|→)\s*)`?"
            r"(created|running|idle|completed|failed|killed|orphaned)`?\b",
            recovery_clause,
        )
        targets.extend(
            re.findall(
                r"\bor\s+`?(created|running|idle|completed|failed|killed|orphaned)`?\b",
                recovery_clause,
            )
        )
        if targets and set(targets) == {"failed"}:
            return True
    return False


def validate_lifecycle_recovery_docs(
    documents: dict[str, str], errors: list[str]
) -> None:
    for path, (heading, level, marker) in LIFECYCLE_RECOVERY_CLAIMS.items():
        section = require_markdown_section(
            documents, path, heading, level=level, errors=errors
        )
        if section is None:
            continue
        claim = require_marked_paragraph(
            section,
            marker,
            path=path,
            label="stale-created recovery",
            errors=errors,
        )
        if claim is None:
            continue
        if not recovery_excludes_adopted_and_reserved(claim):
            errors.append(
                f"{path}: stale-created recovery must exclude keyed launch "
                "adoptions and historical/null-key reservations"
            )

        target_text = claim
        created_context = False
        if path == "docs/API-CONTRACT.md":
            target_section = require_markdown_section(
                documents,
                path,
                "Session record shape (`v1`)",
                level=2,
                errors=errors,
            )
            if target_section is None:
                continue
            table = require_markdown_table(
                target_section,
                ("Harness-session status", "Terminal?", "Meaning"),
                path=path,
                label="session lifecycle",
                errors=errors,
            )
            if table is None:
                continue
            rows = markdown_table_rows(
                table, {"Harness-session status": "created"}
            )
            if len(rows) != 1:
                errors.append(
                    f"{path}: expected one created lifecycle row (found {len(rows)})"
                )
                continue
            target_text = markdown_table_cell(table, rows[0], "Meaning")
            created_context = True

        if not has_stale_created_failure_recovery(
            target_text, created_context=created_context
        ):
            errors.append(f"{path}: stale created recovery is missing")


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
        if not re.search(
            r"not proof (?:of acknowledged process termination|"
            r"that process termination was acknowledged)",
            lowered,
        ):
            errors.append(f"{path}: killed acknowledgement boundary is missing")
        if not all(
            term in lowered
            for term in ("synthetic", "`active`", "not a harness-session state")
        ):
            errors.append(f"{path}: synthetic active distinction is missing")
        if not re.search(r"stored separately (?:in|as) `archived_at`", lowered):
            errors.append(f"{path}: archive separation is missing")

    validate_lifecycle_recovery_docs(documents, errors)
    return errors


def validate_session_launch_policy_docs(documents: dict[str, str]) -> list[str]:
    """Pin the transport and field-list parts of the privileged launch contract."""
    errors: list[str] = []
    for path in SESSION_LAUNCH_POLICY_DOCS:
        text = documents[path]
        lowered = text.lower()
        if "sessionLaunchPolicy" not in text:
            errors.append(f"{path}: sessionLaunchPolicy capability is missing")
        if "tcp" not in lowered or not re.search(
            r"owner[-\s]+(?:gated[-\s]+)?local[-\s]+ipc", lowered
        ):
            errors.append(f"{path}: launchPolicy transport boundary is missing")

    for path in HEALTH_CAPABILITY_EXAMPLE_DOCS:
        for field in HEALTH_CAPABILITY_FIELDS:
            if f'"{field}"' not in documents[path]:
                errors.append(f"{path}: health example missing {field}")

    for path in HEALTH_CAPABILITY_LIST_DOCS:
        for field in HEALTH_CAPABILITY_FIELDS:
            if f"`{field}`" not in documents[path]:
                errors.append(f"{path}: capability list missing {field}")

    expected_count = len(HEALTH_CAPABILITY_FIELDS)
    for path in HEALTH_CAPABILITY_COUNT_DOCS:
        if not re.search(rf"\ball {expected_count}\b[^.\n]*\bfields\b", documents[path]):
            errors.append(f"{path}: health capability field count is stale")

    contract = documents["docs/API-CONTRACT.md"]
    for literal in O3_REQUIRED_LITERALS:
        if literal not in contract:
            errors.append(
                f"docs/API-CONTRACT.md: missing O3 contract literal {literal}"
            )
    return errors


def validate_adopted_status_tables(documents: dict[str, str]) -> list[str]:
    errors: list[str] = []
    routes = O3_ADOPTED_ROUTES

    canonical_table, canonical_rows = validate_separate_adopted_table(
        documents,
        errors,
        path="docs/API-CONTRACT.md",
        heading="Adopted routes, compatibility, and responses",
        level=3,
        headers=(
            "Method and path",
            "Required body metadata",
            "First adoption",
            "Exact replay",
        ),
        rows={
            route: (
                route,
                {"Method and path": f"POST {route}"},
                {},
            )
            for route in routes
        },
        first_header="First adoption",
        replay_header="Exact replay",
        table_label="canonical adopted routes",
        row_kind="route",
        shape_references=True,
    )
    if canonical_table is not None:
        for route, row in canonical_rows.items():
            metadata = markdown_table_cell(
                canonical_table, row, "Required body metadata"
            )
            for field in ("executionBinding", "requestAdoption"):
                if f"`{field}`" not in metadata:
                    errors.append(
                        f"docs/API-CONTRACT.md: {route} Required body metadata "
                        f"cell must include {field}"
                    )

    reference_path = "docs/reference/api.md"
    reference_table, reference_rows = validate_combined_adopted_table(
        documents,
        errors,
        path=reference_path,
        heading="Sessions and events",
        level=2,
        headers=(
            "Method",
            "Path",
            "Purpose",
            "Body / query",
            "Success",
            "Errors",
        ),
        rows={
            route: {"Method": "POST", "Path": route}
            for route in routes
        },
        success_header="Success",
        table_label="sessions and events",
    )

    contract_path = "docs/reference/api-contract.md"
    contract_table, contract_rows = validate_separate_adopted_table(
        documents,
        errors,
        path=contract_path,
        heading="Negotiation",
        level=2,
        headers=("Route", "First adoption", "Exact replay", "Adoption errors"),
        rows={
            route: (
                route,
                {"Route": f"POST {route}"},
                {},
            )
            for route in routes
        },
        first_header="First adoption",
        replay_header="Exact replay",
        table_label="negotiated adopted routes",
        row_kind="route",
    )

    validate_combined_adopted_table(
        documents,
        errors,
        path="docs/daemon/socket-api.md",
        heading="Endpoints",
        level=2,
        headers=("Endpoint", "Purpose"),
        rows={
            route: {"Endpoint": f"POST {route}"}
            for route in routes
        },
        success_header="Purpose",
        table_label="socket endpoints",
    )

    openclaw_path = "packages/openclaw-coven/README.md"
    method_names = {
        "/api/v1/adopted-sessions": "launchAdoptedSession",
        "/api/v1/sessions/:id/adopted-input": "sendAdoptedInput",
    }
    validate_separate_adopted_table(
        documents,
        errors,
        path=openclaw_path,
        heading="Adopted client methods",
        level=3,
        headers=("Method", "Dedicated route", "First adoption", "Exact replay"),
        rows={
            route: (
                method_names[route],
                {"Method": method_names[route]},
                {"Dedicated route": f"POST {route}"},
            )
            for route in routes
        },
        first_header="First adoption",
        replay_header="Exact replay",
        table_label="adopted client methods",
        row_kind="method",
    )

    for path, table, rows, header in (
        (reference_path, reference_table, reference_rows, "Errors"),
        (contract_path, contract_table, contract_rows, "Adoption errors"),
    ):
        if table is None:
            continue
        for route, row in rows.items():
            cell = markdown_table_cell(table, row, header)
            for code in O3_ADOPTION_ERRORS:
                if code not in cell:
                    errors.append(f"{path}: {route} row missing O3 error {code}")

    validate_canonical_adopted_input_results(documents, errors)
    validate_canonical_request_adoption_example(documents, errors)
    validate_canonical_request_adoption_rules(documents, errors)
    validate_canonical_o3_error_contract(documents, errors)
    return errors


def collect_o3_negotiation_claim(
    documents: dict[str, str],
    path: str,
    surface: O3NegotiationSurface,
    errors: list[str],
) -> str | None:
    section = require_markdown_section(
        documents,
        path,
        surface.heading,
        level=surface.level,
        errors=errors,
    )
    if section is None:
        return None
    paragraphs: list[str] = []
    for marker in surface.markers:
        paragraph = require_marked_paragraph(
            section,
            marker,
            path=path,
            label="O3 negotiation claim",
            errors=errors,
        )
        if paragraph is None:
            return None
        paragraphs.append(paragraph)
    return "\n\n".join(paragraphs)


def o3_claim_contains_fragments(
    claim: str, fragments: tuple[str, ...]
) -> bool:
    """Require canonical declarative clauses instead of nearby keywords."""
    normalized = normalized_markdown_text(claim, lowercase=True)
    return all(
        normalized_markdown_text(fragment, lowercase=True) in normalized
        for fragment in fragments
    )


def o3_claim_negates_request_adoption_gate(claim: str) -> bool:
    normalized = normalized_markdown_text(claim, lowercase=True)
    for statement in re.split(r"(?<=[.;!?])\s+", normalized):
        if "requestadoptioncontracts" not in statement:
            continue
        if re.search(
            r"\b(?:do|does|did|must)\s+not\s+"
            r"(?:check|inspect|require|verify|gate)\b|"
            r"\bwithout\s+(?:checking|inspecting|requiring|verifying|gating)\b",
            statement,
        ):
            return True
    return False


def o3_claim_negates_api_version_gate(claim: str) -> bool:
    normalized = normalized_markdown_text(claim, lowercase=True)
    for statement in re.split(r"(?<=[.;!?])\s+", normalized):
        if "apiversion" not in statement:
            continue
        if re.search(
            r"\b(?:do|does|did|must)\s+not\s+"
            r"(?:check|inspect|require|verify|gate)\b|"
            r"\bwithout\s+(?:checking|inspecting|requiring|verifying|gating)\b|"
            r"\bapiversion\b[^.;]{0,80}\b(?:need\s+not|is\s+not\s+required)\b",
            statement,
        ):
            return True
    return False


def o3_claim_orders_api_version_before_request_adoption(claim: str) -> bool:
    normalized = normalized_markdown_text(claim, lowercase=True)
    if re.search(
        r"\brequestadoptioncontracts\b[^.;]{0,160}"
        r"\b(?:before|first)\b[^.;]{0,160}\b(?:health\.)?apiversion\b|"
        r"\b(?:health\.)?apiversion\b[^.;]{0,160}\b(?:only\s+)?after\b"
        r"[^.;]{0,160}\brequestadoptioncontracts\b",
        normalized,
    ):
        return False
    api_version_index = normalized.find("health.apiversion")
    request_adoption_index = normalized.find(
        "health.capabilities.requestadoptioncontracts"
    )
    return (
        api_version_index >= 0
        and request_adoption_index >= 0
        and api_version_index < request_adoption_index
    )


def o3_claim_negates_per_request_proof(claim: str) -> bool:
    normalized = normalized_markdown_text(claim, lowercase=True)
    for statement in re.split(r"(?<=[.;!?])\s+", normalized):
        if not re.search(r"\b(?:o2|executionbinding|proof)\b", statement):
            continue
        if re.search(
            r"\b(?:must|should|do(?:es)?|did)\s+not\s+"
            r"(?:carry|include|require)\b|"
            r"\bneed\s+not\s+(?:carry|include)\b|"
            r"\bmay\s+omit\b|\boptional\b",
            statement,
        ):
            return True
    return False


def o3_claim_allows_legacy_fallback(claim: str) -> bool:
    normalized = normalized_markdown_text(claim, lowercase=True)
    return bool(
        re.search(
            r"\b(?:may|can|will)\s+(?:fall\s+back|retry)\b|"
            r"\bmay\s+retry\s+(?:a\s+)?legacy\s+mutation\b|"
            r"\bdo(?:es)?\s+not\s+prohibit\s+(?:a\s+)?legacy\s+fallback\b",
            normalized,
        )
    )


def o3_claim_positively_gates_on_execution_binding(claim: str) -> bool:
    normalized = re.sub(r"\s+", " ", claim.replace("`", " "))
    for statement in re.split(r"(?<=[.;])\s+", normalized):
        if "executionBindingContracts" not in statement or not re.search(
            r"\b(?:checks?|requires?|verif(?:y|ies)|gates?)\b",
            statement,
            re.IGNORECASE,
        ):
            continue
        if re.search(
            r"\b(?:do|does|must)\s+not\b[^.;]{0,80}"
            r"\b(?:check|require|verify|gate)",
            statement,
            re.IGNORECASE,
        ):
            continue
        return True
    return False


def validate_o3_negotiation_claims(
    documents: dict[str, str], errors: list[str]
) -> None:
    for path, surface in O3_NEGOTIATION_SURFACES.items():
        claim = collect_o3_negotiation_claim(documents, path, surface, errors)
        if claim is None:
            continue
        assertions = O3_NEGOTIATION_REQUIRED_FRAGMENTS[path]
        normalized = normalized_markdown_text(claim)
        api_version_gate_is_valid = (
            o3_claim_contains_fragments(claim, assertions["api_version"])
            and surface.api_version_literal.lower() in normalized.lower()
            and not o3_claim_negates_api_version_gate(claim)
        )
        if not api_version_gate_is_valid:
            errors.append(
                f"{path}: O3 negotiation claim must require the exact "
                f"health.apiVersion {surface.api_version_literal}"
            )
        request_adoption_gate_is_valid = o3_claim_contains_fragments(
            claim, assertions["gate"]
        ) and not o3_claim_negates_request_adoption_gate(claim)
        if not request_adoption_gate_is_valid:
            errors.append(
                f"{path}: O3 negotiation claim must gate on "
                "requestAdoptionContracts"
            )
        if surface.literal_claim.lower() not in normalized.lower():
            errors.append(
                f"{path}: O3 negotiation claim must require the exact "
                f"{surface.literal_claim}"
            )
        if (
            api_version_gate_is_valid
            and request_adoption_gate_is_valid
            and not o3_claim_orders_api_version_before_request_adoption(claim)
        ):
            errors.append(
                f"{path}: O3 negotiation claim must check health.apiVersion "
                "before requestAdoptionContracts"
            )
        if o3_claim_positively_gates_on_execution_binding(claim):
            errors.append(
                f"{path}: O3 negotiation claim must not gate adopted methods "
                "on executionBindingContracts"
            )
        if surface.owns_proof_boundary and (
            not o3_claim_contains_fragments(claim, assertions["proof"])
            or o3_claim_negates_per_request_proof(claim)
        ):
            errors.append(
                f"{path}: O3 negotiation claim must retain exact per-request "
                "executionBinding proof"
            )
        if surface.owns_no_fallback and (
            not o3_claim_contains_fragments(claim, assertions["no_fallback"])
            or o3_claim_allows_legacy_fallback(claim)
        ):
            errors.append(
                f"{path}: O3 negotiation claim must prohibit legacy mutation "
                "fallback"
            )


def validate_o3_document_structures(documents: dict[str, str]) -> list[str]:
    """Validate O3 claims only inside the Markdown structures that own them."""
    errors: list[str] = []

    canonical_path = "docs/API-CONTRACT.md"
    canonical_section = require_markdown_section(
        documents,
        canonical_path,
        "`GET /api/v1/health`",
        level=2,
        errors=errors,
    )
    canonical_examples = (
        fenced_code_blocks(canonical_section, "json")
        if canonical_section is not None
        else []
    )
    canonical_example = (
        canonical_examples[0] if len(canonical_examples) == 1 else None
    )
    validate_health_json_example(
        canonical_example,
        path=canonical_path,
        label="canonical health example",
        errors=errors,
    )

    for path, heading in HEALTH_EXAMPLE_SECTIONS.items():
        section = require_markdown_section(
            documents, path, heading, level=2, errors=errors
        )
        examples = (
            http_json_examples(section, "GET /api/v1/health")
            if section is not None
            else []
        )
        example = examples[0] if len(examples) == 1 else None
        validate_health_json_example(
            example,
            path=path,
            label="health example",
            errors=errors,
        )

    validate_capability_value_table(documents, errors)

    expected_count = len(HEALTH_CAPABILITY_FIELDS)
    for path, (heading, marker) in HEALTH_CAPABILITY_LISTS.items():
        section = markdown_section(documents[path], heading)
        capability_lists = (
            markdown_paragraphs(section, marker) if section else []
        )
        if not capability_lists:
            errors.append(f"{path}: health capability list is missing")
            continue
        if len(capability_lists) != 1:
            errors.append(
                f"{path}: health capability list is ambiguous "
                f"(found {len(capability_lists)})"
            )
            continue
        capability_list = capability_lists[0]
        for field in HEALTH_CAPABILITY_FIELDS:
            if f"`{field}`" not in capability_list:
                errors.append(f"{path}: capability list missing {field}")
        normalized = re.sub(r"\s+", " ", capability_list)
        if not re.search(rf"\ball {expected_count}\b[^.]*\bfields\b", normalized):
            errors.append(f"{path}: health capability field count is stale")

    validate_o3_negotiation_claims(documents, errors)

    errors.extend(validate_adopted_status_tables(documents))

    socket_path = "docs/daemon/socket-api.md"
    socket_section = markdown_section(documents[socket_path], "Endpoints")
    error_list = (
        markdown_paragraph(socket_section, "Their O3-specific errors are")
        if socket_section
        else None
    )
    if error_list is None:
        errors.append(f"{socket_path}: O3 error list is missing")
    else:
        for code in O3_ADOPTION_ERRORS:
            if code not in error_list:
                errors.append(f"{socket_path}: O3 error list missing {code}")

    openclaw_path = "packages/openclaw-coven/README.md"
    compatibility = markdown_section(
        documents[openclaw_path], "Version compatibility"
    )
    fixture_items = (
        markdown_list_items(compatibility, "GET /api/v1/health")
        if compatibility
        else []
    )
    fixture_item = fixture_items[0] if len(fixture_items) == 1 else None
    fixture_fields = (
        "sessions",
        "events",
        "eventCursor",
        "structuredErrors",
        "executionBindingContracts",
        "requestAdoptionContracts",
    )
    if not fixture_items:
        errors.append(f"{openclaw_path}: minimal health fixture claim is missing")
    elif len(fixture_items) != 1:
        errors.append(
            f"{openclaw_path}: minimal health fixture claim is ambiguous "
            f"(found {len(fixture_items)})"
        )
    else:
        for field in fixture_fields:
            if f"`{field}`" not in fixture_item:
                errors.append(
                    f"{openclaw_path}: minimal health fixture missing {field}"
                )

    dto_claims = (
        markdown_paragraphs(
            compatibility,
            "The complete current 16-field Rust health-capability DTO",
        )
        if compatibility
        else []
    )
    if not dto_claims:
        errors.append(f"{openclaw_path}: canonical health DTO reference is missing")
    elif len(dto_claims) != 1:
        errors.append(
            f"{openclaw_path}: canonical health DTO reference is ambiguous "
            f"(found {len(dto_claims)})"
        )
    elif not all(
        literal in dto_claims[0]
        for literal in (
            "16-field",
            "../../docs/API-CONTRACT.md#get-apiv1health",
            "`requestAdoptionContracts`",
        )
    ):
        errors.append(f"{openclaw_path}: canonical health DTO reference is missing")

    adopted_methods = markdown_section(
        documents[openclaw_path], "Adopted client methods", level=3
    )
    if adopted_methods is None:
        errors.append(f"{openclaw_path}: adopted client methods section is missing")
    else:
        if (
            "../../docs/API-CONTRACT.md#psyche-request-adoption-contract-v1"
            not in adopted_methods
        ):
            errors.append(
                f"{openclaw_path}: adopted method negotiation boundary is missing"
            )

    cli_path = "packages/cli/README.md"
    commands = markdown_section(documents[cli_path], "Commands")
    normalized_commands = re.sub(r"\s+", " ", commands or "")
    if not all(
        literal in normalized_commands
        for literal in (
            "non-running",
            "adoption",
            "reservation",
            "retention/fence",
            "O3",
            "future",
            "approved",
        )
    ) or not re.search(
        r"\bpermanent\b[^.]*\bO3\b|\bO3\b[^.]*\bpermanent\b",
        normalized_commands,
        re.IGNORECASE,
    ):
        errors.append(f"{cli_path}: O3 sacrifice retention boundary is missing")

    return errors


def main() -> int:
    paths = tuple(
        dict.fromkeys(
            CONTRACT_DOCS
            + LIFECYCLE_DOCS
            + HEALTH_GUIDANCE_DOCS
            + SESSION_LAUNCH_POLICY_DOCS
            + STRUCTURED_O3_DOCS
            + PACKAGE_README_DOCS
        )
    )
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
    errors.extend(validate_session_launch_policy_docs(documents))
    errors.extend(validate_o3_document_structures(documents))

    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
