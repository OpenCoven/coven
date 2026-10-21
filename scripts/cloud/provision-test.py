#!/usr/bin/env python3
"""Integrity and extraction checks use local synthetic artifacts, no network."""
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

import provision


class ProvisionTests(unittest.TestCase):
    def test_checksum_requires_one_exact_filename_and_matching_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "node.tar.gz"
            artifact.write_bytes(b"synthetic archive")
            digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
            provision.check_checksum(artifact, f"{digest}  node.tar.gz\n", "node.tar.gz")
            for manifest in (f"{digest}  other.tar.gz\n", f"{'0' * 64}  node.tar.gz\n",
                             f"{digest}  node.tar.gz\n{digest}  node.tar.gz\n"):
                with self.subTest(manifest=manifest), self.assertRaises(provision.ProvisionError):
                    provision.check_checksum(artifact, manifest, "node.tar.gz")

    def test_rustup_official_checksum_filename_format(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "rustup-init"
            artifact.write_bytes(b"synthetic rustup executable")
            digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
            provision.check_checksum(artifact, f"{digest} *./rustup-init\n", "rustup-init")

    def test_archive_cannot_escape_staging_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "archive.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                member = tarfile.TarInfo("../escaped")
                member.size = 1
                bundle.addfile(member, io.BytesIO(b"x"))
            with self.assertRaises(provision.ProvisionError):
                provision.extract_node(archive, root / "stage", "node-v1.2.3-linux-x64")
            self.assertFalse((root / "escaped").exists())

    def test_archive_preserves_safe_node_package_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "archive.tar.gz"
            prefix = "node-v1.2.3-linux-x64"
            with tarfile.open(archive, "w:gz") as bundle:
                member = tarfile.TarInfo(prefix + "/lib/npm.js")
                member.size = 1
                bundle.addfile(member, io.BytesIO(b"x"))
                link = tarfile.TarInfo(prefix + "/bin/npm")
                link.type = tarfile.SYMTYPE
                link.linkname = "../lib/npm.js"
                bundle.addfile(link)
            extracted = provision.extract_node(archive, root / "stage", prefix)
            self.assertEqual((extracted / "bin/npm").read_bytes(), b"x")


if __name__ == "__main__":
    unittest.main()
