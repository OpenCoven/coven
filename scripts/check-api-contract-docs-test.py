from __future__ import annotations

import importlib.util
import io
import json
import pathlib
import tempfile
import unittest
from contextlib import redirect_stderr
from unittest import mock


SCRIPT = pathlib.Path(__file__).with_name("check-api-contract-docs.py")
SPEC = importlib.util.spec_from_file_location("check_api_contract_docs", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class ApiContractDocsTests(unittest.TestCase):
    PUBLIC_CONTRACT_DOCS = (
        "packages/openclaw-coven/README.md",
        "docs/OPERATIONAL-MODEL.md",
    )
    EXTRA_HEALTH_GUIDANCE_DOCS = (
        "README.md",
        "docs/CLIENT-INTEGRATION.md",
        "docs/daemon/health.md",
    )
    O3_STRUCTURE_DOCS = (
        "docs/API-CONTRACT.md",
        "docs/reference/api.md",
        "docs/reference/api-contract.md",
        "docs/daemon/socket-api.md",
        "packages/openclaw-coven/README.md",
        "packages/cli/README.md",
    )
    EXPECTED_O3_ERROR_STATUSES = {
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
    MISSING = object()

    def canonical_documents(self) -> dict[str, str]:
        contract = """
Clients negotiate compatibility with GET /api/v1/health and coven.daemon.v1.
Capabilities advertise availability and never grant permission.
GET /api/v1/api-version is a legacy route-family diagnostic returning literal `v1`.
New clients must not use the legacy route as proof of coven.daemon.v1 compatibility.

## Session record shape (`v1`)
| Harness-session status | Terminal? | Meaning |
|---|---|---|
| `created` | No | stale unowned rows without keyed launch-adoption or historical reservation evidence recover as `failed`. |
| `running` | No | live |
| `idle` | No | reusable |
| `completed` | Yes | success |
| `failed` | Yes | failure |
| `killed` | Yes | accepted |
| `orphaned` | Yes | unresolved |
`killed` is not proof of acknowledged process termination.
Synthetic `active` is not a harness-session state.
Archive visibility is stored separately in `archived_at`.

### Lifecycle, ambiguity, and retention

Generic stale-created recovery excludes every session with a keyed launch adoption or historical attempt reservation.

## Orphan recovery

Generic stale-created recovery marks only stale unowned `created` rows without launch-adoption or historical reservation evidence as `failed`.

Marks only stale unowned `created` rows without launch-adoption or historical reservation evidence as `failed`.
"""
        health_docs = getattr(module, "HEALTH_GUIDANCE_DOCS", ())
        return {
            path: contract
            for path in module.CONTRACT_DOCS + module.LIFECYCLE_DOCS + health_docs
        }

    def replace_legacy_explanation(
        self, documents: dict[str, str], path: str, replacement: str
    ) -> None:
        documents[path] = documents[path].replace(
            "GET /api/v1/api-version is a legacy route-family diagnostic "
            "returning literal `v1`.\n"
            "New clients must not use the legacy route as proof of "
            "coven.daemon.v1 compatibility.",
            replacement,
        )

    def o3_structure_documents(self) -> dict[str, str]:
        return {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in self.O3_STRUCTURE_DOCS
        }

    def lifecycle_documents(self) -> dict[str, str]:
        return {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.LIFECYCLE_DOCS
        }

    def canonical_request_adoption_example(
        self, documents: dict[str, str]
    ) -> str:
        path = "docs/API-CONTRACT.md"
        section = module.markdown_section(
            documents[path], "Closed request shape and byte rules", level=3
        )
        self.assertIsNotNone(section)
        example = module.json_example_after_marker(
            section or "",
            "Adopted requests carry this exact object under `requestAdoption`:",
        )
        self.assertIsNotNone(example)
        return example or ""

    def replace_request_adoption_payload(
        self, documents: dict[str, str], payload: dict[str, object]
    ) -> None:
        path = "docs/API-CONTRACT.md"
        example = self.canonical_request_adoption_example(documents)
        documents[path] = self.replace_once(
            documents[path], example, json.dumps(payload, indent=2)
        )

    def canonical_request_adoption_rule_table(
        self, documents: dict[str, str]
    ) -> module.MarkdownTable:
        path = "docs/API-CONTRACT.md"
        section = module.markdown_section(
            documents[path], "Closed request shape and byte rules", level=3
        )
        self.assertIsNotNone(section)
        table = module.markdown_table(section or "", ("Field", "Exact rule"))
        self.assertIsNotNone(table)
        return table

    def mutate_request_adoption_rule(
        self,
        documents: dict[str, str],
        field: str,
        old: str,
        new: str,
    ) -> None:
        path = "docs/API-CONTRACT.md"
        table = self.canonical_request_adoption_rule_table(documents)
        rows = module.markdown_table_rows(table, {"Field": field})
        self.assertEqual(len(rows), 1)
        row = rows[0]
        cells = list(row.cells)
        index = table.headers.index("Exact rule")
        cells[index] = self.replace_once(cells[index], old, new)
        documents[path] = self.replace_once(
            documents[path], row.source, "| " + " | ".join(cells) + " |"
        )

    def mutate_o3_negotiation_claim(
        self,
        documents: dict[str, str],
        path: str,
        old: str,
        new: str,
    ) -> None:
        surface = module.O3_NEGOTIATION_SURFACES[path]
        section = module.markdown_section(
            documents[path], surface.heading, level=surface.level
        )
        self.assertIsNotNone(section)
        candidates = []
        for marker in surface.markers:
            paragraph = module.markdown_paragraph(section or "", marker)
            self.assertIsNotNone(paragraph)
            if old in (paragraph or ""):
                candidates.append(paragraph or "")
        self.assertEqual(len(candidates), 1, (path, old))
        paragraph = candidates[0]
        documents[path] = self.replace_once(
            documents[path], paragraph, self.replace_once(paragraph, old, new)
        )
        documents[path] += (
            "\n\n## Checker decoy\n"
            "`requestAdoptionContracts` contains the exact "
            "`psyche.request_adoption.v1` literal. The capability does not "
            "replace the complete exact O2 proof in every request and never "
            "falls back to a legacy mutation.\n"
        )

    def health_example_cases(
        self,
    ) -> tuple[tuple[str, str, str | None, str], ...]:
        return (
            (
                "docs/API-CONTRACT.md",
                "`GET /api/v1/health`",
                None,
                "canonical health example",
            ),
            (
                "docs/reference/api-contract.md",
                "Negotiation",
                "GET /api/v1/health",
                "health example",
            ),
            (
                "docs/daemon/socket-api.md",
                "Handshake",
                "GET /api/v1/health",
                "health example",
            ),
        )

    def replace_once(self, text: str, old: str, new: str) -> str:
        self.assertIn(old, text)
        return text.replace(old, new, 1)

    def mutate_health_capability(
        self,
        documents: dict[str, str],
        path: str,
        heading: str,
        field: str,
        value: object,
        *,
        request_line: str | None = None,
    ) -> None:
        section = module.markdown_section(documents[path], heading)
        self.assertIsNotNone(section)
        if request_line:
            example = module.http_json_example(section or "", request_line)
        else:
            example = module.fenced_code_block(section or "", "json")
        self.assertIsNotNone(example)
        payload = json.loads(example or "")
        capabilities = payload["capabilities"]
        if value is self.MISSING:
            del capabilities[field]
        else:
            capabilities[field] = value
        replacement = json.dumps(payload, indent=2)
        documents[path] = self.replace_once(documents[path], example or "", replacement)
        documents[path] += (
            f"\n\nDecoy: `{field}` has "
            '`["psyche.execution_binding.v1", "psyche.request_adoption.v1"]`.\n'
        )

    def adopted_table_cases(self, operation: str) -> tuple[dict[str, object], ...]:
        route = {
            "launch": "/api/v1/adopted-sessions",
            "input": "/api/v1/sessions/:id/adopted-input",
        }[operation]
        method = {
            "launch": "launchAdoptedSession",
            "input": "sendAdoptedInput",
        }[operation]
        return (
            {
                "path": "docs/API-CONTRACT.md",
                "heading": "Adopted routes, compatibility, and responses",
                "level": 3,
                "headers": (
                    "Method and path",
                    "Required body metadata",
                    "First adoption",
                    "Exact replay",
                ),
                "criteria": {"Method and path": f"POST {route}"},
                "first": "First adoption",
                "replay": "Exact replay",
                "display": route,
            },
            {
                "path": "docs/reference/api.md",
                "heading": "Sessions and events",
                "level": 2,
                "headers": (
                    "Method",
                    "Path",
                    "Purpose",
                    "Body / query",
                    "Success",
                    "Errors",
                ),
                "criteria": {"Method": "POST", "Path": route},
                "combined": "Success",
                "display": route,
            },
            {
                "path": "docs/reference/api-contract.md",
                "heading": "Negotiation",
                "level": 2,
                "headers": (
                    "Route",
                    "First adoption",
                    "Exact replay",
                    "Adoption errors",
                ),
                "criteria": {"Route": f"POST {route}"},
                "first": "First adoption",
                "replay": "Exact replay",
                "display": route,
            },
            {
                "path": "docs/daemon/socket-api.md",
                "heading": "Endpoints",
                "level": 2,
                "headers": ("Endpoint", "Purpose"),
                "criteria": {"Endpoint": f"POST {route}"},
                "combined": "Purpose",
                "display": route,
            },
            {
                "path": "packages/openclaw-coven/README.md",
                "heading": "Adopted client methods",
                "level": 3,
                "headers": (
                    "Method",
                    "Dedicated route",
                    "First adoption",
                    "Exact replay",
                ),
                "criteria": {"Method": method},
                "first": "First adoption",
                "replay": "Exact replay",
                "display": method,
            },
        )

    def mutate_adopted_statuses(
        self, documents: dict[str, str], case: dict[str, object], operation: str
    ) -> None:
        path = str(case["path"])
        section = module.markdown_section(
            documents[path], str(case["heading"]), level=int(case["level"])
        )
        self.assertIsNotNone(section)
        table = module.markdown_table(section or "", tuple(case["headers"]))
        self.assertIsNotNone(table)
        rows = module.markdown_table_rows(table, dict(case["criteria"]))
        self.assertEqual(len(rows), 1)
        row = rows[0]
        cells = list(row.cells)
        first_status, replay_status = (
            ("201", "200") if operation == "launch" else ("202", "200")
        )
        if "combined" in case:
            index = table.headers.index(str(case["combined"]))
            sentinel = "SWAPPED_STATUS"
            cells[index] = (
                cells[index]
                .replace(first_status, sentinel, 1)
                .replace(replay_status, first_status, 1)
                .replace(sentinel, replay_status, 1)
            )
        else:
            first_index = table.headers.index(str(case["first"]))
            replay_index = table.headers.index(str(case["replay"]))
            cells[first_index] = cells[first_index].replace(
                first_status, replay_status, 1
            )
            cells[replay_index] = cells[replay_index].replace(
                replay_status, first_status, 1
            )
        replacement = "| " + " | ".join(cells) + " |"
        documents[path] = self.replace_once(documents[path], row.source, replacement)
        documents[path] += (
            f"\n\nDecoy for {case['display']}: first `{first_status}`, "
            f"replay `{replay_status}`.\n"
        )

    def replace_adopted_result_json(
        self,
        documents: dict[str, str],
        case: dict[str, object],
        expected: dict[str, object],
        replacement: str,
    ) -> None:
        path = str(case["path"])
        section = module.markdown_section(
            documents[path], str(case["heading"]), level=int(case["level"])
        )
        self.assertIsNotNone(section)
        table = module.markdown_table(section or "", tuple(case["headers"]))
        self.assertIsNotNone(table)
        rows = module.markdown_table_rows(table, dict(case["criteria"]))
        self.assertEqual(len(rows), 1)
        row = rows[0]
        cells = list(row.cells)
        expected_text = json.dumps(expected, separators=(",", ":"))
        indexes = [
            index for index, cell in enumerate(cells) if expected_text in cell
        ]
        self.assertEqual(len(indexes), 1)
        index = indexes[0]
        cells[index] = self.replace_once(cells[index], expected_text, replacement)
        documents[path] = self.replace_once(
            documents[path],
            row.source,
            "| " + " | ".join(cells) + " |",
        )

    def replace_launch_result_type(
        self,
        documents: dict[str, str],
        case: dict[str, object],
        phase: str,
        replacement: str,
    ) -> None:
        path = str(case["path"])
        section = module.markdown_section(
            documents[path], str(case["heading"]), level=int(case["level"])
        )
        self.assertIsNotNone(section)
        table = module.markdown_table(section or "", tuple(case["headers"]))
        self.assertIsNotNone(table)
        rows = module.markdown_table_rows(table, dict(case["criteria"]))
        self.assertEqual(len(rows), 1)
        row = rows[0]
        cells = list(row.cells)
        if "combined" in case:
            index = table.headers.index(str(case["combined"]))
            status = "201" if phase == "first" else "200"
            cells[index] = self.replace_once(
                cells[index],
                f"{status} SessionRecord",
                f"{status} {replacement}",
            )
        else:
            header = str(case[phase])
            index = table.headers.index(header)
            cells[index] = self.replace_once(
                cells[index], "SessionRecord", replacement
            )
        documents[path] = self.replace_once(
            documents[path], row.source, "| " + " | ".join(cells) + " |"
        )

    def test_accepts_canonical_handshake_and_lifecycle(self) -> None:
        self.assertEqual(module.validate_documents(self.canonical_documents()), [])

    def test_current_launch_policy_transport_and_capability_docs_are_guarded(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        self.assertEqual(module.validate_session_launch_policy_docs(documents), [])

    def test_launch_policy_docs_reject_a_missing_tcp_authority_boundary(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        path = "docs/daemon/capabilities-handshake.md"
        documents[path] = documents[path].replace("TCP", "remote transport")
        errors = module.validate_session_launch_policy_docs(documents)
        self.assertIn(f"{path}: launchPolicy transport boundary is missing", errors)

    def test_every_public_health_example_has_the_exact_capability_set(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        for path in module.HEALTH_CAPABILITY_EXAMPLE_DOCS:
            with self.subTest(path=path):
                changed = dict(documents)
                changed[path] = changed[path].replace(
                    '"sessionLaunchPolicy": true,', "", 1
                )
                errors = module.validate_session_launch_policy_docs(changed)
                self.assertIn(
                    f"{path}: health example missing sessionLaunchPolicy", errors
                )

    def test_every_public_health_count_is_guarded(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        expected_count = len(module.HEALTH_CAPABILITY_FIELDS)
        self.assertEqual(expected_count, 16)
        for path in module.HEALTH_CAPABILITY_COUNT_DOCS:
            with self.subTest(path=path):
                changed = dict(documents)
                current = f"all {expected_count}"
                self.assertIn(current, changed[path])
                changed[path] = changed[path].replace(current, "all nine", 1)
                errors = module.validate_session_launch_policy_docs(changed)
                self.assertIn(
                    f"{path}: health capability field count is stale", errors
                )

    def test_every_public_capability_list_is_guarded(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        for path in module.HEALTH_CAPABILITY_LIST_DOCS:
            with self.subTest(path=path):
                changed = dict(documents)
                changed[path] = changed[path].replace(
                    "`sessionLaunchPolicy`", "`removedCapability`"
                )
                errors = module.validate_session_launch_policy_docs(changed)
                self.assertIn(
                    f"{path}: capability list missing sessionLaunchPolicy", errors
                )

    def test_every_synchronized_reference_requires_request_adoption(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        for path in module.HEALTH_CAPABILITY_LIST_DOCS:
            with self.subTest(path=path):
                changed = dict(documents)
                self.assertIn("requestAdoptionContracts", changed[path])
                changed[path] = changed[path].replace(
                    "requestAdoptionContracts", "removedCapability"
                )
                errors = module.validate_session_launch_policy_docs(changed)
                self.assertIn(
                    f"{path}: capability list missing requestAdoptionContracts",
                    errors,
                )

    def test_requires_every_o3_literal_in_the_canonical_contract(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        path = "docs/API-CONTRACT.md"
        for literal in module.O3_REQUIRED_LITERALS:
            with self.subTest(literal=literal):
                changed = dict(documents)
                self.assertIn(literal, changed[path])
                changed[path] = changed[path].replace(literal, "removed-o3-literal")
                errors = module.validate_session_launch_policy_docs(changed)
                self.assertIn(
                    f"{path}: missing O3 contract literal {literal}",
                    errors,
                )

    def test_current_o3_markdown_structures_and_package_claims_are_guarded(self) -> None:
        documents = self.o3_structure_documents()
        self.assertEqual(module.validate_o3_document_structures(documents), [])

    def test_canonical_request_adoption_rejects_closed_shape_mutations(self) -> None:
        path = "docs/API-CONTRACT.md"
        for mutation in ("renamed", "missing", "extra", "duplicate"):
            with self.subTest(mutation=mutation):
                changed = self.o3_structure_documents()
                example = self.canonical_request_adoption_example(changed)
                payload = json.loads(example)
                if mutation == "renamed":
                    payload["digest"] = payload.pop("requestDigest")
                    self.replace_request_adoption_payload(changed, payload)
                elif mutation == "missing":
                    del payload["key"]
                    self.replace_request_adoption_payload(changed, payload)
                elif mutation == "extra":
                    payload["nonce"] = "decoy"
                    self.replace_request_adoption_payload(changed, payload)
                else:
                    original = (
                        '  "requestDigest": "sha256:'
                        + "a" * 64
                        + '"'
                    )
                    duplicate = (
                        '  "requestDigest": "sha256:'
                        + "b" * 64
                        + '"'
                    )
                    corrupt = self.replace_once(
                        example, original, f"{original},\n{duplicate}"
                    )
                    changed[path] = self.replace_once(changed[path], example, corrupt)
                changed[path] += (
                    "\n\n## Checker decoy\n"
                    '`{"contract":"psyche.request_adoption.v1",'
                    '"key":"psyche:decoy","requestDigest":"sha256:'
                    + "c" * 64
                    + '"}`\n'
                )

                errors = module.validate_o3_document_structures(changed)
                if mutation == "duplicate":
                    self.assertTrue(
                        any(
                            error.startswith(
                                f"{path}: canonical requestAdoption JSON is invalid"
                            )
                            and "duplicate key 'requestDigest'" in error
                            for error in errors
                        ),
                        errors,
                    )
                else:
                    self.assertIn(
                        f"{path}: canonical requestAdoption JSON must contain "
                        "exactly contract, key, and requestDigest",
                        errors,
                    )

    def test_canonical_request_adoption_rejects_bad_values(self) -> None:
        path = "docs/API-CONTRACT.md"
        cases = (
            (
                "contract",
                "psyche.request-adoption.v1",
                "canonical requestAdoption contract must equal",
            ),
            (
                "key",
                "psyche key with spaces",
                "canonical requestAdoption key must be representative valid ASCII",
            ),
            (
                "requestDigest",
                "sha256:" + "A" * 64,
                "canonical requestAdoption requestDigest must be sha256:",
            ),
        )
        for field, value, expected in cases:
            with self.subTest(field=field):
                changed = self.o3_structure_documents()
                payload = json.loads(
                    self.canonical_request_adoption_example(changed)
                )
                payload[field] = value
                self.replace_request_adoption_payload(changed, payload)
                changed[path] += (
                    "\n\n## Checker decoy\n"
                    "`psyche.request_adoption.v1` "
                    "`sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    "aaaaaaaaaaaaaaaa`\n"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(expected in error for error in errors),
                    errors,
                )

    def test_canonical_request_adoption_rule_cells_are_exact(self) -> None:
        path = "docs/API-CONTRACT.md"
        cases = (
            (
                "contract",
                "psyche.request_adoption.v1",
                "psyche.request_adoption.v2",
            ),
            ("key", "1 to 255", "0 to 255"),
            ("key", "1 to 255", "1 to 256"),
            ("key", "[A-Za-z0-9._:/-]", "[A-Za-z0-9._:/@-]"),
            ("key", "ASCII bytes", "Unicode characters"),
            ("requestDigest", "sha256:", "sha512:"),
            ("requestDigest", "64 lowercase", "63 lowercase"),
            ("requestDigest", "64 lowercase", "65 lowercase"),
            ("requestDigest", "lowercase", "uppercase"),
        )
        for field, old, new in cases:
            with self.subTest(field=field, mutation=new):
                changed = self.o3_structure_documents()
                self.mutate_request_adoption_rule(changed, field, old, new)
                changed[path] += (
                    "\n\n## Checker decoy\n"
                    "| Field | Exact rule |\n"
                    "|---|---|\n"
                    "| `contract` | Must equal "
                    "`psyche.request_adoption.v1` byte-for-byte. |\n"
                    "| `key` | 1 to 255 ASCII bytes; every byte must match "
                    "`[A-Za-z0-9._:/-]`. |\n"
                    "| `requestDigest` | Exactly `sha256:` followed by 64 "
                    "lowercase hexadecimal characters (71 ASCII bytes total). |\n"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        error.startswith(
                            f"{path}: requestAdoption {field} Exact rule cell "
                            "must equal"
                        )
                        for error in errors
                    ),
                    errors,
                )

    def test_canonical_request_adoption_rules_reject_extra_member(self) -> None:
        path = "docs/API-CONTRACT.md"
        changed = self.o3_structure_documents()
        table = self.canonical_request_adoption_rule_table(changed)
        extra_row = "| `nonce` | Any string. |"
        changed[path] = self.replace_once(
            changed[path],
            table.rows[-1].source,
            f"{table.rows[-1].source}\n{extra_row}",
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            f"{path}: requestAdoption rule table must contain exactly "
            "contract, key, and requestDigest",
            errors,
        )

    def test_canonical_request_adoption_closed_and_byte_preserving_claims_are_exact(
        self,
    ) -> None:
        path = "docs/API-CONTRACT.md"
        cases = (
            (
                "all three members are required",
                "four members are required",
                "closed-shape claim",
            ),
            (
                "any missing,\nunknown, or extra member",
                "an unknown or extra member is accepted; only a missing member",
                "closed-shape claim",
            ),
            (
                "performs no trimming, case folding",
                "performs trimming and case folding",
                "byte-preservation claim",
            ),
            (
                "no trimming, case folding",
                "no trimming but performs case folding",
                "byte-preservation claim",
            ),
            (
                "Unicode normalization",
                "Unicode normalization before comparison",
                "byte-preservation claim",
            ),
        )
        for old, new, label in cases:
            with self.subTest(mutation=new):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path], "Closed request shape and byte rules", level=3
                )
                self.assertIsNotNone(section)
                marker = (
                    "The object is closed:"
                    if label == "closed-shape claim"
                    else "Coven performs"
                )
                claim = module.markdown_paragraph(section or "", marker)
                self.assertIsNotNone(claim)
                changed[path] = self.replace_once(
                    changed[path],
                    claim or "",
                    self.replace_once(claim or "", old, new),
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        error.startswith(
                            f"{path}: canonical requestAdoption {label} "
                            "must equal"
                        )
                        for error in errors
                    ),
                    errors,
                )

    def test_canonical_request_adoption_byte_claim_rejects_contradictory_suffix(
        self,
    ) -> None:
        path = "docs/API-CONTRACT.md"
        changed = self.o3_structure_documents()
        section = module.markdown_section(
            changed[path], "Closed request shape and byte rules", level=3
        )
        claim = module.markdown_paragraph(section or "", "Coven performs")
        self.assertIsNotNone(claim)
        changed[path] = self.replace_once(
            changed[path],
            claim or "",
            (claim or "") + " Clients may trim and case-fold before sending.",
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertTrue(
            any(
                error.startswith(
                    f"{path}: canonical requestAdoption byte-preservation "
                    "claim must equal"
                )
                for error in errors
            ),
            errors,
        )

    def test_canonical_routes_require_both_metadata_members_in_the_right_cell(
        self,
    ) -> None:
        path = "docs/API-CONTRACT.md"
        for operation in ("launch", "input"):
            case = self.adopted_table_cases(operation)[0]
            route = str(case["display"])
            for field in ("executionBinding", "requestAdoption"):
                with self.subTest(route=route, field=field):
                    changed = self.o3_structure_documents()
                    section = module.markdown_section(
                        changed[path],
                        str(case["heading"]),
                        level=int(case["level"]),
                    )
                    table = module.markdown_table(
                        section or "", tuple(case["headers"])
                    )
                    rows = module.markdown_table_rows(
                        table, dict(case["criteria"])
                    )
                    self.assertEqual(len(rows), 1)
                    row = rows[0]
                    cells = list(row.cells)
                    index = table.headers.index("Required body metadata")
                    cells[index] = self.replace_once(
                        cells[index], f"`{field}`", "`removedMetadata`"
                    )
                    changed[path] = self.replace_once(
                        changed[path],
                        row.source,
                        "| " + " | ".join(cells) + " |",
                    )
                    changed[path] += (
                        f"\n\n## Checker decoy\n`{route}` requires `{field}`.\n"
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{path}: {route} Required body metadata cell must "
                        f"include {field}",
                        errors,
                    )

    def test_canonical_o3_error_statuses_are_exact(self) -> None:
        path = "docs/API-CONTRACT.md"
        self.assertEqual(
            module.O3_ERROR_STATUSES,
            self.EXPECTED_O3_ERROR_STATUSES,
        )
        for code, expected_status in self.EXPECTED_O3_ERROR_STATUSES.items():
            with self.subTest(code=code):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path], "O3 error matrix", level=3
                )
                table = module.markdown_table(
                    section or "",
                    (
                        "Code",
                        "Status",
                        "Phase and condition",
                        "Exact message and details",
                    ),
                )
                rows = module.markdown_table_rows(table, {"Code": code})
                self.assertEqual(len(rows), 1)
                row = rows[0]
                cells = list(row.cells)
                index = table.headers.index("Status")
                wrong_status = "400" if expected_status == "409" else "409"
                cells[index] = wrong_status
                changed[path] = self.replace_once(
                    changed[path],
                    row.source,
                    "| " + " | ".join(cells) + " |",
                )
                changed[path] += (
                    f"\n\n## Checker decoy\n`{code}` uses {expected_status}.\n"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 error {code} status must equal {expected_status}",
                    errors,
                )

    def test_canonical_o3_error_rows_cannot_be_deleted_or_duplicated(self) -> None:
        path = "docs/API-CONTRACT.md"
        for code in self.EXPECTED_O3_ERROR_STATUSES:
            for mutation in ("deleted", "duplicated"):
                with self.subTest(code=code, mutation=mutation):
                    changed = self.o3_structure_documents()
                    section = module.markdown_section(
                        changed[path], "O3 error matrix", level=3
                    )
                    table = module.markdown_table(
                        section or "",
                        (
                            "Code",
                            "Status",
                            "Phase and condition",
                            "Exact message and details",
                        ),
                    )
                    rows = module.markdown_table_rows(table, {"Code": code})
                    self.assertEqual(len(rows), 1)
                    row = rows[0]
                    replacement = "" if mutation == "deleted" else f"{row.source}\n{row.source}"
                    changed[path] = self.replace_once(
                        changed[path], row.source, replacement
                    )
                    changed[path] += (
                        f"\n\n## Checker decoy\n`{code}` remains documented.\n"
                    )

                    errors = module.validate_o3_document_structures(changed)
                    found = 0 if mutation == "deleted" else 2
                    self.assertIn(
                        f"{path}: O3 error matrix must contain one {code} row "
                        f"(found {found})",
                        errors,
                    )

    def test_canonical_o3_error_matrix_rejects_unknown_rows(self) -> None:
        path = "docs/API-CONTRACT.md"
        changed = self.o3_structure_documents()
        section = module.markdown_section(changed[path], "O3 error matrix", level=3)
        table = module.markdown_table(
            section or "",
            (
                "Code",
                "Status",
                "Phase and condition",
                "Exact message and details",
            ),
        )
        self.assertIsNotNone(table)
        extra = (
            "| `request_adoption_decoy` | 418 | Decoy. | "
            "`Decoy.`; details are omitted. |"
        )
        changed[path] = self.replace_once(
            changed[path],
            table.rows[-1].source,
            f"{table.rows[-1].source}\n{extra}",
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            f"{path}: O3 error matrix must contain exactly the expected "
            "adopted-operation codes",
            errors,
        )

    def test_canonical_o3_static_field_paths_cannot_move(self) -> None:
        path = "docs/API-CONTRACT.md"
        for marker, expected_path in module.O3_STATIC_FIELD_PATHS:
            with self.subTest(marker=marker):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path], "Metadata isolation and privacy", level=3
                )
                table = module.markdown_table(
                    section or "", ("Condition", "error.details.fields")
                )
                rows = module.markdown_table_rows_containing(
                    table, {"Condition": marker}
                )
                self.assertEqual(len(rows), 1)
                row = rows[0]
                cells = list(row.cells)
                index = table.headers.index("`error.details.fields`")
                wrong_path = (
                    "requestAdoption.key"
                    if expected_path == "requestAdoption"
                    else "requestAdoption"
                )
                cells[index] = (
                    f"`{json.dumps([wrong_path], separators=(',', ':'))}`"
                )
                changed[path] = self.replace_once(
                    changed[path],
                    row.source,
                    "| " + " | ".join(cells) + " |",
                )
                changed[path] += (
                    f"\n\n## Checker decoy\n{marker}: "
                    f"`{json.dumps([expected_path])}`.\n"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 static field path for {marker!r} must equal "
                    f"{json.dumps([expected_path], separators=(',', ':'))}",
                    errors,
                )

    def test_every_o3_negotiation_surface_guards_gate_and_literal(self) -> None:
        for path, surface in module.O3_NEGOTIATION_SURFACES.items():
            with self.subTest(path=path, mutation="field"):
                changed = self.o3_structure_documents()
                self.mutate_o3_negotiation_claim(
                    changed,
                    path,
                    "requestAdoptionContracts",
                    "executionBindingContracts",
                )
                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 negotiation claim must gate on "
                    "requestAdoptionContracts",
                    errors,
                )

            with self.subTest(path=path, mutation="literal"):
                changed = self.o3_structure_documents()
                replacement = (
                    "psyche.request_adoption.v2"
                    if surface.literal_claim == module.REQUEST_ADOPTION_CONTRACT
                    else "an O3 value"
                )
                self.mutate_o3_negotiation_claim(
                    changed, path, surface.literal_claim, replacement
                )
                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 negotiation claim must require the exact "
                    f"{surface.literal_claim}",
                    errors,
                )

        execution_gate_mutations = {
            "docs/API-CONTRACT.md": (
                "does not independently\ngate",
                "does independently\ngate",
            ),
            "docs/reference/api-contract.md": (
                "does not\nindependently gate",
                "does\nindependently gate",
            ),
            "docs/daemon/socket-api.md": (
                "does not\nindependently gate",
                "does\nindependently gate",
            ),
        }
        for path, (old, new) in execution_gate_mutations.items():
            with self.subTest(path=path, mutation="executionBindingContracts"):
                changed = self.o3_structure_documents()
                self.mutate_o3_negotiation_claim(changed, path, old, new)
                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 negotiation claim must not gate adopted "
                    "methods on executionBindingContracts",
                    errors,
                )

    def test_every_o3_negotiation_surface_rejects_negated_gate_claim(self) -> None:
        mutations = {
            "docs/API-CONTRACT.md": ("verifies that", "does not verify that"),
            "docs/reference/api.md": (
                "checks the exact",
                "does not check the exact",
            ),
            "docs/reference/api-contract.md": (
                "require the exact O3 literal",
                "require clients to proceed but do not verify the exact O3 literal",
            ),
            "docs/daemon/socket-api.md": (
                "verifies only that `requestAdoptionContracts`",
                "verifies only that it does not verify `requestAdoptionContracts`",
            ),
            "packages/openclaw-coven/README.md": (
                "requires\n`requestAdoptionContracts`",
                "does not require\n`requestAdoptionContracts`",
            ),
        }
        self.assertEqual(set(mutations), set(module.O3_NEGOTIATION_SURFACES))
        for path, (old, new) in mutations.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                self.mutate_o3_negotiation_claim(changed, path, old, new)

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 negotiation claim must gate on "
                    "requestAdoptionContracts",
                    errors,
                )

    def test_owned_o3_negotiation_surfaces_guard_per_request_proof(self) -> None:
        mutations = {
            "docs/API-CONTRACT.md": ("must still carry", "must not carry"),
            "docs/reference/api.md": (
                "the capability does not replace the complete exact O2\n"
                "proof in each request",
                "each request must not carry the complete exact O2\nproof",
            ),
            "docs/reference/api-contract.md": (
                "including the mandatory\nexact O2 proof in every request",
                "but every request must not carry the\nexact O2 proof",
            ),
            "docs/daemon/socket-api.md": ("still must\ncarry", "must not\ncarry"),
        }
        self.assertEqual(
            set(mutations),
            {
                path
                for path, surface in module.O3_NEGOTIATION_SURFACES.items()
                if surface.owns_proof_boundary
            },
        )
        for path, (old, new) in mutations.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                self.mutate_o3_negotiation_claim(changed, path, old, new)
                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 negotiation claim must retain exact "
                    "per-request executionBinding proof",
                    errors,
                )

    def test_owned_o3_negotiation_surfaces_guard_no_fallback(self) -> None:
        mutations = {
            "docs/API-CONTRACT.md": ("no legacy fallback", "a legacy fallback"),
            "docs/reference/api.md": ("never fall back", "may fall back"),
            "docs/reference/api-contract.md": ("never retry", "may retry"),
            "docs/daemon/socket-api.md": ("never falls back", "may fall back"),
            "packages/openclaw-coven/README.md": (
                "never falls back",
                "may fall back",
            ),
        }
        self.assertEqual(
            set(mutations),
            {
                path
                for path, surface in module.O3_NEGOTIATION_SURFACES.items()
                if surface.owns_no_fallback
            },
        )
        for path, (old, new) in mutations.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                self.mutate_o3_negotiation_claim(changed, path, old, new)
                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 negotiation claim must prohibit legacy "
                    "mutation fallback",
                    errors,
                )

    def test_o3_negotiation_claims_reject_contradictions_after_canonical_text(
        self,
    ) -> None:
        contradictions = (
            (
                set(module.O3_NEGOTIATION_SURFACES),
                " The client does not verify `requestAdoptionContracts` before POST.",
                "must gate on requestAdoptionContracts",
            ),
            (
                {
                    path
                    for path, surface in module.O3_NEGOTIATION_SURFACES.items()
                    if surface.owns_proof_boundary
                },
                " Every adopted request must not carry the complete exact O2 "
                "`executionBinding` proof.",
                "must retain exact per-request executionBinding proof",
            ),
            (
                {
                    path
                    for path, surface in module.O3_NEGOTIATION_SURFACES.items()
                    if surface.owns_no_fallback
                },
                " The client may fall back to a legacy mutation.",
                "must prohibit legacy mutation fallback",
            ),
        )
        for paths, contradiction, diagnostic in contradictions:
            for path in paths:
                with self.subTest(path=path, diagnostic=diagnostic):
                    changed = self.o3_structure_documents()
                    surface = module.O3_NEGOTIATION_SURFACES[path]
                    section = module.markdown_section(
                        changed[path], surface.heading, level=surface.level
                    )
                    marker = surface.markers[0]
                    paragraph = module.markdown_paragraph(section or "", marker)
                    self.assertIsNotNone(paragraph)
                    changed[path] = self.replace_once(
                        changed[path],
                        paragraph or "",
                        (paragraph or "") + contradiction,
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertTrue(
                        any(
                            error == f"{path}: O3 negotiation claim {diagnostic}"
                            for error in errors
                        ),
                        errors,
                    )

    def test_canonical_health_requires_request_adoption_in_its_json(self) -> None:
        changed = self.o3_structure_documents()
        self.mutate_health_capability(
            changed,
            "docs/API-CONTRACT.md",
            "`GET /api/v1/health`",
            "requestAdoptionContracts",
            self.MISSING,
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            "docs/API-CONTRACT.md: canonical health example missing "
            "requestAdoptionContracts",
            errors,
        )

    def test_canonical_health_rejects_invalid_json_with_a_clear_error(self) -> None:
        path = "docs/API-CONTRACT.md"
        changed = self.o3_structure_documents()
        section = module.markdown_section(changed[path], "`GET /api/v1/health`")
        self.assertIsNotNone(section)
        example = module.fenced_code_block(section or "", "json")
        self.assertIsNotNone(example)
        invalid = self.replace_once(example or "", '"ok": true', '"ok": tru')
        changed[path] = self.replace_once(changed[path], example or "", invalid)
        changed[path] += (
            '\n\nDecoy valid JSON: `{"requestAdoptionContracts":'
            '["psyche.request_adoption.v1"]}`.\n'
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertTrue(
            any(
                error.startswith(
                    "docs/API-CONTRACT.md: canonical health example JSON is invalid"
                )
                for error in errors
            ),
            errors,
        )

    def test_canonical_health_rejects_non_finite_json_constants(self) -> None:
        path = "docs/API-CONTRACT.md"
        for constant in ("NaN", "Infinity", "-Infinity"):
            with self.subTest(constant=constant):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path], "`GET /api/v1/health`"
                )
                self.assertIsNotNone(section)
                example = module.fenced_code_block(section or "", "json")
                self.assertIsNotNone(example)
                corrupt = self.replace_once(
                    example or "", '"ok": true', f'"ok": {constant}'
                )
                changed[path] = self.replace_once(
                    changed[path], example or "", corrupt
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: canonical health example JSON is invalid "
                    "(line 1, column 1: non-finite JSON constant "
                    f"{constant!r})",
                    errors,
                )

    def test_canonical_health_rejects_empty_request_adoption_contracts(self) -> None:
        changed = self.o3_structure_documents()
        self.mutate_health_capability(
            changed,
            "docs/API-CONTRACT.md",
            "`GET /api/v1/health`",
            "requestAdoptionContracts",
            [],
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            "docs/API-CONTRACT.md: canonical health example "
            "requestAdoptionContracts must equal "
            '["psyche.request_adoption.v1"]',
            errors,
        )

    def test_canonical_health_rejects_wrong_request_adoption_literal(self) -> None:
        changed = self.o3_structure_documents()
        self.mutate_health_capability(
            changed,
            "docs/API-CONTRACT.md",
            "`GET /api/v1/health`",
            "requestAdoptionContracts",
            ["psyche.request-adoption.v1"],
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            "docs/API-CONTRACT.md: canonical health example "
            "requestAdoptionContracts must equal "
            '["psyche.request_adoption.v1"]',
            errors,
        )

    def test_canonical_health_rejects_scalar_request_adoption_contracts(self) -> None:
        changed = self.o3_structure_documents()
        self.mutate_health_capability(
            changed,
            "docs/API-CONTRACT.md",
            "`GET /api/v1/health`",
            "requestAdoptionContracts",
            "psyche.request_adoption.v1",
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            "docs/API-CONTRACT.md: canonical health example "
            "requestAdoptionContracts must equal "
            '["psyche.request_adoption.v1"]',
            errors,
        )

    def test_health_examples_reject_numeric_boolean_capabilities(self) -> None:
        for path, heading, request_line, label in self.health_example_cases():
            for field, value, expected in (
                ("sessions", 1, "true"),
                ("afsMount", 0, "false"),
            ):
                with self.subTest(path=path, field=field):
                    changed = self.o3_structure_documents()
                    self.mutate_health_capability(
                        changed,
                        path,
                        heading,
                        field,
                        value,
                        request_line=request_line,
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{path}: {label} {field} must equal {expected}",
                        errors,
                    )

    def test_duplicate_health_examples_fail_closed(self) -> None:
        for path, heading, request_line, label in self.health_example_cases():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                section = module.markdown_section(changed[path], heading)
                self.assertIsNotNone(section)
                if request_line is None:
                    example = module.fenced_code_block(section or "", "json")
                else:
                    example = module.http_json_example(
                        section or "", request_line
                    )
                self.assertIsNotNone(example)
                payload = json.loads(example or "")
                payload["capabilities"]["requestAdoptionContracts"] = [
                    "decoy.request_adoption.v1"
                ]
                corrupt = json.dumps(payload, indent=2)
                if request_line is None:
                    original = f"```json\n{example}\n```"
                    duplicate = f"```json\n{corrupt}\n```"
                else:
                    original = (
                        f"```http\n{request_line}\n```\n\n"
                        f"```json\n{example}\n```"
                    )
                    duplicate = (
                        f"```http\n{request_line}\n```\n\n"
                        f"```json\n{corrupt}\n```"
                    )
                changed[path] = self.replace_once(
                    changed[path], original, f"{original}\n\n{duplicate}"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: {label} JSON fence is missing or ambiguous",
                    errors,
                )

    def test_health_examples_reject_duplicate_capability_key_with_conflicting_values(
        self,
    ) -> None:
        """A duplicated `requestAdoptionContracts` key with a conflicting array
        must fail closed instead of silently keeping json.loads' last value,
        in both the canonical health block and every synchronized example."""
        original = '    "requestAdoptionContracts": ["psyche.request_adoption.v1"]'
        duplicate = '    "requestAdoptionContracts": ["decoy.request_adoption.v1"]'
        for path, heading, request_line, label in self.health_example_cases():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                changed[path] = self.replace_once(
                    changed[path], original, f"{original},\n{duplicate}"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: {label} JSON is invalid "
                    "(line 1, column 1: duplicate key 'requestAdoptionContracts')",
                    errors,
                )

    def test_health_examples_reject_harmless_duplicate_capability_key(self) -> None:
        """Even a duplicate that repeats an already-correct, unrelated field
        with the *same* value both times must fail closed: the capabilities
        object is a closed shape, but a collapsing duplicate leaves the final
        field count unchanged, so only strict duplicate-key rejection (not
        the exact-field-count check) can catch it."""
        original = '    "hub": true,'
        for path, heading, request_line, label in self.health_example_cases():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                changed[path] = self.replace_once(
                    changed[path], original, f"{original}\n{original}"
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: {label} JSON is invalid "
                    "(line 1, column 1: duplicate key 'hub')",
                    errors,
                )

    def test_adopted_input_table_cells_reject_duplicate_adopted_and_replayed_keys(
        self,
    ) -> None:
        """Table cells embed the same JSON shape as prose examples and must
        use the same strict loader: a duplicated `adopted` or `replayed` key
        with a conflicting value must fail closed with a duplicate-key
        diagnostic instead of silently matching on the last value."""
        mutations = (
            (
                module.ADOPTED_INPUT_FIRST_RESULT,
                "first-adoption",
                "adopted",
                '{"adopted":true,"adopted":false,"replayed":false,'
                '"delivery":"not_asserted"}',
            ),
            (
                module.ADOPTED_INPUT_REPLAY_RESULT,
                "exact-replay",
                "replayed",
                '{"adopted":true,"replayed":true,"replayed":false,'
                '"delivery":"not_asserted"}',
            ),
        )
        for case in self.adopted_table_cases("input")[1:]:
            for expected, label, field, replacement in mutations:
                with self.subTest(path=case["path"], field=field):
                    changed = self.o3_structure_documents()
                    self.replace_adopted_result_json(
                        changed, case, expected, replacement
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{case['path']}: {case['display']} {label} "
                        f"result JSON has duplicate key {field!r}",
                        errors,
                    )

    def test_adopted_input_result_cells_reject_non_finite_json_constants(
        self,
    ) -> None:
        for case in self.adopted_table_cases("input")[1:]:
            for constant in ("NaN", "Infinity", "-Infinity"):
                with self.subTest(path=case["path"], constant=constant):
                    changed = self.o3_structure_documents()
                    replacement = (
                        '{"adopted":true,"replayed":false,"delivery":'
                        f"{constant}"
                        "}"
                    )
                    self.replace_adopted_result_json(
                        changed,
                        case,
                        module.ADOPTED_INPUT_FIRST_RESULT,
                        replacement,
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{case['path']}: {case['display']} first-adoption "
                        "result JSON is invalid "
                        "(line 1, column 1: non-finite JSON constant "
                        f"{constant!r})",
                        errors,
                    )

    def test_canonical_adopted_input_rejects_duplicate_delivery_key(self) -> None:
        """The canonical adopted-input JSON blocks (not table cells) go
        through the same strict loader, so a duplicated `delivery` key with a
        conflicting value must fail closed there too."""
        path = "docs/API-CONTRACT.md"
        cases = (
            ("The first successful adopted-input response is exactly:", "first-adoption"),
            ("An exact adopted-input replay is exactly:", "exact-replay"),
        )
        for marker, label in cases:
            with self.subTest(label=label):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path],
                    "Adopted routes, compatibility, and responses",
                    level=3,
                )
                self.assertIsNotNone(section)
                example = module.json_example_after_marker(section or "", marker)
                self.assertIsNotNone(example)
                original = '  "delivery": "not_asserted"'
                self.assertIn(original, example or "")
                corrupt = (example or "").replace(
                    original, f'{original},\n  "delivery": "confirmed"', 1
                )
                changed[path] = self.replace_once(
                    changed[path], example or "", corrupt
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: canonical adopted-input {label} JSON is invalid "
                    "(line 1, column 1: duplicate key 'delivery')",
                    errors,
                )

    def test_synchronized_health_examples_reject_empty_and_wrong_o3_values(
        self,
    ) -> None:
        examples = {
            "docs/reference/api-contract.md": "Negotiation",
            "docs/daemon/socket-api.md": "Handshake",
        }
        for path, heading in examples.items():
            for value in ([], ["psyche.request_adoption.V1"]):
                with self.subTest(path=path, value=value):
                    changed = self.o3_structure_documents()
                    self.mutate_health_capability(
                        changed,
                        path,
                        heading,
                        "requestAdoptionContracts",
                        value,
                        request_line="GET /api/v1/health",
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{path}: health example requestAdoptionContracts "
                        'must equal ["psyche.request_adoption.v1"]',
                        errors,
                    )

    def test_all_health_examples_reject_wrong_execution_binding_semantics(
        self,
    ) -> None:
        examples = (
            ("docs/API-CONTRACT.md", "`GET /api/v1/health`", None),
            ("docs/reference/api-contract.md", "Negotiation", "GET /api/v1/health"),
            ("docs/daemon/socket-api.md", "Handshake", "GET /api/v1/health"),
        )
        for path, heading, request_line in examples:
            for value in ([], ["psyche.execution-binding.v1"], "psyche.execution_binding.v1"):
                with self.subTest(path=path, value=value):
                    changed = self.o3_structure_documents()
                    self.mutate_health_capability(
                        changed,
                        path,
                        heading,
                        "executionBindingContracts",
                        value,
                        request_line=request_line,
                    )

                    errors = module.validate_o3_document_structures(changed)
                    label = (
                        "canonical health example"
                        if path == "docs/API-CONTRACT.md"
                        else "health example"
                    )
                    self.assertIn(
                        f"{path}: {label} executionBindingContracts "
                        'must equal ["psyche.execution_binding.v1"]',
                        errors,
                    )

    def test_canonical_capability_table_rejects_wrong_type_and_value_cells(
        self,
    ) -> None:
        path = "docs/API-CONTRACT.md"
        for header, old, new in (
            ("Type", "string array", "string"),
            (
                "Description",
                '["psyche.request_adoption.v1"]',
                "[]",
            ),
        ):
            with self.subTest(header=header):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path], "Capability fields", level=3
                )
                self.assertIsNotNone(section)
                table = module.markdown_table(
                    section or "", ("Field", "Type", "Description")
                )
                self.assertIsNotNone(table)
                rows = module.markdown_table_rows(
                    table, {"Field": "requestAdoptionContracts"}
                )
                self.assertEqual(len(rows), 1)
                row = rows[0]
                cells = list(row.cells)
                index = table.headers.index(header)
                cells[index] = self.replace_once(cells[index], old, new)
                replacement = "| " + " | ".join(cells) + " |"
                changed[path] = self.replace_once(
                    changed[path], row.source, replacement
                )
                changed[path] += (
                    "\n\nDecoy table claim: `requestAdoptionContracts`, "
                    '`string array`, `["psyche.request_adoption.v1"]`.\n'
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        error.startswith(
                            f"{path}: capability table "
                            "requestAdoptionContracts"
                        )
                        for error in errors
                    ),
                    errors,
                )

    def test_strict_json_equality_rejects_boolean_number_aliases_recursively(
        self,
    ) -> None:
        self.assertFalse(module.strict_json_equal(True, 1))
        self.assertFalse(module.strict_json_equal(0, False))
        self.assertFalse(
            module.strict_json_equal(
                {"outer": [{"enabled": 1}]},
                {"outer": [{"enabled": True}]},
            )
        )
        self.assertFalse(
            module.strict_json_equal(
                {"outer": [{"count": False}]},
                {"outer": [{"count": 0}]},
            )
        )

    def test_canonical_adopted_input_rejects_numeric_booleans(self) -> None:
        path = "docs/API-CONTRACT.md"
        cases = (
            (
                "The first successful adopted-input response is exactly:",
                "first-adoption",
                module.ADOPTED_INPUT_FIRST_RESULT,
            ),
            (
                "An exact adopted-input replay is exactly:",
                "exact-replay",
                module.ADOPTED_INPUT_REPLAY_RESULT,
            ),
        )
        for marker, label, expected in cases:
            for field in ("adopted", "replayed"):
                with self.subTest(label=label, field=field):
                    changed = self.o3_structure_documents()
                    section = module.markdown_section(
                        changed[path],
                        "Adopted routes, compatibility, and responses",
                        level=3,
                    )
                    self.assertIsNotNone(section)
                    example = module.json_example_after_marker(
                        section or "", marker
                    )
                    self.assertIsNotNone(example)
                    payload = json.loads(example or "")
                    payload[field] = int(expected[field])
                    changed[path] = self.replace_once(
                        changed[path],
                        example or "",
                        json.dumps(payload, indent=2),
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{path}: canonical adopted-input {label} JSON must equal "
                        f"{json.dumps(expected, separators=(',', ':'))}",
                        errors,
                    )

    def test_adopted_input_table_json_tolerates_whitespace_and_key_order(
        self,
    ) -> None:
        first = '{ "delivery" : "not_asserted", "replayed" : false, "adopted" : true }'
        replay = '{ "replayed" : true, "delivery" : "not_asserted", "adopted" : true }'
        for case in self.adopted_table_cases("input")[1:]:
            with self.subTest(path=case["path"]):
                changed = self.o3_structure_documents()
                self.replace_adopted_result_json(
                    changed, case, module.ADOPTED_INPUT_FIRST_RESULT, first
                )
                self.replace_adopted_result_json(
                    changed, case, module.ADOPTED_INPUT_REPLAY_RESULT, replay
                )
                path = str(case["path"])
                section = module.markdown_section(
                    changed[path],
                    str(case["heading"]),
                    level=int(case["level"]),
                )
                table = module.markdown_table(
                    section or "", tuple(case["headers"])
                )
                rows = module.markdown_table_rows(
                    table, dict(case["criteria"])
                )
                self.assertEqual(len(rows), 1)
                row = rows[0]
                formatted_row = (
                    row.source.replace("`202 ", "`  202   ", 1)
                    .replace("`200 ", "` 200  ", 1)
                )
                changed[path] = self.replace_once(
                    changed[path], row.source, formatted_row
                )

                self.assertEqual(
                    module.validate_o3_document_structures(changed), []
                )

    def test_adopted_input_table_json_rejects_wrong_types_values_and_shapes(
        self,
    ) -> None:
        invalid = (
            {
                "adopted": 1,
                "replayed": False,
                "delivery": "not_asserted",
            },
            {
                "adopted": True,
                "replayed": 0,
                "delivery": "not_asserted",
            },
            {
                "adopted": False,
                "replayed": False,
                "delivery": "not_asserted",
            },
            {
                "adopted": True,
                "replayed": False,
                "delivery": "not_asserted",
                "decoy": "requestAdoptionContracts",
            },
            {
                "adopted": True,
                "delivery": "not_asserted",
            },
        )
        for case in self.adopted_table_cases("input")[1:]:
            for payload in invalid:
                with self.subTest(path=case["path"], payload=payload):
                    changed = self.o3_structure_documents()
                    self.replace_adopted_result_json(
                        changed,
                        case,
                        module.ADOPTED_INPUT_FIRST_RESULT,
                        json.dumps(payload, separators=(",", ":")),
                    )

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{case['path']}: {case['display']} "
                        "first-adoption status/result is incorrect",
                        errors,
                    )

    def test_adopted_input_table_status_is_an_exact_integer_token(self) -> None:
        case = self.adopted_table_cases("input")[-1]
        path = str(case["path"])
        for invalid in ("202.0", "true", "1202"):
            with self.subTest(status=invalid):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path],
                    str(case["heading"]),
                    level=int(case["level"]),
                )
                table = module.markdown_table(
                    section or "", tuple(case["headers"])
                )
                rows = module.markdown_table_rows(
                    table, dict(case["criteria"])
                )
                self.assertEqual(len(rows), 1)
                row = rows[0]
                changed[path] = self.replace_once(
                    changed[path],
                    row.source,
                    row.source.replace("`202 ", f"`{invalid} ", 1),
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: {case['display']} "
                    "first-adoption status/result is incorrect",
                    errors,
                )

    def test_launch_first_and_replay_status_swaps_fail_in_every_table(self) -> None:
        for case in self.adopted_table_cases("launch"):
            with self.subTest(path=case["path"]):
                changed = self.o3_structure_documents()
                self.mutate_adopted_statuses(changed, case, "launch")

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        str(case["path"]) in error
                        and str(case["display"]) in error
                        and "first-adoption status/result is incorrect" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_launch_results_require_exact_singular_session_record_expression(
        self,
    ) -> None:
        invalid_results = (
            "SessionRecord[]",
            "Optional<SessionRecord>",
            "not SessionRecord",
            "SessionRecord payload",
        )
        for case in self.adopted_table_cases("launch"):
            for phase in ("first", "replay"):
                for replacement in invalid_results:
                    with self.subTest(
                        path=case["path"],
                        phase=phase,
                        replacement=replacement,
                    ):
                        changed = self.o3_structure_documents()
                        self.replace_launch_result_type(
                            changed, case, phase, replacement
                        )

                        errors = module.validate_o3_document_structures(changed)
                        label = (
                            "first-adoption"
                            if phase == "first"
                            else "exact-replay"
                        )
                        self.assertIn(
                            f"{case['path']}: {case['display']} {label} "
                            "status/result is incorrect",
                            errors,
                        )

    def test_combined_launch_result_cells_reject_prose_suffix(self) -> None:
        for case in (
            candidate
            for candidate in self.adopted_table_cases("launch")
            if "combined" in candidate
        ):
            with self.subTest(path=case["path"]):
                changed = self.o3_structure_documents()
                path = str(case["path"])
                section = module.markdown_section(
                    changed[path],
                    str(case["heading"]),
                    level=int(case["level"]),
                )
                table = module.markdown_table(
                    section or "", tuple(case["headers"])
                )
                rows = module.markdown_table_rows(table, dict(case["criteria"]))
                self.assertEqual(len(rows), 1)
                row = rows[0]
                cells = list(row.cells)
                index = table.headers.index(str(case["combined"]))
                cells[index] += " The result may also be optional."
                changed[path] = self.replace_once(
                    changed[path],
                    row.source,
                    "| " + " | ".join(cells) + " |",
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        error.startswith(f"{path}: {case['display']} ")
                        and "status/result is incorrect" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_combined_launch_result_cells_reject_negated_prose_prefix(self) -> None:
        for case in (
            candidate
            for candidate in self.adopted_table_cases("launch")
            if "combined" in candidate
        ):
            with self.subTest(path=case["path"]):
                changed = self.o3_structure_documents()
                path = str(case["path"])
                section = module.markdown_section(
                    changed[path],
                    str(case["heading"]),
                    level=int(case["level"]),
                )
                table = module.markdown_table(
                    section or "", tuple(case["headers"])
                )
                rows = module.markdown_table_rows(table, dict(case["criteria"]))
                self.assertEqual(len(rows), 1)
                row = rows[0]
                cells = list(row.cells)
                index = table.headers.index(str(case["combined"]))
                cells[index] = "Not a SessionRecord: " + cells[index]
                changed[path] = self.replace_once(
                    changed[path],
                    row.source,
                    "| " + " | ".join(cells) + " |",
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        error.startswith(f"{path}: {case['display']} ")
                        and "status/result is incorrect" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_input_first_and_replay_status_swaps_fail_in_every_table(self) -> None:
        for case in self.adopted_table_cases("input"):
            with self.subTest(path=case["path"]):
                changed = self.o3_structure_documents()
                self.mutate_adopted_statuses(changed, case, "input")

                errors = module.validate_o3_document_structures(changed)
                self.assertTrue(
                    any(
                        str(case["path"]) in error
                        and str(case["display"]) in error
                        and "first-adoption status/result is incorrect" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_method_path_and_status_shifted_into_wrong_columns_fails(self) -> None:
        path = "packages/openclaw-coven/README.md"
        changed = self.o3_structure_documents()
        case = self.adopted_table_cases("launch")[-1]
        section = module.markdown_section(
            changed[path], str(case["heading"]), level=int(case["level"])
        )
        table = module.markdown_table(section or "", tuple(case["headers"]))
        self.assertIsNotNone(table)
        rows = module.markdown_table_rows(table, dict(case["criteria"]))
        self.assertEqual(len(rows), 1)
        row = rows[0]
        cells = list(row.cells)
        route_index = table.headers.index("Dedicated route")
        status_index = table.headers.index("First adoption")
        cells[route_index], cells[status_index] = (
            cells[status_index],
            cells[route_index],
        )
        replacement = "| " + " | ".join(cells) + " |"
        changed[path] = self.replace_once(changed[path], row.source, replacement)
        changed[path] += (
            "\n\nDecoy: `launchAdoptedSession`, "
            "`POST /api/v1/adopted-sessions`, `201 SessionRecord`.\n"
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            f"{path}: launchAdoptedSession Dedicated route cell must equal "
            "POST /api/v1/adopted-sessions",
            errors,
        )

    def test_duplicate_target_heading_and_row_fail_closed(self) -> None:
        path = "packages/openclaw-coven/README.md"

        duplicate_heading = self.o3_structure_documents()
        section = module.markdown_section(
            duplicate_heading[path], "Adopted client methods", level=3
        )
        self.assertIsNotNone(section)
        duplicate_heading[path] += (
            "\n\n### Adopted client methods\n\n" + (section or "") + "\n"
        )
        errors = module.validate_o3_document_structures(duplicate_heading)
        self.assertIn(
            f"{path}: expected one Adopted client methods section (found 2)",
            errors,
        )

        duplicate_row = self.o3_structure_documents()
        section = module.markdown_section(
            duplicate_row[path], "Adopted client methods", level=3
        )
        table = module.markdown_table(
            section or "",
            ("Method", "Dedicated route", "First adoption", "Exact replay"),
        )
        rows = module.markdown_table_rows(
            table, {"Method": "launchAdoptedSession"}
        )
        self.assertEqual(len(rows), 1)
        row = rows[0]
        corrupt_cells = list(row.cells)
        corrupt_cells[2] = corrupt_cells[2].replace(
            "201 SessionRecord", "299 decoy SessionRecord"
        )
        corrupt_row = "| " + " | ".join(corrupt_cells) + " |"
        duplicate_row[path] = self.replace_once(
            duplicate_row[path], row.source, f"{row.source}\n{corrupt_row}"
        )
        errors = module.validate_o3_document_structures(duplicate_row)
        self.assertIn(
            f"{path}: adopted method table has ambiguous launchAdoptedSession row",
            errors,
        )

    def test_duplicate_capability_table_row_fails_closed(self) -> None:
        path = "docs/API-CONTRACT.md"
        changed = self.o3_structure_documents()
        section = module.markdown_section(
            changed[path], "Capability fields", level=3
        )
        table = module.markdown_table(
            section or "", ("Field", "Type", "Description")
        )
        rows = module.markdown_table_rows(
            table, {"Field": "requestAdoptionContracts"}
        )
        self.assertEqual(len(rows), 1)
        row = rows[0]
        corrupt_cells = list(row.cells)
        corrupt_cells[1] = "boolean"
        corrupt_cells[2] = (
            "Contradictory decoy claim: "
            '`["psyche.request_adoption.v0"]` plus `requestAdoptionContracts`.'
        )
        corrupt_row = "| " + " | ".join(corrupt_cells) + " |"
        changed[path] = self.replace_once(
            changed[path], row.source, f"{row.source}\n{corrupt_row}"
        )

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            f"{path}: capability table has ambiguous "
            "requestAdoptionContracts row",
            errors,
        )

    def test_duplicate_o3_capability_paragraphs_fail_closed(self) -> None:
        for path, surface in module.O3_NEGOTIATION_SURFACES.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                section = module.markdown_section(
                    changed[path], surface.heading, level=surface.level
                )
                self.assertIsNotNone(section)
                marker = surface.markers[0]
                paragraph = module.markdown_paragraph(section or "", marker)
                self.assertIsNotNone(paragraph)
                corrupt = (paragraph or "").replace(
                    "requestAdoptionContracts", "removedCapability"
                )
                corrupt += (
                    "\nDecoy tokens: `requestAdoptionContracts`, "
                    "`psyche.request_adoption.v1`, fail closed, never falls back."
                )
                changed[path] = self.replace_once(
                    changed[path],
                    paragraph or "",
                    f"{paragraph}\n\n{corrupt}",
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: expected one O3 negotiation claim paragraph "
                    f"for {marker!r} (found 2)",
                    errors,
                )

    def test_duplicate_health_capability_list_paragraphs_fail_closed(self) -> None:
        for path, (heading, marker) in module.HEALTH_CAPABILITY_LISTS.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                section = module.markdown_section(changed[path], heading)
                self.assertIsNotNone(section)
                paragraph = module.markdown_paragraph(section or "", marker)
                self.assertIsNotNone(paragraph)
                corrupt = (paragraph or "").replace(
                    "`requestAdoptionContracts`", "`removedCapability`"
                )
                corrupt += (
                    "\nDecoy tokens: all 16 fields and "
                    "`requestAdoptionContracts`."
                )
                changed[path] = self.replace_once(
                    changed[path],
                    paragraph or "",
                    f"{paragraph}\n\n{corrupt}",
                )

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: health capability list is ambiguous (found 2)",
                    errors,
                )

    def test_duplicate_openclaw_health_claims_fail_closed(self) -> None:
        path = "packages/openclaw-coven/README.md"

        duplicate_fixture = self.o3_structure_documents()
        section = module.markdown_section(
            duplicate_fixture[path], "Version compatibility"
        )
        item = module.markdown_list_item(
            section or "", "GET /api/v1/health"
        )
        self.assertIsNotNone(item)
        corrupt_item = (item or "").replace(
            "`requestAdoptionContracts`", "`removedCapability`"
        )
        corrupt_item += "\n  Decoy: `requestAdoptionContracts`."
        duplicate_fixture[path] = self.replace_once(
            duplicate_fixture[path],
            item or "",
            f"{item}\n{corrupt_item}",
        )
        errors = module.validate_o3_document_structures(duplicate_fixture)
        self.assertIn(
            f"{path}: minimal health fixture claim is ambiguous (found 2)",
            errors,
        )

        duplicate_dto = self.o3_structure_documents()
        section = module.markdown_section(
            duplicate_dto[path], "Version compatibility"
        )
        marker = "The complete current 16-field Rust health-capability DTO"
        paragraph = module.markdown_paragraph(section or "", marker)
        self.assertIsNotNone(paragraph)
        corrupt_paragraph = (paragraph or "").replace(
            "`requestAdoptionContracts`", "`removedCapability`"
        )
        corrupt_paragraph += (
            "\nDecoy: `requestAdoptionContracts` and "
            "../../docs/API-CONTRACT.md#get-apiv1health."
        )
        duplicate_dto[path] = self.replace_once(
            duplicate_dto[path],
            paragraph or "",
            f"{paragraph}\n\n{corrupt_paragraph}",
        )
        errors = module.validate_o3_document_structures(duplicate_dto)
        self.assertIn(
            f"{path}: canonical health DTO reference is ambiguous (found 2)",
            errors,
        )

    def test_health_examples_ignore_request_adoption_decoys(self) -> None:
        examples = {
            "docs/reference/api-contract.md": "Negotiation",
            "docs/daemon/socket-api.md": "Handshake",
        }
        for path, heading in examples.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                section = module.markdown_section(changed[path], heading)
                self.assertIsNotNone(section)
                example = module.http_json_example(
                    section or "", "GET /api/v1/health"
                )
                self.assertIsNotNone(example)
                mutated = self.replace_once(
                    example or "",
                    '"requestAdoptionContracts"',
                    '"removedCapability"',
                )
                changed[path] = self.replace_once(
                    changed[path], example or "", mutated
                )
                changed[path] += "\n\nDecoy: requestAdoptionContracts.\n"

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: health example missing requestAdoptionContracts",
                    errors,
                )

    def test_capability_lists_ignore_request_adoption_decoys(self) -> None:
        capability_lists = {
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
        for path, (heading, marker) in capability_lists.items():
            with self.subTest(path=path):
                changed = self.o3_structure_documents()
                section = module.markdown_section(changed[path], heading)
                self.assertIsNotNone(section)
                capability_list = module.markdown_paragraph(section or "", marker)
                self.assertIsNotNone(capability_list)
                mutated = self.replace_once(
                    capability_list or "",
                    "`requestAdoptionContracts`",
                    "`removedCapability`",
                )
                changed[path] = self.replace_once(
                    changed[path], capability_list or "", mutated
                )
                changed[path] += "\n\nDecoy: `requestAdoptionContracts`.\n"

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: capability list missing requestAdoptionContracts",
                    errors,
                )

    def test_adopted_route_tables_ignore_route_decoys(self) -> None:
        route_sections = {
            "docs/reference/api.md": "Sessions and events",
            "docs/reference/api-contract.md": "Negotiation",
            "docs/daemon/socket-api.md": "Endpoints",
        }
        routes = (
            "/api/v1/adopted-sessions",
            "/api/v1/sessions/:id/adopted-input",
        )
        for path, heading in route_sections.items():
            for route in routes:
                with self.subTest(path=path, route=route):
                    changed = self.o3_structure_documents()
                    section = module.markdown_section(changed[path], heading)
                    self.assertIsNotNone(section)
                    row = module.markdown_route_row(section or "", "POST", route)
                    self.assertIsNotNone(row)
                    changed[path] = self.replace_once(changed[path], row or "", "")
                    changed[path] += f"\n\nDecoy: `{route}`.\n"

                    errors = module.validate_o3_document_structures(changed)
                    self.assertIn(
                        f"{path}: adopted route table missing POST {route}",
                        errors,
                    )

    def test_o3_error_surfaces_ignore_error_decoys(self) -> None:
        routes = (
            "/api/v1/adopted-sessions",
            "/api/v1/sessions/:id/adopted-input",
        )
        error_codes = (
            "request_adoption_required",
            "request_adoption_invalid",
            "request_adoption_unsupported",
            "request_adoption_conflict",
        )
        route_tables = {
            "docs/reference/api.md": "Sessions and events",
            "docs/reference/api-contract.md": "Negotiation",
        }
        for path, heading in route_tables.items():
            for route in routes:
                for code in error_codes:
                    with self.subTest(path=path, route=route, code=code):
                        changed = self.o3_structure_documents()
                        section = module.markdown_section(changed[path], heading)
                        row = module.markdown_route_row(section or "", "POST", route)
                        self.assertIsNotNone(row)
                        mutated = self.replace_once(
                            row or "", code, "removed_adoption_error"
                        )
                        changed[path] = self.replace_once(
                            changed[path], row or "", mutated
                        )
                        changed[path] += f"\n\nDecoy: `{code}`.\n"

                        errors = module.validate_o3_document_structures(changed)
                        self.assertIn(
                            f"{path}: {route} row missing O3 error {code}",
                            errors,
                        )

        path = "docs/daemon/socket-api.md"
        for code in error_codes:
            with self.subTest(path=path, code=code):
                changed = self.o3_structure_documents()
                section = module.markdown_section(changed[path], "Endpoints")
                error_list = module.markdown_paragraph(
                    section or "", "Their O3-specific errors are"
                )
                self.assertIsNotNone(error_list)
                mutated = self.replace_once(
                    error_list or "", code, "removed_adoption_error"
                )
                changed[path] = self.replace_once(
                    changed[path], error_list or "", mutated
                )
                changed[path] += f"\n\nDecoy: `{code}`.\n"

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(f"{path}: O3 error list missing {code}", errors)

    def test_openclaw_fixture_health_claim_ignores_decoys(self) -> None:
        path = "packages/openclaw-coven/README.md"
        changed = self.o3_structure_documents()
        section = module.markdown_section(changed[path], "Version compatibility")
        fixture_item = module.markdown_list_item(
            section or "", "GET /api/v1/health"
        )
        self.assertIsNotNone(fixture_item)
        mutated = self.replace_once(
            fixture_item or "",
            "`requestAdoptionContracts`",
            "`removedCapability`",
        )
        changed[path] = self.replace_once(changed[path], fixture_item or "", mutated)
        changed[path] += "\n\nDecoy: `requestAdoptionContracts`.\n"

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            f"{path}: minimal health fixture missing requestAdoptionContracts",
            errors,
        )

    def test_openclaw_adopted_method_rows_ignore_decoys(self) -> None:
        path = "packages/openclaw-coven/README.md"
        methods = {
            "launchAdoptedSession": (
                "/api/v1/adopted-sessions",
                "`201",
                "`200",
            ),
            "sendAdoptedInput": (
                "/api/v1/sessions/:id/adopted-input",
                "`202",
                "`200",
            ),
        }
        for method, row_literals in methods.items():
            for literal in (method,) + row_literals:
                with self.subTest(method=method, literal=literal):
                    changed = self.o3_structure_documents()
                    section = module.markdown_section(
                        changed[path], "Adopted client methods", level=3
                    )
                    row = module.markdown_table_row(section or "", f"`{method}`")
                    self.assertIsNotNone(row)
                    mutated = self.replace_once(row or "", literal, "removedClaim")
                    changed[path] = self.replace_once(
                        changed[path], row or "", mutated
                    )
                    changed[path] += f"\n\nDecoy: `{literal}`.\n"

                    errors = module.validate_o3_document_structures(changed)
                    if literal == method:
                        expected = (
                            f"{path}: adopted method table missing {method}"
                        )
                    elif literal.startswith("/"):
                        expected = (
                            f"{path}: {method} Dedicated route cell must equal "
                            f"POST {row_literals[0]}"
                        )
                    elif literal == row_literals[1]:
                        expected = (
                            f"{path}: {method} first-adoption "
                            "status/result is incorrect"
                        )
                    else:
                        expected = (
                            f"{path}: {method} exact-replay "
                            "status/result is incorrect"
                        )
                    self.assertIn(expected, errors)

    def test_openclaw_adopted_negotiation_claims_ignore_decoys(self) -> None:
        path = "packages/openclaw-coven/README.md"
        changed = self.o3_structure_documents()
        section = module.markdown_section(
            changed[path], "Adopted client methods", level=3
        )
        self.assertIsNotNone(section)
        mutated = self.replace_once(
            section or "", "never falls back", "may fall back"
        )
        changed[path] = self.replace_once(changed[path], section or "", mutated)
        changed[path] += "\n\nDecoy: never falls back.\n"

        errors = module.validate_o3_document_structures(changed)
        self.assertIn(
            f"{path}: O3 negotiation claim must prohibit legacy mutation fallback",
            errors,
        )

    def test_cli_sacrifice_retention_claim_ignores_decoys(self) -> None:
        path = "packages/cli/README.md"
        cases = (
            ("retention/fence", "release mechanism"),
            ("permanent in O3", "temporary in O3"),
            ("future\napproved", "currently\navailable"),
        )
        for literal, replacement in cases:
            with self.subTest(literal=literal):
                changed = self.o3_structure_documents()
                section = module.markdown_section(changed[path], "Commands")
                self.assertIsNotNone(section)
                mutated = self.replace_once(section or "", literal, replacement)
                changed[path] = self.replace_once(
                    changed[path], section or "", mutated
                )
                changed[path] += f"\n\nDecoy: {literal}.\n"

                errors = module.validate_o3_document_structures(changed)
                self.assertIn(
                    f"{path}: O3 sacrifice retention boundary is missing", errors
                )

    def test_every_public_transport_statement_is_guarded(self) -> None:
        documents = {
            path: (module.ROOT / path).read_text(encoding="utf-8")
            for path in module.SESSION_LAUNCH_POLICY_DOCS
        }
        for path in module.SESSION_LAUNCH_POLICY_DOCS:
            with self.subTest(path=path):
                changed = dict(documents)
                changed[path] = changed[path].replace("TCP", "remote transport")
                changed[path] = changed[path].replace("tcp", "remote transport")
                errors = module.validate_session_launch_policy_docs(changed)
                self.assertIn(
                    f"{path}: launchPolicy transport boundary is missing", errors
                )

    def test_guards_public_contract_guides(self) -> None:
        for path in self.PUBLIC_CONTRACT_DOCS:
            with self.subTest(path=path):
                self.assertIn(path, module.CONTRACT_DOCS)

    def test_guards_all_health_guidance(self) -> None:
        health_docs = getattr(module, "HEALTH_GUIDANCE_DOCS", ())
        self.assertTrue(set(module.CONTRACT_DOCS).issubset(health_docs))
        for path in self.EXTRA_HEALTH_GUIDANCE_DOCS:
            with self.subTest(path=path):
                self.assertIn(path, health_docs)

    def test_rejects_stale_health_claim_in_extra_guides(self) -> None:
        for path in self.EXTRA_HEALTH_GUIDANCE_DOCS:
            with self.subTest(path=path):
                documents = self.canonical_documents()
                documents[path] = (
                    "GET /api/v1/health includes supportedApiVersions."
                )
                errors = module.validate_documents(documents)
                self.assertIn(
                    f"{path}: health must not advertise supportedApiVersions",
                    errors,
                )

    def test_rejects_supported_api_versions_in_health_guidance(self) -> None:
        for path in self.PUBLIC_CONTRACT_DOCS:
            with self.subTest(path=path):
                documents = self.canonical_documents()
                documents[path] = documents["docs/API.md"] + (
                    "\n\nGET /api/v1/health returns coven.daemon.v1 and "
                    "includes supportedApiVersions.\n"
                )
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any(
                        "health must not advertise supportedApiVersions" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_rejects_shared_health_and_legacy_positive_predicate(self) -> None:
        statements = (
            (
                "`GET /api/v1/health` and `GET /api/v1/api-version` include "
                "`supportedApiVersions`."
            ),
            (
                '"GET /api/v1/health" and "GET /api/v1/api-version" include '
                '"supportedApiVersions".'
            ),
        )
        for statement in statements:
            with self.subTest(statement=statement):
                documents = self.canonical_documents()
                path = "packages/openclaw-coven/README.md"
                documents[path] = documents["docs/API.md"] + f"\n\n{statement}\n"
                errors = module.validate_documents(documents)
                self.assertIn(
                    f"{path}: health must not advertise supportedApiVersions",
                    errors,
                )

    def test_accepts_legacy_field_after_explicit_health_absence(self) -> None:
        statements = (
            (
                "GET /api/v1/health does not include supportedApiVersions; "
                "GET /api/v1/api-version includes supportedApiVersions."
            ),
            (
                "supportedApiVersions is absent from GET /api/v1/health; "
                "GET /api/v1/api-version includes supportedApiVersions."
            ),
        )
        for statement in statements:
            with self.subTest(statement=statement):
                documents = self.canonical_documents()
                path = "packages/openclaw-coven/README.md"
                documents[path] = documents["docs/API.md"] + f"\n\n{statement}\n"
                self.assertEqual(module.validate_documents(documents), [])

    def test_rejects_ambiguous_or_double_negative_health_disposition(self) -> None:
        statements = (
            "GET /api/v1/health does not omit supportedApiVersions.",
            "GET /api/v1/health does not exclude supportedApiVersions.",
            "GET /api/v1/health cannot omit supportedApiVersions.",
            "GET /api/v1/health can't omit supportedApiVersions.",
            "GET /api/v1/health doesn't omit supportedApiVersions.",
            "GET /api/v1/health fails to omit supportedApiVersions.",
            "GET /api/v1/health no longer omits supportedApiVersions.",
            "GET /api/v1/health is not complete without supportedApiVersions.",
            "GET /api/v1/health never omits supportedApiVersions.",
            "supportedApiVersions is not omitted from GET /api/v1/health.",
            "supportedApiVersions is not excluded from GET /api/v1/health.",
            "supportedApiVersions is not complete without GET /api/v1/health.",
            "supportedApiVersions is never omitted from GET /api/v1/health.",
        )
        for statement in statements:
            with self.subTest(statement=statement):
                documents = self.canonical_documents()
                path = "packages/openclaw-coven/README.md"
                documents[path] = documents["docs/API.md"] + f"\n\n{statement}\n"
                errors = module.validate_documents(documents)
                self.assertIn(
                    f"{path}: health must not advertise supportedApiVersions",
                    errors,
                )

    def test_accepts_explicit_health_field_absence_matrix(self) -> None:
        statements = (
            "GET /api/v1/health excludes supportedApiVersions.",
            "GET /api/v1/health lacks supportedApiVersions.",
            "GET /api/v1/health omits supportedApiVersions.",
            "GET /api/v1/health does not include supportedApiVersions.",
            "GET /api/v1/health doesn't include supportedApiVersions.",
            (
                "GET /api/v1/health and GET /api/v1/api-version do not "
                "include supportedApiVersions."
            ),
            (
                "GET /api/v1/health and GET /api/v1/api-version don't "
                "include supportedApiVersions."
            ),
            "GET /api/v1/health did not include supportedApiVersions.",
            "GET /api/v1/health didn't include supportedApiVersions.",
            "GET /api/v1/health has removed supportedApiVersions.",
            "supportedApiVersions is absent from GET /api/v1/health.",
            "supportedApiVersions is not returned by GET /api/v1/health.",
            "supportedApiVersions has been removed from GET /api/v1/health.",
        )
        for statement in statements:
            with self.subTest(statement=statement):
                documents = self.canonical_documents()
                path = "packages/openclaw-coven/README.md"
                documents[path] = documents["docs/API.md"] + f"\n\n{statement}\n"
                self.assertEqual(module.validate_documents(documents), [])

    def test_rejects_supported_api_versions_in_health_field_lists(self) -> None:
        structures = (
            "GET /api/v1/health returns:\n- supportedApiVersions\n",
            "GET /api/v1/health returns:\n* supportedApiVersions\n",
            (
                "GET /api/v1/health returns:\n"
                "| Field | Type |\n"
                "|---|---|\n"
                "| supportedApiVersions | array |\n"
            ),
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for structure in structures:
                with self.subTest(path=path, structure=structure):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        f"\n\n{structure}"
                    )
                    errors = module.validate_documents(documents)
                    self.assertTrue(
                        any(
                            "health must not advertise supportedApiVersions" in error
                            for error in errors
                        ),
                        errors,
                    )

    def test_rejects_blank_line_health_field_blocks(self) -> None:
        structures = (
            (
                "GET /api/v1/health returns these fields:\n\n"
                "- supportedApiVersions\n"
            ),
            (
                "GET /api/v1/health returns these fields:\n\n"
                "* supportedApiVersions\n"
            ),
            (
                "GET /api/v1/health returns these fields:\n\n"
                "| Field | Type |\n"
                "|---|---|\n"
                "| supportedApiVersions | array |\n"
            ),
            (
                "GET /api/v1/health returns these fields:\n\n"
                "Field | Type\n"
                "---|---\n"
                "supportedApiVersions | array\n"
            ),
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for structure in structures:
                with self.subTest(path=path, structure=structure):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        f"\n\n{structure}"
                    )
                    errors = module.validate_documents(documents)
                    self.assertIn(
                        f"{path}: health must not advertise supportedApiVersions",
                        errors,
                    )

    def test_rejects_legacy_source_inside_health_field_lists(self) -> None:
        structures = (
            (
                "GET /api/v1/health returns:\n"
                "- supportedApiVersions (source: GET /api/v1/api-version)\n"
            ),
            (
                "GET /api/v1/health returns:\n"
                "| Field | Source |\n"
                "|---|---|\n"
                "| supportedApiVersions | GET /api/v1/api-version |\n"
            ),
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for structure in structures:
                with self.subTest(path=path, structure=structure):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        f"\n\n{structure}"
                    )
                    errors = module.validate_documents(documents)
                    self.assertTrue(
                        any(
                            "health must not advertise supportedApiVersions" in error
                            for error in errors
                        ),
                        errors,
                    )

    def test_resets_health_field_scope_after_intervening_prose(self) -> None:
        guidance = (
            (
                "GET /api/v1/health returns documented fields.\n\n"
                "This intervening prose is not a structural field block.\n\n"
                "- supportedApiVersions belongs to an unrelated example.\n"
            ),
            (
                "See the health guide for details.\n\n"
                "- supportedApiVersions belongs to unrelated legacy example.\n"
            ),
            (
                "Health negotiation is described above.\n\n"
                "- GET /api/v1/api-version returns supportedApiVersions.\n"
            ),
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for statement in guidance:
                with self.subTest(path=path, statement=statement):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        f"\n\n{statement}"
                    )
                    self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_supported_api_versions_in_legacy_field_lists(self) -> None:
        field_blocks = (
            "- supportedApiVersions\n",
            (
                "| Field | Type |\n"
                "|---|---|\n"
                "| supportedApiVersions | array |\n"
            ),
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for field_block in field_blocks:
                with self.subTest(path=path, field_block=field_block):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        "\n\nGET /api/v1/api-version is a legacy route-family "
                        "diagnostic returning literal `v1` with fields:\n\n"
                        f"{field_block}"
                    )
                    self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_removed_supported_api_versions_health_field(self) -> None:
        field_blocks = (
            "- supportedApiVersions has been removed\n",
            "- GET /api/v1/health does not include supportedApiVersions\n",
            (
                "| Field | Disposition |\n"
                "|---|---|\n"
                "| supportedApiVersions | has been removed from "
                "GET /api/v1/health |\n"
            ),
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for field_block in field_blocks:
                with self.subTest(path=path, field_block=field_block):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        "\n\nGET /api/v1/health returns documented fields:\n\n"
                        f"{field_block}"
                    )
                    self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_explicit_supported_api_versions_health_removal(self) -> None:
        guidance = (
            "GET /api/v1/health does not include supportedApiVersions.",
            "GET /api/v1/health has removed supportedApiVersions.",
        )
        for path in self.PUBLIC_CONTRACT_DOCS:
            for statement in guidance:
                with self.subTest(path=path, statement=statement):
                    documents = self.canonical_documents()
                    documents[path] = documents["docs/API.md"] + (
                        f"\n\n{statement}\n"
                    )
                    self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_supported_api_versions_on_legacy_route_only(self) -> None:
        for path in self.PUBLIC_CONTRACT_DOCS:
            with self.subTest(path=path):
                documents = self.canonical_documents()
                documents[path] = documents["docs/API.md"] + (
                    "\n\nGET /api/v1/api-version is a legacy route-family "
                    "diagnostic returning literal `v1` and "
                    "supportedApiVersions: [`v1`].\n"
                )
                self.assertEqual(module.validate_documents(documents), [])

    def test_rejects_legacy_endpoint_as_named_handshake(self) -> None:
        documents = self.canonical_documents()
        documents["docs/reference/api-contract.md"] = (
            "The legacy route-family GET /api/v1/api-version is the "
            "coven.daemon.v1 compatibility handshake."
        )
        errors = module.validate_documents(documents)
        self.assertTrue(any("legacy route" in error for error in errors))

    def test_requires_legacy_route_explanation_in_every_contract_guide(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            "",
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any("legacy route explanation is missing" in error for error in errors),
            errors,
        )

    def test_rejects_non_standalone_literal_v1_tokens(self) -> None:
        invalid_tokens = (
            "v1-beta",
            "pre-v1",
            "/api/v1",
            "coven.daemon.v1",
            "v10",
            "v1.0",
            "v1/beta",
            "v1+meta",
        )
        for invalid in invalid_tokens:
            with self.subTest(invalid=invalid):
                documents = self.canonical_documents()
                documents["docs/reference/api-contract.md"] = documents[
                    "docs/reference/api-contract.md"
                ].replace("literal `v1`", f"literal `{invalid}`")
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any(
                        "legacy route explanation is missing" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_rejects_uppercase_quoted_and_backticked_v1(self) -> None:
        for token in ('"V1"', "`V1`"):
            with self.subTest(token=token):
                documents = self.canonical_documents()
                documents["docs/reference/api-contract.md"] = documents[
                    "docs/reference/api-contract.md"
                ].replace("literal `v1`", f"literal {token}")
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any(
                        "legacy route explanation is missing" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_rejects_api_version_route_as_compatibility_handshake(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is the coven.daemon.v1 compatibility "
                "handshake and returns literal v1."
            ),
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any("legacy route presented as named-contract handshake" in error for error in errors),
            errors,
        )

    def test_unrelated_not_and_proof_do_not_hide_positive_handshake_claim(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is a legacy route-family diagnostic "
                "returning literal v1 and is the coven.daemon.v1 compatibility "
                "handshake. This unrelated note is not proof that capabilities "
                "authorize actions."
            ),
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any("legacy route presented as named-contract handshake" in error for error in errors),
            errors,
        )

    def test_rejects_positive_named_contract_claim_variants(self) -> None:
        explanations = (
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1` and provides the coven.daemon.v1 compatibility handshake.",
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1` and proves coven.daemon.v1 compatibility.",
            "Use GET /api/v1/api-version, a legacy route-family diagnostic returning "
            "literal `v1`, as proof of coven.daemon.v1 compatibility.",
            "The coven.daemon.v1 compatibility handshake uses GET /api/v1/api-version, "
            "a legacy route-family diagnostic returning literal `v1`.",
        )
        for explanation in explanations:
            with self.subTest(explanation=explanation):
                documents = self.canonical_documents()
                self.replace_legacy_explanation(
                    documents,
                    "docs/reference/api-contract.md",
                    explanation,
                )
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any(
                        "legacy route presented as named-contract handshake" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_rejects_clause_local_negation_bypasses(self) -> None:
        explanations = (
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1`, not deprecated and provides the coven.daemon.v1 "
            "compatibility handshake.",
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1`, not proof of authorization and proves coven.daemon.v1 "
            "compatibility.",
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1`. Clients negotiate coven.daemon.v1 through this route.",
        )
        for explanation in explanations:
            with self.subTest(explanation=explanation):
                documents = self.canonical_documents()
                self.replace_legacy_explanation(
                    documents,
                    "docs/reference/api-contract.md",
                    explanation,
                )
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any(
                        "legacy route presented as named-contract handshake" in error
                        for error in errors
                    ),
                    errors,
                )

    def test_rejects_positive_pronoun_continuation(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is a legacy route-family diagnostic "
                "returning literal `v1`. It provides the coven.daemon.v1 "
                "compatibility handshake."
            ),
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any(
                "legacy route presented as named-contract handshake" in error
                for error in errors
            ),
            errors,
        )

    def test_unrelated_negation_does_not_hide_positive_continuation(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is a legacy route-family diagnostic "
                "returning literal `v1`. It is not proof of authorization, but "
                "it provides the coven.daemon.v1 compatibility handshake."
            ),
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any(
                "legacy route presented as named-contract handshake" in error
                for error in errors
            ),
            errors,
        )

    def test_accepts_modified_legitimate_negation(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is a legacy route-family diagnostic "
                "returning literal `v1` and is absolutely not proof of "
                "coven.daemon.v1 compatibility."
            ),
        )
        self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_cannot_and_fails_to_prove_negations(self) -> None:
        explanations = (
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1` and cannot prove coven.daemon.v1 compatibility.",
            "GET /api/v1/api-version is a legacy route-family diagnostic returning "
            "literal `v1` and fails to prove coven.daemon.v1 compatibility.",
        )
        for explanation in explanations:
            with self.subTest(explanation=explanation):
                documents = self.canonical_documents()
                self.replace_legacy_explanation(
                    documents,
                    "docs/reference/api-contract.md",
                    explanation,
                )
                self.assertEqual(module.validate_documents(documents), [])

    def test_rejects_immediate_pronoun_verification_claim(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is a legacy route-family diagnostic "
                "returning literal `v1`. It verifies support for coven.daemon.v1."
            ),
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any(
                "legacy route presented as named-contract handshake" in error
                for error in errors
            ),
            errors,
        )

    def test_ignores_later_health_scoped_compatibility_sentence(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "GET /api/v1/api-version is a legacy route-family diagnostic "
                "returning literal `v1`. Separately, clients negotiate "
                "coven.daemon.v1 compatibility through GET /api/v1/health.\n"
                "| GET | `/api/v1/api-version` | legacy route-family literal `v1` |"
            ),
        )
        self.assertEqual(module.validate_documents(documents), [])

    def test_rejects_same_statement_legacy_health_contradiction(self) -> None:
        documents = self.canonical_documents()
        self.replace_legacy_explanation(
            documents,
            "docs/reference/api-contract.md",
            (
                "`GET /api/v1/api-version`, not `GET /api/v1/health`, is the "
                "`coven.daemon.v1` compatibility handshake and returns legacy "
                "route-family literal `v1`."
            ),
        )
        errors = module.validate_documents(documents)
        self.assertTrue(
            any(
                "legacy route presented as named-contract handshake" in error
                for error in errors
            ),
            errors,
        )

    def test_requires_health_handshake_in_every_contract_guide(self) -> None:
        documents = self.canonical_documents()
        documents["docs/ARCHITECTURE.md"] = "coven.daemon.v1"
        errors = module.validate_documents(documents)
        self.assertTrue(any("missing health handshake" in error for error in errors))

    def test_requires_all_lifecycle_and_authority_boundaries(self) -> None:
        documents = self.canonical_documents()
        documents["docs/API-CONTRACT.md"] = (
            "GET /api/v1/health coven.daemon.v1 `created` `running` `completed`"
        )
        errors = module.validate_documents(documents)
        self.assertTrue(any("missing lifecycle status idle" in error for error in errors))
        self.assertTrue(
            any("capabilities versus authorization" in error for error in errors)
        )
        self.assertTrue(any("synthetic active distinction" in error for error in errors))

    def test_rejects_idle_as_terminal(self) -> None:
        documents = self.canonical_documents()
        documents["docs/SESSION-LIFECYCLE.md"] = documents[
            "docs/SESSION-LIFECYCLE.md"
        ].replace("| `idle` | No |", "| `idle` | Yes |")
        errors = module.validate_documents(documents)
        self.assertTrue(
            any(
                "incorrect terminal classification for idle" in error
                for error in errors
            )
        )

    def test_rejects_each_lifecycle_authority_boundary_when_missing(self) -> None:
        cases = {
            "not proof of acknowledged process termination": (
                "killed acknowledgement boundary"
            ),
            "Synthetic `active` is not a harness-session state.": (
                "synthetic active distinction"
            ),
            "stored separately in `archived_at`": "archive separation",
            (
                "stale unowned rows without keyed launch-adoption or historical "
                "reservation evidence recover as `failed`"
            ): "stale created recovery",
        }
        for phrase, expected_error in cases.items():
            with self.subTest(phrase=phrase):
                documents = self.canonical_documents()
                documents["docs/API-CONTRACT.md"] = documents[
                    "docs/API-CONTRACT.md"
                ].replace(phrase, "")
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any(expected_error in error for error in errors),
                    errors,
                )

    def test_bounded_recovery_claims_reject_processing_retained_rows(self) -> None:
        documents = self.lifecycle_documents()
        baseline: list[str] = []
        module.validate_lifecycle_recovery_docs(documents, baseline)
        self.assertEqual(baseline, [])

        for path, (heading, level, marker) in (
            module.LIFECYCLE_RECOVERY_CLAIMS.items()
        ):
            with self.subTest(path=path):
                changed = dict(documents)
                section = module.markdown_section(
                    changed[path], heading, level=level
                )
                self.assertIsNotNone(section)
                claim = module.markdown_paragraph(section or "", marker)
                self.assertIsNotNone(claim)
                if path == "docs/API-CONTRACT.md":
                    corrupt = self.replace_once(
                        claim or "",
                        "excludes every session",
                        "processes every session",
                    )
                else:
                    corrupt = self.replace_once(
                        claim or "",
                        "without launch-adoption or historical reservation evidence",
                        "including launch-adoption and historical reservation evidence",
                    )
                changed[path] = self.replace_once(
                    changed[path], claim or "", corrupt
                )
                changed[path] += (
                    "\n\n# Checker decoy\nGeneric stale-created recovery "
                    "excludes keyed launch adoption and historical/null-key "
                    "reservation rows.\n"
                )

                errors: list[str] = []
                module.validate_lifecycle_recovery_docs(changed, errors)
                self.assertIn(
                    f"{path}: stale-created recovery must exclude keyed launch "
                    "adoptions and historical/null-key reservations",
                    errors,
                )

    def test_bounded_recovery_claims_reject_negated_or_contradictory_exclusion(
        self,
    ) -> None:
        invalid_claims = {
            "docs/API-CONTRACT.md": (
                "Generic stale-created recovery runs without excluding adopted "
                "launch rows or historical attempt reservations.",
                "Generic stale-created recovery says adopted launch rows and "
                "historical attempt reservations are not excluded.",
                "Generic stale-created recovery excludes adopted launch rows and "
                "historical attempt reservations but also fails those adopted rows.",
                "Generic stale-created recovery excludes adopted launch rows and "
                "historical attempt reservations but includes them for processing.",
            ),
            "docs/SESSION-LIFECYCLE.md": (
                "Generic stale-created recovery runs without excluding adopted "
                "launch rows or historical attempt reservations.",
                "Generic stale-created recovery says adopted launch rows and "
                "historical attempt reservations are not excluded.",
                "Generic stale-created recovery excludes adopted launch rows and "
                "historical attempt reservations but also fails those adopted rows.",
                "Generic stale-created recovery excludes adopted launch rows and "
                "historical attempt reservations but includes them for processing.",
            ),
            "docs/sessions/lifecycle.md": (
                "Marks only stale unowned `created` rows as `failed` without "
                "excluding adopted launch rows or historical attempt reservations.",
                "Marks only stale unowned `created` rows as `failed`; adopted "
                "launch rows and historical attempt reservations are not excluded.",
                "Marks only stale unowned `created` rows without launch-adoption or "
                "historical reservation evidence as `failed` but also fails adopted "
                "launch rows.",
                "Marks only stale unowned `created` rows without launch-adoption or "
                "historical reservation evidence as `failed` but includes adopted "
                "launch rows for processing.",
            ),
        }
        self.assertEqual(set(invalid_claims), set(module.LIFECYCLE_RECOVERY_CLAIMS))
        for path, claims in invalid_claims.items():
            heading, level, marker = module.LIFECYCLE_RECOVERY_CLAIMS[path]
            for claim in claims:
                with self.subTest(path=path, claim=claim):
                    changed = self.lifecycle_documents()
                    section = module.markdown_section(
                        changed[path], heading, level=level
                    )
                    original = module.markdown_paragraph(section or "", marker)
                    self.assertIsNotNone(original)
                    changed[path] = self.replace_once(
                        changed[path], original or "", claim
                    )

                    errors: list[str] = []
                    module.validate_lifecycle_recovery_docs(changed, errors)
                    self.assertIn(
                        f"{path}: stale-created recovery must exclude keyed "
                        "launch adoptions and historical/null-key reservations",
                        errors,
                    )

    def test_bounded_recovery_rejects_contradiction_after_affirmative_claim(
        self,
    ) -> None:
        for path, (heading, level, marker) in (
            module.LIFECYCLE_RECOVERY_CLAIMS.items()
        ):
            with self.subTest(path=path):
                changed = self.lifecycle_documents()
                section = module.markdown_section(
                    changed[path], heading, level=level
                )
                claim = module.markdown_paragraph(section or "", marker)
                self.assertIsNotNone(claim)
                contradiction = (
                    " That stale-created recovery includes adopted launch rows "
                    "and historical attempt reservations."
                )
                changed[path] = self.replace_once(
                    changed[path], claim or "", (claim or "") + contradiction
                )

                errors: list[str] = []
                module.validate_lifecycle_recovery_docs(changed, errors)
                self.assertIn(
                    f"{path}: stale-created recovery must exclude keyed launch "
                    "adoptions and historical/null-key reservations",
                    errors,
                )

    def test_bounded_recovery_targets_remain_failed(self) -> None:
        for path in module.LIFECYCLE_DOCS:
            with self.subTest(path=path):
                changed = self.lifecycle_documents()
                if path == "docs/API-CONTRACT.md":
                    section = module.markdown_section(
                        changed[path], "Session record shape (`v1`)", level=2
                    )
                    table = module.markdown_table(
                        section or "",
                        ("Harness-session status", "Terminal?", "Meaning"),
                    )
                    rows = module.markdown_table_rows(
                        table, {"Harness-session status": "created"}
                    )
                    self.assertEqual(len(rows), 1)
                    row = rows[0]
                    corrupt = self.replace_once(
                        row.source, "recover to `failed`", "recover to `running`"
                    )
                    changed[path] = self.replace_once(
                        changed[path], row.source, corrupt
                    )
                else:
                    heading, level, marker = module.LIFECYCLE_RECOVERY_CLAIMS[path]
                    section = module.markdown_section(
                        changed[path], heading, level=level
                    )
                    claim = module.markdown_paragraph(section or "", marker)
                    self.assertIsNotNone(claim)
                    corrupt = self.replace_once(
                        claim or "", "as `failed`", "as `running`"
                    )
                    changed[path] = self.replace_once(
                        changed[path], claim or "", corrupt
                    )
                changed[path] += (
                    "\n\n# Checker decoy\nStale unowned `created` rows without "
                    "launch-adoption or historical reservation evidence recover "
                    "as `failed`.\n"
                )

                errors: list[str] = []
                module.validate_lifecycle_recovery_docs(changed, errors)
                self.assertIn(f"{path}: stale created recovery is missing", errors)

    def test_rejects_stale_created_recovery_as_running(self) -> None:
        documents = self.canonical_documents()
        documents["docs/API-CONTRACT.md"] = documents[
            "docs/API-CONTRACT.md"
        ].replace(
            "stale unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `failed`",
            "stale unowned `created` rows without keyed launch-adoption or "
            "historical reservation evidence recover as `running`",
        )
        documents["docs/API-CONTRACT.md"] += (
            "\nAn unrelated operation is marked as `failed`.\n"
        )
        errors = module.validate_documents(documents)
        self.assertTrue(any("stale created recovery" in error for error in errors))

    def test_rejects_unrelated_failure_in_same_created_row(self) -> None:
        documents = self.canonical_documents()
        documents["docs/API-CONTRACT.md"] = documents[
            "docs/API-CONTRACT.md"
        ].replace(
            "stale unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `failed`",
            "stale unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `running`; another operation "
            "marks `failed`",
        )
        errors = module.validate_documents(documents)
        self.assertTrue(any("stale created recovery" in error for error in errors))

    def test_rejects_ambiguous_stale_created_recovery_targets(self) -> None:
        for replacement in (
            "stale unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `running` or as `failed`",
            "stale unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `failed` or as `running`",
        ):
            with self.subTest(replacement=replacement):
                documents = self.canonical_documents()
                documents["docs/API-CONTRACT.md"] = documents[
                    "docs/API-CONTRACT.md"
                ].replace(
                    "stale unowned rows without keyed launch-adoption or "
                    "historical reservation evidence recover as `failed`",
                    replacement,
                )
                errors = module.validate_documents(documents)
                self.assertTrue(
                    any("stale created recovery" in error for error in errors)
                )

    def test_accepts_line_wrapped_stale_created_recovery_sentence(self) -> None:
        claim = (
            "Stale unowned `created` rows without launch-adoption or historical "
            "reservation evidence recover\nas `failed`."
        )
        self.assertTrue(
            module.has_stale_created_failure_recovery(claim)
        )

    def test_requires_stale_and_unowned_created_recovery(self) -> None:
        documents = self.canonical_documents()
        documents["docs/API-CONTRACT.md"] = documents[
            "docs/API-CONTRACT.md"
        ].replace(
            "stale unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `failed`",
            "Unowned rows without keyed launch-adoption or historical "
            "reservation evidence recover as `failed`",
        )
        errors = module.validate_documents(documents)
        self.assertTrue(any("stale created recovery" in error for error in errors))

    def test_main_reports_missing_document_without_traceback(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            stderr = io.StringIO()
            with mock.patch.object(module, "ROOT", pathlib.Path(directory)):
                with redirect_stderr(stderr):
                    result = module.main()

        self.assertEqual(result, 1)
        message = stderr.getvalue()
        self.assertIn("docs/API-CONTRACT.md: unable to read", message)
        self.assertNotIn("Traceback", message)


if __name__ == "__main__":
    unittest.main()
