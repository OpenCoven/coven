#!/usr/bin/env bash
set -euo pipefail

apt_etc_dir="${COVEN_APT_ETC_DIR:-/etc/apt}"
# Download bound (#1578): each attempt may take a grace period plus the time
# the package set needs at a minimum acceptable rate. A slower transfer is
# stopped and retried with the next mirror first; apt resumes partial files.
download_attempts="${COVEN_APT_DOWNLOAD_ATTEMPTS:-3}"
download_grace_seconds="${COVEN_APT_DOWNLOAD_GRACE_SECONDS:-15}"
download_min_bytes_per_second="${COVEN_APT_DOWNLOAD_MIN_BYTES_PER_SECOND:-1048576}"
for bound in "$download_attempts" "$download_grace_seconds" "$download_min_bytes_per_second"; do
  if [[ ! "$bound" =~ ^[1-9][0-9]{0,8}$ ]]; then
    echo "::error::Native dependency download bounds must be positive integers" >&2
    exit 1
  fi
done
workdir="$(mktemp -d "${TMPDIR:-/tmp}/coven-native-link.XXXXXXXXXX")"
apt_started=0
use_sudo=0
run_privileged() {
  if ((use_sudo)); then
    sudo -n "$@"
  else
    "$@"
  fi
}
cleanup() {
  if ((apt_started)); then
    run_privileged rm -rf -- "$workdir"
  else
    rm -rf -- "$workdir"
  fi
}
trap cleanup EXIT
# Apt's unprivileged downloader must be able to traverse the public metadata directory.
chmod 755 "$workdir"

sourceparts="$workdir/sources.list.d"
lists="$workdir/lists"
mirrors="$workdir/mirrors"
mirror_plans="$workdir/mirror-plans"
mkdir -p "$sourceparts" "$lists/partial" "$mirrors" "$mirror_plans"

python3 - "$apt_etc_dir" "$sourceparts" "${COVEN_OS_RELEASE_FILE:-/etc/os-release}" \
  "$mirrors" "$mirror_plans" "$download_attempts" <<'PY'
import pathlib
import re
import sys
import shlex

apt_etc_dir, sourceparts, os_release, mirrors, mirror_plans = map(pathlib.Path, sys.argv[1:6])
download_attempts = int(sys.argv[6])
mirror_lists = {}
archives = {
    "ubuntu": re.compile(r"https?://(?:[a-z0-9-]+\.)*ubuntu\.com/ubuntu(?:-ports)?/?", re.IGNORECASE),
    "debian": re.compile(r"https?://(?:[a-z0-9-]+\.)*debian\.org/debian(?:-security)?/?", re.IGNORECASE),
}


def host_distribution():
    # Parse data, never source shell code from the fixture or the host file.
    fields = {}
    for line in os_release.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        name, separator, value = line.partition("=")
        if not separator or not re.fullmatch(r"[A-Z_][A-Z_0-9]*", name) or name in fields:
            raise ValueError("Malformed os-release metadata")
        words = shlex.split(value)
        if len(words) != 1:
            raise ValueError("Malformed os-release value")
        fields[name] = words[0]
    distro = fields.get("ID")
    if distro not in archives:
        raise ValueError("Unsupported OS: native dependencies require Debian or Ubuntu")
    codename = fields.get("VERSION_CODENAME", "")
    if not re.fullmatch(r"[a-z0-9][a-z0-9._-]*", codename):
        raise ValueError("Missing or invalid VERSION_CODENAME in os-release")
    return distro, codename


def validate_options(options):
    # apt-secure(8): these source options override authentication defaults.
    for name in ("trusted", "allow-insecure", "allow-weak", "allow-downgrade-to-insecure"):
        if name in options and options[name].lower() not in ("no", "false", "0"):
            raise ValueError("APT source requests an authentication bypass")
    for name in ("check-valid-until", "check-date"):
        if name in options and options[name].lower() not in ("yes", "true", "1"):
            raise ValueError("APT source disables release validity checks")


def validate_suites(suites):
    allowed = {codename, *(codename + suffix for suffix in ("-updates", "-security", "-backports"))}
    if not suites or not set(suites) <= allowed:
        raise ValueError("APT source suite does not match host VERSION_CODENAME")


