#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime
import json
import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
DATA_PATH = ROOT / "docs/reliability-scorecard.json"
MARKDOWN_PATH = ROOT / "docs/reliability-scorecard.md"
README_PATH = ROOT / "README.md"
SCHEMA_VERSION = "coven.reliability-scorecard.v1"
SCORECARD_TITLE = "Coven reliability, recovery, and usefulness scorecard"
SCORECARD_PURPOSE = (
    "Current decision view for issue #807. Values remain unmeasured unless "
    "retained outcome evidence exists."
)
README_LINK = "(docs/reliability-scorecard.md)"
TOP_LEVEL_FIELDS = {"schemaVersion", "title", "purpose", "rows"}
ALLOWED_STATUSES = {
    "Observed current",
    "Historical observation",
    "Target/SLO",
    "Benchmark condition/input",
    "Not yet measured",
}
REQUIRED_CATEGORIES = {
    "core_journey",
    "operation_reliability",
    "release_quality",
    "performance_resource",
    "agentfs_security",
    "api_compatibility",
    "usefulness",
}
REQUIRED_FIELDS = {
    "id",
    "category",
    "metric",
    "status",
    "evidenceKind",
    "definition",
    "numerator",
    "denominator",
    "cohort",
    "window",
    "source",
    "privacy",
    "value",
    "owner",
    "target",
    "confidence",
    "action",
    "observedAt",
    "evidenceRef",
}
NONEMPTY_TEXT_FIELDS = {
    "metric",
    "evidenceKind",
    "definition",
    "numerator",
    "denominator",
    "cohort",
    "window",
    "source",
    "privacy",
    "confidence",
}
FORBIDDEN_FIELDS = {
    "prompt",
    "prompts",
    "credential",
    "credentials",
    "commandoutput",
    "repositorycontent",
    "rawoutput",
    "fulloutput",
}
CATEGORY_LABELS = {
    "core_journey": "Core journey",
    "operation_reliability": "Operation reliability",
    "release_quality": "Release quality",
    "performance_resource": "Performance/resource",
    "agentfs_security": "AgentFS/security boundary",
    "api_compatibility": "API/client compatibility",
    "usefulness": "Usefulness",
}


def cell(value: object) -> str:
    if value is None:
        return "—"
    return str(value).replace("|", "\\|").replace("\n", " ")


def evidence_link(reference: object) -> str:
    if not reference:
        return "none retained"
    if not isinstance(reference, str):
        return "invalid evidence reference"
    value = reference
    if value.startswith(("https://", "http://", "#")):
        return "invalid external evidence reference"
    return f"[{value}](../{value})"


def is_iso_date(value: object) -> bool:
    if not isinstance(value, str) or not value:
        return False
    try:
        datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return False
    return True


