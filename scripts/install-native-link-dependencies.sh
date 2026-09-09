#!/usr/bin/env bash
set -euo pipefail

apt_etc_dir="${COVEN_APT_ETC_DIR:-/etc/apt}"
source_file=""
workdir="$(mktemp -d)"
cleanup() { rm -rf "$workdir"; }
trap cleanup EXIT
# Apt's unprivileged downloader must be able to traverse the public metadata directory.
chmod 755 "$workdir"

ubuntu_archive_pattern='ubuntu\.com/ubuntu/?([[:space:]]|$)'

deb822_source_points_to_ubuntu_archive() {
  local file="$1"
  if grep -Eq "^[[:space:]]*URIs:[[:space:]].*$ubuntu_archive_pattern" "$file"; then
    return 0
  fi

  local uri mirror_file
  while IFS= read -r uri; do
    mirror_file="${uri#mirror+file:}"
    if [[ -r "$mirror_file" ]] && grep -Eq "$ubuntu_archive_pattern" "$mirror_file"; then
      return 0
    fi
  done < <(grep -Eho 'mirror\+file:[^[:space:]]+' "$file" || true)

  return 1
}

sourceparts="$workdir/sources.list.d"
lists="$workdir/lists"
mkdir -p "$sourceparts" "$lists/partial"

if [[ -r "$apt_etc_dir/sources.list.d/ubuntu.sources" ]]; then
  source_file="$apt_etc_dir/sources.list.d/ubuntu.sources"
  if ! grep -Eq '^[[:space:]]*Types:[[:space:]]*deb([[:space:]]|$)' "$source_file"; then
    echo "::error::Ubuntu apt source $source_file does not declare deb package sources." >&2
    exit 1
  fi
  if ! deb822_source_points_to_ubuntu_archive "$source_file"; then
    echo "::error::Ubuntu apt source $source_file does not point at an Ubuntu distribution archive." >&2
    exit 1
  fi
  if ! grep -Eq '^[[:space:]]*Signed-By:[[:space:]]*[^[:space:]]+' "$source_file"; then
    echo "::error::Ubuntu apt source $source_file does not declare Signed-By key material." >&2
    exit 1
  fi
  cp "$source_file" "$sourceparts/ubuntu.sources"
elif [[ -r "$apt_etc_dir/sources.list" ]]; then
  source_file="$apt_etc_dir/sources.list"
  awk '
    /^[[:space:]]*deb([[:space:]]|$)/ && /ubuntu\.com\/ubuntu\/?([[:space:]]|$)/ { print }
  ' "$source_file" > "$sourceparts/ubuntu.list"
  if [[ ! -s "$sourceparts/ubuntu.list" ]]; then
    echo "::error::Ubuntu apt source $source_file does not contain deb entries for an Ubuntu distribution archive." >&2
    exit 1
  fi
else
  echo "::error::Unable to find the Ubuntu apt distribution source in $apt_etc_dir." >&2
  exit 1
fi

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

sudo apt-get "${apt_options[@]}" update
sudo apt-get "${apt_options[@]}" install -y --no-install-recommends -- "${packages[@]}"