def distribution_uri(uri):
    if uri.startswith("mirror+file:"):
        mirror_file = pathlib.Path(uri.removeprefix("mirror+file:"))
        if not mirror_file.is_absolute():
            return False
        entries = [
            fields for fields in (
                line.split("#", 1)[0].split()
                for line in mirror_file.read_text(encoding="utf-8").splitlines()
            ) if fields
        ]
        if not entries or not all(archive_uri.fullmatch(fields[0]) for fields in entries):
            return False
        mirror_lists.setdefault(str(mirror_file), entries)
        return True
    return archive_uri.fullmatch(uri) is not None


def private_mirror_lists(text):
    """Point validated mirror+file sources at private lists whose order can rotate.

    Each download attempt k puts mirror k first. Index and archive hashes are
    still checked against the signed Release file, whichever mirror serves them.
    A temporary root that is not URI-safe keeps the host list: retries stay
    bounded but do not rotate.
    """
    if not re.fullmatch(r"[A-Za-z0-9._/+-]+", str(mirrors)):
        return text
    names = {path: f"{index}.list" for index, path in enumerate(mirror_lists)}
    for attempt in range(download_attempts):
        plan = mirror_plans / str(attempt)
        plan.mkdir()
        for path, entries in mirror_lists.items():
            first = attempt % len(entries)
            lines = []
            for rank, (url, *metadata) in enumerate(entries[first:] + entries[:first], 1):
                metadata = [item for item in metadata if not item.startswith("priority:")]
                lines.append("\t".join([url, " ".join([f"priority:{rank}", *metadata])]))
            (plan / names[path]).write_text("\n".join(lines) + "\n", encoding="utf-8")
    for name in names.values():
        (mirrors / name).write_bytes((mirror_plans / "0" / name).read_bytes())
    return re.sub(
        r"mirror\+file:(\S+)",
        lambda match: f"mirror+file:{mirrors / names[match.group(1)]}"
        if match.group(1) in names else match.group(0),
        text,
    )


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
                raise ValueError(f"{distro.title()} apt source has an orphan continuation line")
            fields[current] += " " + line.strip()
        else:
            name, separator, value = line.partition(":")
            name = name.lower()
            if not separator or not re.fullmatch(r"[a-z][a-z-]*", name) or name in fields:
                raise ValueError(f"{distro.title()} apt source has malformed or duplicate fields")
            fields[name], current = value.strip(), name
    if not stanzas:
        raise ValueError(f"{distro.title()} apt source has no package stanzas")
    for stanza in stanzas:
        if "deb" not in stanza.get("types", "").split():
            raise ValueError(f"{distro.title()} apt source does not declare deb package sources")
        validate_options(stanza)
        validate_suites(stanza.get("suites", "").split())
        if not stanza.get("components", "").split():
            raise ValueError("APT source has no components")
        uris = stanza.get("uris", "").split()
        if not uris or not all(distribution_uri(uri) for uri in uris):
            raise ValueError(f"{distro.title()} apt source contains a foreign distribution archive")
        if not stanza.get("signed-by"):
            raise ValueError(f"{distro.title()} apt source does not declare Signed-By key material")


try:
    distro, codename = host_distribution()
    archive_uri = archives[distro]
    deb822 = apt_etc_dir / f"sources.list.d/{distro}.sources"
    legacy = apt_etc_dir / "sources.list"
    if deb822.exists():
        text = deb822.read_text(encoding="utf-8")
        validate_deb822(text)
        (sourceparts / f"{distro}.sources").write_text(private_mirror_lists(text), encoding="utf-8")
    elif legacy.exists():
        selected = []
        for line in legacy.read_text(encoding="utf-8").splitlines():
            line = line.split("#", 1)[0].strip()
            if not line or line.startswith("deb-src "):
                continue
            match = re.fullmatch(r"deb\s+(?:\[([^\]\n]*)\]\s+)?(\S+)\s+(\S+)\s+(.+)", line)
            if not match:
                raise ValueError("Malformed legacy APT source")
            options_text, uri, suite, components = match.groups()
            if any(pattern.fullmatch(uri) for name, pattern in archives.items() if name != distro):
                raise ValueError("APT source contains a foreign distribution archive")
            if not distribution_uri(uri):
                continue  # Do not refresh unrelated third-party package indexes (#971).
            options = {}
            for option in (options_text or "").split():
                name, separator, value = option.partition("=")
                # APT unquotes and percent-decodes option names. Accept only
                # canonical names so that validation and APT interpret them alike.
                if (not separator or not value or name in options
                        or not re.fullmatch(r"[a-z][a-z-]*[+-]?", name)):
                    raise ValueError("Malformed legacy APT source option")
                options[name] = value
            validate_options(options)
            validate_suites([suite])
            selected.append(line)
        if not selected:
            raise ValueError(f"{distro.title()} apt source has no deb entries for the host distribution archive")
        text = "\n".join(selected) + "\n"
        (sourceparts / f"{distro}.list").write_text(private_mirror_lists(text), encoding="utf-8")
    else:
        raise ValueError(f"Unable to find the {distro.title()} apt distribution source")
