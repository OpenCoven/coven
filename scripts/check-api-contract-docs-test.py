from __future__ import annotations

import importlib.util
import io
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

    def canonical_documents(self) -> dict[str, str]:
        contract = """
Clients negotiate compatibility with GET /api/v1/health and coven.daemon.v1.
Capabilities advertise availability and never grant permission.
GET /api/v1/api-version is a legacy route-family diagnostic returning literal `v1`.
New clients must not use the legacy route as proof of coven.daemon.v1 compatibility.
| `created` | No | stale unowned rows recover as `failed`. |
| `running` | No | live |
| `idle` | No | reusable |
| `completed` | Yes | success |
| `failed` | Yes | failure |
| `killed` | Yes | accepted |
| `orphaned` | Yes | unresolved |
`killed` is not proof of acknowledged process termination.
Synthetic `active` is not a harness-session state.
Archive visibility is stored separately in `archived_at`.
"""
        return {path: contract for path in module.CONTRACT_DOCS + module.LIFECYCLE_DOCS}

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

    def test_accepts_canonical_handshake_and_lifecycle(self) -> None:
        self.assertEqual(module.validate_documents(self.canonical_documents()), [])

    def test_guards_public_contract_guides(self) -> None:
        for path in self.PUBLIC_CONTRACT_DOCS:
            with self.subTest(path=path):
                self.assertIn(path, module.CONTRACT_DOCS)

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

    def test_bounds_health_field_list_scope_to_one_paragraph(self) -> None:
        documents = self.canonical_documents()
        path = "packages/openclaw-coven/README.md"
        documents[path] = documents["docs/API.md"] + (
            "\n\nGET /api/v1/health returns documented fields.\n"
            "\n- supportedApiVersions belongs to an unrelated example.\n"
        )
        self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_supported_api_versions_in_legacy_field_lists(self) -> None:
        documents = self.canonical_documents()
        path = "packages/openclaw-coven/README.md"
        documents[path] = documents["docs/API.md"] + (
            "\n\nGET /api/v1/api-version is a legacy route-family diagnostic "
            "returning literal `v1` with fields:\n"
            "- supportedApiVersions\n"
        )
        self.assertEqual(module.validate_documents(documents), [])

    def test_accepts_removed_supported_api_versions_health_field(self) -> None:
        documents = self.canonical_documents()
        path = "packages/openclaw-coven/README.md"
        documents[path] = documents["docs/API.md"] + (
            "\n\nGET /api/v1/health returns documented fields:\n"
            "- supportedApiVersions has been removed\n"
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
            "stale unowned rows recover as `failed`": "stale created recovery",
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
