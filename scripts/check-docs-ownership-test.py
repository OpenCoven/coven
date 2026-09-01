from __future__ import annotations

import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).with_name("check-docs-ownership.py")
SPEC = importlib.util.spec_from_file_location("check_docs_ownership", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class DocsOwnershipTests(unittest.TestCase):
    def test_short_canonical_pointer_is_allowed(self) -> None:
        content = """---
title: Install
description: Pointer to canonical install guidance.
---

Canonical install guidance: **https://docs.opencoven.ai/docs/guide/install**
"""
        self.assertEqual(module.validate_page("docs/install/index.md", content), [])

    def test_source_adjacent_page_requires_a_reason(self) -> None:
        content = """---
title: Socket API
source_adjacent_reason: Tracks the daemon API implemented in this repository.
---

Normative contract.
"""
        self.assertEqual(module.validate_page("docs/daemon/socket-api.md", content), [])

    def test_long_public_guidance_without_ownership_fails(self) -> None:
        content = "---\ntitle: Install\n---\n\n" + "\n".join(["Duplicate guidance."] * 30)
        errors = module.validate_page("docs/install/new-guide.md", content)
        self.assertEqual(len(errors), 1)
        self.assertIn("source_adjacent_reason", errors[0])

    def test_empty_source_adjacent_reason_fails(self) -> None:
        content = """---
title: Socket API
source_adjacent_reason:
---

Normative contract.
"""
        errors = module.validate_page("docs/daemon/socket-api.md", content)
        self.assertEqual(len(errors), 1)

    def test_non_public_docs_are_ignored(self) -> None:
        self.assertEqual(
            module.validate_page("docs/development/source-map.md", "Maintainer detail."),
            [],
        )


if __name__ == "__main__":
    unittest.main()