except (OSError, ValueError) as error:
    # Avoid echoing source contents/URIs, which may contain credentials.
    diagnostic = str(error) if isinstance(error, ValueError) else "Unable to read APT or OS metadata"
    print(f"::error::{diagnostic}", file=sys.stderr)
    sys.exit(1)
PY

# Never prompt in a managed setup task. Root needs no sudo installation.
if [[ "$(id -u)" != 0 ]]; then
  if ! command -v sudo >/dev/null 2>&1 || ! sudo -n true 2>/dev/null; then
    echo "::error::Native dependency privilege blocker: run in a root setup phase or grant noninteractive sudo." >&2
    exit 1
  fi
  use_sudo=1
fi

apt_options=(
  -o "Dir::Etc::sourcelist=/dev/null"
  -o "Dir::Etc::sourceparts=$sourceparts"
  -o "Dir::State::lists=$lists"
  -o "APT::Get::List-Cleanup=0"
  -o "APT::Get::AllowUnauthenticated=false"
  -o "Acquire::AllowInsecureRepositories=false"
  -o "Acquire::AllowWeakRepositories=false"
  -o "Acquire::AllowDowngradeToInsecureRepositories=false"
  -o "Acquire::Check-Valid-Until=true"
  -o "Acquire::Check-Date=true"
)

if (($#)); then
  packages=("$@")
else
  packages=(libopenblas-dev)
fi

install_options=(install -y --no-install-recommends)

apt_started=1
run_privileged apt-get "${apt_options[@]}" update

# Size the bound from the signed index; files already in the archive cache are
# not listed. Field 3 of each --print-uris line is the archive size in bytes.
download_bytes="$(
  run_privileged apt-get "${apt_options[@]}" "${install_options[@]}" --print-uris -- "${packages[@]}" |
    awk 'index($1, "\047") == 1 && $3 ~ /^[0-9]+$/ { total += $3 } END { printf "%d\n", total }'
)"
download_seconds=$((download_grace_seconds + download_bytes / download_min_bytes_per_second))

for ((attempt = 0; ; attempt++)); do
  if ((attempt)) && compgen -G "$mirror_plans/$attempt/*" >/dev/null; then
    cp -- "$mirror_plans/$attempt/"* "$mirrors/"
  fi
  status=0
  # timeout(1) signals apt's whole process group; apt keeps partial files.
  run_privileged timeout --kill-after=10 "$download_seconds" \
    apt-get "${apt_options[@]}" "${install_options[@]}" --download-only -- "${packages[@]}" || status=$?
  if ((status == 0)); then
    break
  fi
  if ((status != 124 && status != 137)); then
    exit "$status"
  fi
  if ((attempt + 1 >= download_attempts)); then
    echo "::error::Native dependency download did not finish within the bounded retry budget" >&2
    exit 1
  fi
  echo "::warning::Native dependency download exceeded ${download_seconds}s (attempt $((attempt + 1)) of $download_attempts); retrying with the next mirror first" >&2
done

# Install only the downloaded archives. apt verifies each one against the
# signed index and never falls back to another unbounded download.
run_privileged apt-get "${apt_options[@]}" "${install_options[@]}" --no-download -- "${packages[@]}"
