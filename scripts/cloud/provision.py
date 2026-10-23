"""Integrity checks for official tool artifacts; no network-policy fallback."""
import hashlib
from pathlib import Path, PurePosixPath
import re
import tarfile


class ProvisionError(ValueError):
    """Fixed non-secret installation diagnostic."""


def check_checksum(artifact, manifest, filename):
    matches = []
    for line in manifest.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[1].removeprefix("*").removeprefix("./") == filename:
            matches.append(fields[0])
    if len(matches) != 1 or not re.fullmatch(r"[0-9a-fA-F]{64}", matches[0]):
        raise ProvisionError("official checksum manifest has no unique artifact entry")
    digest = hashlib.sha256()
    with Path(artifact).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    if digest.hexdigest() != matches[0].lower():
        raise ProvisionError("downloaded artifact does not match its official checksum")


def extract_node(archive, destination, prefix):
    destination = Path(destination)
    if not hasattr(tarfile, "data_filter"):
        raise ProvisionError("Python needs the maintained tarfile extraction security filters")
    try:
        with tarfile.open(archive, "r:gz") as bundle:
            members = bundle.getmembers()
            if not members:
                raise ProvisionError("Node archive is empty")
            for member in members:
                parts = PurePosixPath(member.name).parts
                if not parts or parts[0] != prefix or ".." in parts:
                    raise ProvisionError("Node archive contains an unexpected extraction path")
            destination.mkdir(parents=True, exist_ok=False)
            bundle.extractall(destination, members=members, filter="data")
        return destination / prefix
    except (tarfile.TarError, OSError):
        raise ProvisionError("Node archive could not be safely extracted") from None
