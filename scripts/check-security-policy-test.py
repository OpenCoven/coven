#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = pathlib.Path(__file__).with_name("check-security-policy.py")


def load_checker():
    if not SCRIPT.is_file():
        return None
    spec = importlib.util.spec_from_file_location("check_security_policy", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


module = load_checker()


class SecurityPolicyTests(unittest.TestCase):
    def checker(self):
        self.assertIsNotNone(module, "check-security-policy.py is missing")
        return module

    def canonical_policy(self) -> str:
        return (ROOT / "SECURITY.md").read_text(encoding="utf-8")

    def canonical_readme(self) -> str:
        return (ROOT / "README.md").read_text(encoding="utf-8")

    def validate(
        self,
        policy: str,
        *,
        readme: str | None = None,
        root: pathlib.Path = ROOT,
    ) -> list[str]:
        checker = self.checker()
        return checker.validate_policy(
            policy,
            self.canonical_readme() if readme is None else readme,
            root,
        )

    def test_current_security_policy_is_valid(self) -> None:
        self.assertEqual(self.validate(self.canonical_policy()), [])

    def test_duplicate_security_policy_heading_fails(self) -> None:
        policy = self.canonical_policy() + "\n## Security Policy\n"
        errors = self.validate(policy)
        self.assertTrue(
            any("exactly one '# Security Policy' heading" in error for error in errors)
        )

    def test_retired_scope_and_personal_reporting_language_fail(self) -> None:
        policy = self.canonical_policy().replace(
            "## Policy maintenance",
            "## OpenCoven Security Disclosure Addendum\n\n"
            "- OpenTrust memory and session substrate\n"
            "- Discord: https://discord.gg/example (DM @Example)\n\n"
            "## Policy maintenance",
        )
        errors = self.validate(policy)
        self.assertTrue(any("retired scope language" in error for error in errors))
        self.assertTrue(any("personal reporting channel" in error for error in errors))

    def test_unsupported_response_deadline_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "Coven deliberately publishes **no acknowledgment or remediation deadline**.",
            "We will acknowledge receipt within 48 hours.",
        )
        errors = self.validate(policy)
        self.assertTrue(any("response-time commitment" in error for error in errors))

    def test_generic_response_deadline_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "Coven deliberately publishes **no acknowledgment or remediation deadline**.",
            "We will respond within 48 hours.",
        )
        errors = self.validate(policy)
        self.assertTrue(any("response-time commitment" in error for error in errors))

    def test_wrapped_response_deadline_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "Coven deliberately publishes **no acknowledgment or remediation deadline**.",
            "We will respond\nwithin 48 hours.",
        )
        errors = self.validate(policy)
        self.assertTrue(any("response-time commitment" in error for error in errors))

    def test_equivalent_addendum_scope_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "## Policy maintenance",
            "This repository inherits the organization-wide OpenCoven security addendum.\n\n"
            "## Policy maintenance",
        )
        errors = self.validate(policy)
        self.assertTrue(any("retired scope language" in error for error in errors))

    def test_inherited_organization_wide_addendum_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "## Policy maintenance",
            "This repository inherits the organization-wide security addendum.\n\n"
            "## Policy maintenance",
        )
        errors = self.validate(policy)
        self.assertTrue(any("retired scope language" in error for error in errors))

    def test_discord_user_dm_link_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "## Policy maintenance",
            "Discord DM: https://discord.com/users/123456789012345678\n\n"
            "## Policy maintenance",
        )
        errors = self.validate(policy)
        self.assertTrue(any("personal reporting channel" in error for error in errors))

    def test_markdown_discord_user_link_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "## Policy maintenance",
            "[Contact a maintainer](https://discord.com/users/123456789012345678)\n\n"
            "## Policy maintenance",
        )
        errors = self.validate(policy)
        self.assertTrue(any("personal reporting channel" in error for error in errors))

    def test_policy_cannot_broaden_to_all_opencoven_repositories(self) -> None:
        policy = self.canonical_policy().replace(
            "> Scope note: this policy covers Coven the runtime/daemon/CLI and the code in\n"
            "> this repository.",
            "> Scope note: this policy covers Coven and every OpenCoven repository.",
        )
        errors = self.validate(policy)
        self.assertTrue(any("must remain scoped to Coven" in error for error in errors))

    def test_scope_note_cannot_add_the_rest_of_the_organization(self) -> None:
        policy = self.canonical_policy().replace(
            "> this repository.",
            "> this repository, plus the rest of the OpenCoven organization.",
            1,
        )
        errors = self.validate(policy)
        self.assertTrue(any("must remain scoped to Coven" in error for error in errors))

    def test_primary_advisory_intake_is_required(self) -> None:
        policy = self.canonical_policy().replace(
            "https://github.com/OpenCoven/coven/security/advisories/new",
            "https://github.com/OpenCoven/coven/issues/new",
        )
        errors = self.validate(policy)
        self.assertTrue(any("private advisory intake" in error for error in errors))

    def test_public_issues_cannot_become_the_primary_reporting_path(self) -> None:
        policy = self.canonical_policy().replace(
            "**Do not open a public GitHub issue for security vulnerabilities.**",
            "**Open a public GitHub issue first for security vulnerabilities.**",
        )
        errors = self.validate(policy)
        self.assertTrue(any("must forbid public vulnerability reports" in error for error in errors))

    def test_required_policy_sections_are_enforced(self) -> None:
        policy = self.canonical_policy().replace(
            "## 3. Residual risk and safe configuration",
            "## 3. Operational notes",
        )
        errors = self.validate(policy)
        self.assertTrue(any("required section" in error for error in errors))

    def test_broken_relative_policy_link_fails(self) -> None:
        policy = self.canonical_policy().replace(
            "docs/API-CONTRACT.md",
            "docs/DOES-NOT-EXIST.md",
            1,
        )
        errors = self.validate(policy)
        self.assertTrue(any("relative link does not resolve" in error for error in errors))

    def test_readme_must_link_policy_and_private_advisories(self) -> None:
        readme = self.canonical_readme().replace("(SECURITY.md)", "(README.md)")
        readme = readme.replace(
            "https://github.com/OpenCoven/coven/security/advisories",
            "https://github.com/OpenCoven/coven/issues",
        )
        errors = self.validate(self.canonical_policy(), readme=readme)
        self.assertTrue(any("README.md must link to SECURITY.md" in error for error in errors))
        self.assertTrue(
            any("README.md must link to private advisories" in error for error in errors)
        )

    def test_ci_runs_security_policy_test_and_checker(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("python3 scripts/check-security-policy-test.py", workflow)
        self.assertIn("python3 scripts/check-security-policy.py", workflow)


if __name__ == "__main__":
    unittest.main()
