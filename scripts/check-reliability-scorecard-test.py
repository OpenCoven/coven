#!/usr/bin/env python3
from __future__ import annotations

import copy
import importlib.util
import json
import pathlib
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = pathlib.Path(__file__).with_name("check-reliability-scorecard.py")
DATA_PATH = ROOT / "docs/reliability-scorecard.json"
MARKDOWN_PATH = ROOT / "docs/reliability-scorecard.md"


def load_checker():
    if not SCRIPT.is_file():
        return None
    spec = importlib.util.spec_from_file_location("check_reliability_scorecard", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


module = load_checker()


class ReliabilityScorecardTests(unittest.TestCase):
    def checker(self):
        self.assertIsNotNone(module, "check-reliability-scorecard.py is missing")
        return module

    def canonical_data(self) -> dict[str, object]:
        self.assertTrue(DATA_PATH.is_file(), "reliability scorecard data is missing")
        return json.loads(DATA_PATH.read_text(encoding="utf-8"))

    def validate(
        self,
        data: dict[str, object],
        *,
        markdown: str | None = None,
        readme: str | None = None,
    ) -> list[str]:
        checker = self.checker()
        return checker.validate_scorecard(
            data,
            (
                MARKDOWN_PATH.read_text(encoding="utf-8")
                if markdown is None
                else markdown
            ),
            (
                (ROOT / "README.md").read_text(encoding="utf-8")
                if readme is None
                else readme
            ),
            ROOT,
        )

    def test_current_scorecard_is_valid(self) -> None:
        self.assertEqual(self.validate(self.canonical_data()), [])

    def test_every_metric_contract_field_is_required(self) -> None:
        data = self.canonical_data()
        del data["rows"][0]["confidence"]
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("missing required field: confidence" in error for error in errors))

    def test_all_required_metric_categories_are_present(self) -> None:
        data = self.canonical_data()
        data["rows"] = [
            row for row in data["rows"] if row["category"] != "usefulness"
        ]
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("missing required category: usefulness" in error for error in errors))

    def test_undeclared_metric_categories_are_rejected(self) -> None:
        data = self.canonical_data()
        data["rows"][0]["category"] = "bogus-category"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("unsupported category" in error for error in errors))

    def test_benchmark_evidence_cannot_be_observed_current(self) -> None:
        data = self.canonical_data()
        benchmark = next(
            row for row in data["rows"] if row["evidenceKind"] == "benchmark"
        )
        benchmark["status"] = "Observed current"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("benchmark evidence must use status" in error for error in errors))

    def test_benchmark_rows_require_retained_evidence(self) -> None:
        for reference in (None, "#self-proof"):
            with self.subTest(reference=reference):
                data = self.canonical_data()
                benchmark = next(
                    row for row in data["rows"] if row["evidenceKind"] == "benchmark"
                )
                benchmark["evidenceRef"] = reference
                errors = self.validate(
                    data,
                    markdown=self.checker().render_scorecard(data),
                )
                self.assertTrue(
                    any("benchmark row requires a retained evidenceRef" in error for error in errors)
                )

    def test_retained_local_evidence_is_linked_and_must_resolve(self) -> None:
        data = self.canonical_data()
        rendered = self.checker().render_scorecard(data)
        self.assertIn(
            "[scripts/benchmark-cli.mjs](../scripts/benchmark-cli.mjs)",
            rendered,
        )

        benchmark = next(
            row for row in data["rows"] if row["evidenceKind"] == "benchmark"
        )
        benchmark["evidenceRef"] = "scripts/does-not-exist.mjs"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("evidenceRef does not resolve" in error for error in errors))

    def test_external_evidence_is_rejected_in_favor_of_retained_local_files(self) -> None:
        data = self.canonical_data()
        benchmark = next(
            row for row in data["rows"] if row["evidenceKind"] == "benchmark"
        )
        benchmark["evidenceRef"] = "https://evil.example/proof"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("repository-local file" in error for error in errors))

        benchmark["evidenceRef"] = (
            "https://github.com/OpenCoven/coven/actions/runs/1"
        )
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("repository-local file" in error for error in errors))

    def test_observed_current_requires_dated_external_evidence(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["status"] = "Observed current"
        row["evidenceKind"] = "release_receipt"
        row["value"] = "100%"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("observedAt" in error for error in errors))
        self.assertTrue(any("evidenceRef" in error for error in errors))

    def test_observed_date_must_be_iso_formatted(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["status"] = "Observed current"
        row["evidenceKind"] = "release_receipt"
        row["value"] = "100%"
        row["observedAt"] = "not-a-date"
        row["evidenceRef"] = "README.md"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("valid ISO date" in error for error in errors))

    def test_observed_current_requires_a_non_blank_value(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["status"] = "Observed current"
        row["evidenceKind"] = "release_receipt"
        row["value"] = "  "
        row["observedAt"] = "2026-09-04"
        row["evidenceRef"] = "README.md"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("observed row requires a value" in error for error in errors))

    def test_observed_evidence_cannot_be_a_self_reference(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["status"] = "Observed current"
        row["evidenceKind"] = "release_receipt"
        row["value"] = "100%"
        row["observedAt"] = "2026-09-04"
        row["evidenceRef"] = "#self-proof"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("retained evidenceRef" in error for error in errors))

    def test_evidence_reference_must_be_null_or_string(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["status"] = "Observed current"
        row["evidenceKind"] = "release_receipt"
        row["value"] = "100%"
        row["observedAt"] = "2026-09-04"
        row["evidenceRef"] = ["README.md"]
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("evidenceRef must be null or a string" in error for error in errors))

    def test_not_yet_measured_rows_require_an_owner_and_null_value(self) -> None:
        data = self.canonical_data()
        row = next(row for row in data["rows"] if row["status"] == "Not yet measured")
        row["owner"] = "unassigned"
        row["value"] = "probably healthy"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("named owner" in error for error in errors))
        self.assertTrue(any("null value" in error for error in errors))

    def test_thresholds_require_an_explicit_breach_action(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["target"] = "99%"
        row["action"] = ""
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("non-empty action" in error for error in errors))

    def test_zero_is_a_valid_target(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["status"] = "Target/SLO"
        row["evidenceKind"] = "target"
        row["target"] = 0
        row["value"] = None
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertFalse(any("target status requires a target" in error for error in errors))
        self.assertIn("Target: 0.", self.checker().render_scorecard(data))

    def test_privacy_sensitive_fields_are_forbidden(self) -> None:
        data = self.canonical_data()
        data["rows"][0]["prompt"] = "real user content"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("privacy-sensitive field" in error for error in errors))

    def test_unexpected_top_level_fields_are_rejected(self) -> None:
        data = self.canonical_data()
        data["prompt"] = "sensitive content"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("unexpected top-level field: prompt" in error for error in errors))

    def test_unexpected_row_fields_are_rejected(self) -> None:
        data = self.canonical_data()
        data["rows"][0]["notes"] = "raw prompt and repository content"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("unexpected row field: notes" in error for error in errors))

    def test_full_output_field_aliases_are_forbidden(self) -> None:
        for field in (
            "fullOutput",
            "full_output",
            "full-output",
            "full output",
            "full.output",
            "command output",
            "full command output",
            "credential ",
        ):
            with self.subTest(field=field):
                data = self.canonical_data()
                data["rows"][0][field] = "sensitive output"
                errors = self.validate(
                    data,
                    markdown=self.checker().render_scorecard(data),
                )
                self.assertTrue(
                    any("privacy-sensitive field" in error for error in errors)
                )

    def test_non_object_scorecard_reports_an_error_instead_of_crashing(self) -> None:
        checker = self.checker()
        self.assertEqual(checker.render_scorecard([]), "")
        errors = checker.validate_scorecard(
            [],
            "",
            (ROOT / "README.md").read_text(encoding="utf-8"),
            ROOT,
        )
        self.assertEqual(errors, ["scorecard root must be a JSON object"])

    def test_write_does_not_clobber_markdown_when_data_is_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "scorecard.md"
            output.write_text("keep this content", encoding="utf-8")
            errors = self.checker().write_scorecard(
                [],
                (ROOT / "README.md").read_text(encoding="utf-8"),
                output,
                ROOT,
            )
            self.assertEqual(errors, ["scorecard root must be a JSON object"])
            self.assertEqual(output.read_text(encoding="utf-8"), "keep this content")

    def test_malformed_identity_fields_report_errors_instead_of_crashing(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["id"] = []
        row["category"] = []
        row["status"] = []
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("id must be a non-empty string" in error for error in errors))
        self.assertTrue(any("category must be a non-empty string" in error for error in errors))
        self.assertTrue(any("status must be a non-empty string" in error for error in errors))

    def test_contract_fields_have_stable_scalar_types(self) -> None:
        data = self.canonical_data()
        row = data["rows"][0]
        row["definition"] = []
        row["value"] = {}
        row["target"] = []
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("definition must be a non-empty string" in error for error in errors))
        self.assertTrue(any("value must be null or a scalar" in error for error in errors))
        self.assertTrue(any("target must be null or a scalar" in error for error in errors))

    def test_metric_ids_must_be_stable_slugs(self) -> None:
        data = self.canonical_data()
        data["rows"][0]["id"] = "Not A Stable ID"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("id must be a lowercase slug" in error for error in errors))

    def test_non_observed_rows_require_null_observation_dates(self) -> None:
        data = self.canonical_data()
        data["rows"][0]["observedAt"] = "hidden content"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("observedAt must be null" in error for error in errors))

    def test_top_level_metadata_is_canonical(self) -> None:
        data = self.canonical_data()
        data["purpose"] = "hidden content"
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("purpose must match" in error for error in errors))

    def test_markdown_is_deterministically_rendered_from_json(self) -> None:
        data = self.canonical_data()
        markdown = MARKDOWN_PATH.read_text(encoding="utf-8").replace(
            "Not yet measured",
            "Observed current",
            1,
        )
        errors = self.validate(data, markdown=markdown)
        self.assertTrue(any("does not match the structured source" in error for error in errors))

    def test_readme_links_the_current_scorecard(self) -> None:
        data = self.canonical_data()
        readme = (ROOT / "README.md").read_text(encoding="utf-8").replace(
            "(docs/reliability-scorecard.md)",
            "(README.md)",
        )
        errors = self.validate(
            data,
            markdown=self.checker().render_scorecard(data),
            readme=readme,
        )
        self.assertTrue(any("README.md must link" in error for error in errors))

    def test_duplicate_metric_ids_fail(self) -> None:
        data = self.canonical_data()
        duplicate = copy.deepcopy(data["rows"][0])
        data["rows"].append(duplicate)
        errors = self.validate(data, markdown=self.checker().render_scorecard(data))
        self.assertTrue(any("duplicate metric id" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
