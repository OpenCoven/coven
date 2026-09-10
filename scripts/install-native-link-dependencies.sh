#!/usr/bin/env bash
set -euo pipefail

apt_etc_dir="${COVEN_APT_ETC_DIR:-/etc/apt}"
workdir="$(mktemp -d)"
apt_started=0
cleanup() {
  if ((apt_started)); then
    sudo rm -rf -- "$workdir"
  else
    rm -rf -- "$workdir"
  fi
}
trap cleanup EXIT
# Apt's unprivileged downloader must be able to traverse the public metadata directory.
chmod 755 "$workdir"

sourceparts="$workdir/sources.list.d"
lists="$workdir/lists"
mkdir -p "$sourceparts" "$lists/partial"

python3 - "$apt_etc_dir" "$sourceparts" <<'PY'
import pathlib
import re
import sys

apt_etc_dir, sourceparts = map(pathlib.Path, sys.argv[1:])
archive_uri = re.compile(r"https?://(?:[a-z0-9-]+\.)*ubuntu\.com/ubuntu/?", re.IGNORECASE)


def ubuntu_uri(uri):
    if uri.startswith("mirror+file:"):
        mirror_file = pathlib.Path(uri.removeprefix("mirror+file:"))
        if not mirror_file.is_absolute():
            return False
        mirrors = [
            line.split("#", 1)[0].split()
            for line in mirror_file.read_text(encoding="utf-8").splitlines()
        ]
        urls = [fields[0] for fields in mirrors if fields]
        return bool(urls) and all(archive_uri.fullmatch(url) for url in urls)
    return archive_uri.fullmatch(uri) is not None


def validate_deb822(text):
    # Validate each stanza and every URI, not merely one match in the whole file.
    stanzas = []
    fields = {}
    current = None
    for line in text.splitlines() + [""]:
        if line.lstrip().startswith("#"):
            continue
        if not line.strip():
            if fields:
                stanzas.append(fields)
            fields, current = {}, None
        elif line[0].isspace():
            if current is None:
                raise ValueError("Ubuntu apt source has an orphan continuation line")
            fields[current] += " " + line.strip()
        else:
            name, separator, value = line.partition(":")
            name = name.lower()
            if not separator or not re.fullmatch(r"[a-z][a-z-]*", name) or name in fields:
                raise ValueError("Ubuntu apt source has malformed or duplicate fields")
            fields[name], current = value.strip(), name
    if not stanzas:
        raise ValueError("Ubuntu apt source has no package stanzas")
    for stanza in stanzas:
        if "deb" not in stanza.get("types", "").split():
            raise ValueError("Ubuntu apt source does not declare deb package sources")
        uris = stanza.get("uris", "").split()
        if not uris or not all(ubuntu_uri(uri) for uri in uris):
            raise ValueError("Ubuntu apt source contains a non-Ubuntu distribution archive")
        if not stanza.get("signed-by"):
            raise ValueError("Ubuntu apt source does not declare Signed-By key material")


try:
    deb822 = apt_etc_dir / "sources.list.d/ubuntu.sources"
    legacy = apt_etc_dir / "sources.list"
    if deb822.exists():
        text = deb822.read_text(encoding="utf-8")
        validate_deb822(text)
        (sourceparts / "ubuntu.sources").write_text(text, encoding="utf-8")
    elif legacy.exists():
        selected = []
        for line in legacy.read_text(encoding="utf-8").splitlines():
            match = re.match(r"^\s*deb\s+(?:\[[^\]\n]*\]\s+)?(\S+)\s+", line)
            if match and archive_uri.fullmatch(match[1]):
                selected.append(line)
        if not selected:
            raise ValueError("Ubuntu apt source has no deb entries for an Ubuntu distribution archive")
        (sourceparts / "ubuntu.list").write_text("\n".join(selected) + "\n", encoding="utf-8")
    else:
        raise ValueError(f"Unable to find the Ubuntu apt distribution source in {apt_etc_dir}")
except (OSError, ValueError) as error:
    print(f"::error::{error}", file=sys.stderr)
    sys.exit(1)
PY

apt_options=(
  -o "Dir::Etc::sourcelist=/dev/null"
  -o "Dir::Etc::sourceparts=$sourceparts"
  -o "Dir::State::lists=$lists"
  -o "APT::Get::List-Cleanup=0"
)

if (($#)); then
  packages=("$@")
else
  packages=(libopenblas-dev)
fi

apt_started=1
sudo apt-get "${apt_options[@]}" update
sudo apt-get "${apt_options[@]}" install -y --no-install-recommends -- "${packages[@]}"