def render_scorecard(data: object) -> str:
    if not isinstance(data, dict):
        return ""
    rows = data.get("rows", [])
    lines = [
        "# Coven reliability, recovery, and usefulness scorecard",
        "",
        "> Generated from [`reliability-scorecard.json`](reliability-scorecard.json).",
        "> Edit the structured source and run",
        "> `python3 scripts/check-reliability-scorecard.py --write`.",
        "",
        "This is the current decision view for #807. It distinguishes observed",
        "release or field evidence from historical observations, targets, benchmark",
        "conditions, and work that is not yet measured. A green test count is not",
        "an observed product reliability result.",
        "",
        "Rows dependent on installed-artifact certification or exact-commit release",
        "governance remain **Not yet measured** until #779 or #805 produces the named",
        "receipt. No target or SLO is implied by an empty target field.",
        "",
        "| Category | Metric | Status | Current result | Definition | Cohort / window | Source / privacy | Owner / confidence | Target / action |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            continue
        definition = (
            f"{row.get('definition', '')} "
            f"Numerator: {row.get('numerator', '')} "
            f"Denominator: {row.get('denominator', '')}"
        )
        cohort_window = f"{row.get('cohort', '')} Window: {row.get('window', '')}"
        source_privacy = (
            f"{row.get('source', '')} "
            f"Evidence: {evidence_link(row.get('evidenceRef'))}. "
            f"Privacy: {row.get('privacy', '')}"
        )
        owner_confidence = (
            f"{row.get('owner', '')}. Confidence: {row.get('confidence', '')}"
        )
        target = row.get("target")
        target_action = (
            f"Target: {'none adopted' if target is None else target}. "
            f"Action: {row.get('action', '')}"
        )
        current_result = row.get("value")
        if (
            row.get("status") in ("Observed current", "Historical observation")
            and row.get("observedAt")
        ):
            current_result = f"{current_result} (observed {row.get('observedAt')})"
        lines.append(
            "| "
            + " | ".join(
                cell(value)
                for value in (
                    CATEGORY_LABELS.get(
                        str(row.get("category")), str(row.get("category", ""))
                    ),
                    row.get("metric"),
                    row.get("status"),
                    current_result,
                    definition.strip(),
                    cohort_window.strip(),
                    source_privacy.strip(),
                    owner_confidence.strip(),
                    target_action.strip(),
                )
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "## Interpretation rules",
            "",
            "- **Observed current** and **Historical observation** require a dated,",
            "  retained evidence reference. This initial scorecard intentionally has",
            "  no such rows because no qualifying receipt or field cohort is retained.",
            "- **Benchmark condition/input** describes a deterministic fixture or",
            "  collector configuration only. It must never be quoted as achieved",
            "  product reliability or a field SLO.",
            "- **Not yet measured** is an explicit decision state with an owner and",
            "  next action; it is neither success nor failure.",
            "- Raw prompts, credentials, repository content, and full command output",
            "  are outside this scorecard contract.",
            "",
        ]
    )
    return "\n".join(lines)


def validate_scorecard(
    data: object,
    markdown: str,
    readme: str,
    root: pathlib.Path = ROOT,
) -> list[str]:
    if not isinstance(data, dict):
        return ["scorecard root must be a JSON object"]
    errors = []
    for field in sorted(data.keys() - TOP_LEVEL_FIELDS):
        errors.append(f"unexpected top-level field: {field}")
    if data.get("schemaVersion") != SCHEMA_VERSION:
        errors.append(f"scorecard schemaVersion must be {SCHEMA_VERSION}")
    if data.get("title") != SCORECARD_TITLE:
        errors.append("scorecard title must match the canonical title")
    if data.get("purpose") != SCORECARD_PURPOSE:
        errors.append("scorecard purpose must match the canonical purpose")

    rows = data.get("rows")
    if not isinstance(rows, list) or not rows:
        return errors + ["scorecard rows must be a non-empty array"]

    seen_ids = set()
    categories = set()
    for index, row in enumerate(rows):
        prefix = f"row {index + 1}"
        if not isinstance(row, dict):
            errors.append(f"{prefix} must be an object")
            continue

        normalized_keys = {
            re.sub(r"[^a-z0-9]", "", str(key).lower()) for key in row
        }
        forbidden = {
            key
            for key in normalized_keys
            if key in FORBIDDEN_FIELDS
            or key.endswith("commandoutput")
            or key.endswith("fulloutput")
        }
        if forbidden:
            errors.append(
                f"{prefix} contains privacy-sensitive field: {sorted(forbidden)[0]}"
            )
        for field in sorted(row.keys() - REQUIRED_FIELDS):
            errors.append(f"{prefix} unexpected row field: {field}")

        for field in sorted(REQUIRED_FIELDS - row.keys()):
            errors.append(f"{prefix} missing required field: {field}")
        if REQUIRED_FIELDS - row.keys():
            continue

        for field in sorted(NONEMPTY_TEXT_FIELDS):
            if not isinstance(row[field], str) or not row[field].strip():
                errors.append(f"{prefix} {field} must be a non-empty string")
        for field in ("value", "target"):
            if row[field] is not None and isinstance(row[field], (dict, list)):
                errors.append(f"{prefix} {field} must be null or a scalar")

        metric_id = row["id"]
        if not isinstance(metric_id, str) or not metric_id.strip():
            errors.append(f"{prefix} id must be a non-empty string")
        else:
            if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", metric_id):
                errors.append(f"{prefix} id must be a lowercase slug")
            if metric_id in seen_ids:
                errors.append(f"duplicate metric id: {metric_id}")
            seen_ids.add(metric_id)

        category = row["category"]
        if not isinstance(category, str) or not category.strip():
            errors.append(f"{prefix} category must be a non-empty string")
        else:
            categories.add(category)
            if category not in REQUIRED_CATEGORIES:
                errors.append(f"{prefix} has unsupported category: {category}")

        status = row["status"]
        if not isinstance(status, str) or not status.strip():
            errors.append(f"{prefix} status must be a non-empty string")
        elif status not in ALLOWED_STATUSES:
            errors.append(f"{prefix} has unsupported status: {status}")
        if not isinstance(row["owner"], str) or not row["owner"].strip():
            errors.append(f"{prefix} requires a named owner")
        elif row["owner"].strip().lower() in {"unassigned", "unknown", "tbd"}:
            errors.append(f"{prefix} requires a named owner")
        if not isinstance(row["action"], str) or not row["action"].strip():
            errors.append(f"{prefix} requires a non-empty action")

        if row["evidenceKind"] == "benchmark" and status != "Benchmark condition/input":
            errors.append(f"{prefix} benchmark evidence must use status Benchmark condition/input")
        if status == "Benchmark condition/input" and row["evidenceKind"] != "benchmark":
            errors.append(f"{prefix} benchmark status requires benchmark evidenceKind")
        if status == "Benchmark condition/input" and (
            not isinstance(row["evidenceRef"], str)
            or not row["evidenceRef"].strip()
            or row["evidenceRef"].startswith("#")
        ):
            errors.append(f"{prefix} benchmark row requires a retained evidenceRef")
        if status == "Not yet measured":
            if row["evidenceKind"] != "not_measured":
                errors.append(f"{prefix} not-yet-measured status requires not_measured evidenceKind")
            if row["value"] is not None:
                errors.append(f"{prefix} not-yet-measured row requires a null value")
        if status in ("Observed current", "Historical observation"):
            if row["evidenceKind"] not in {"release_receipt", "field_observation"}:
                errors.append(f"{prefix} observed row requires retained outcome evidence")
            if not is_iso_date(row["observedAt"]):
                errors.append(f"{prefix} observedAt must be a valid ISO date or datetime")
            if not isinstance(row["evidenceRef"], str) or not row[
                "evidenceRef"
            ].strip():
                errors.append(f"{prefix} observed row requires evidenceRef")
            elif row["evidenceRef"].startswith("#"):
                errors.append(f"{prefix} observed row requires a retained evidenceRef")
            if row["value"] is None or (
                isinstance(row["value"], str) and not row["value"].strip()
            ):
                errors.append(f"{prefix} observed row requires a value")
        elif row["observedAt"] is not None:
            errors.append(f"{prefix} observedAt must be null for status {status}")
        if status == "Target/SLO":
            if row["evidenceKind"] != "target":
                errors.append(f"{prefix} target status requires target evidenceKind")
            if row["target"] is None or (
                isinstance(row["target"], str) and not row["target"].strip()
            ):
                errors.append(f"{prefix} target status requires a target")
            if row["value"] is not None:
                errors.append(f"{prefix} target row requires a null current value")
        if status in ("Not yet measured", "Target/SLO") and row[
            "evidenceRef"
        ] is not None:
            errors.append(f"{prefix} evidenceRef must be null for status {status}")

        evidence_ref = row["evidenceRef"]
        if evidence_ref is not None and not isinstance(evidence_ref, str):
            errors.append(f"{prefix} evidenceRef must be null or a string")
        if isinstance(evidence_ref, str) and evidence_ref.startswith(
            ("https://", "http://", "#")
        ):
            errors.append(
                f"{prefix} evidenceRef must be a retained repository-local file"
            )
        if (
            isinstance(evidence_ref, str)
            and evidence_ref
            and not evidence_ref.startswith(("https://", "http://", "#"))
        ):
            resolved_root = root.resolve()
            resolved_ref = (root / evidence_ref).resolve()
            if (
                not resolved_ref.is_relative_to(resolved_root)
                or not resolved_ref.is_file()
            ):
                errors.append(
                    f"{prefix} evidenceRef does not resolve in the repository: "
                    f"{evidence_ref}"
                )

    for category in sorted(REQUIRED_CATEGORIES - categories):
        errors.append(f"missing required category: {category}")

    if markdown != render_scorecard(data):
        errors.append("docs/reliability-scorecard.md does not match the structured source")
    if README_LINK not in readme:
        errors.append("README.md must link docs/reliability-scorecard.md")
    return errors


def write_scorecard(
    data: object,
    readme: str,
    output_path: pathlib.Path = MARKDOWN_PATH,
    root: pathlib.Path = ROOT,
) -> list[str]:
    rendered = render_scorecard(data)
    errors = validate_scorecard(data, rendered, readme, root)
    if errors:
        return errors
    output_path.write_text(rendered, encoding="utf-8")
    return []


def load_data() -> object:
    return json.loads(DATA_PATH.read_text(encoding="utf-8"))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate or render the reliability scorecard.")
    parser.add_argument(
        "--write",
        action="store_true",
        help="Render docs/reliability-scorecard.md from the structured JSON source.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    data = load_data()
    readme = README_PATH.read_text(encoding="utf-8")
    if args.write:
        errors = write_scorecard(data, readme)
    else:
        errors = validate_scorecard(
            data,
            MARKDOWN_PATH.read_text(encoding="utf-8"),
            readme,
        )
    if not errors:
        return 0
    print("Reliability scorecard check failed:", file=sys.stderr)
    for error in errors:
        print(f"- {error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
