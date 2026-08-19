#!/usr/bin/env bash
set -euo pipefail

ACTIONLINT_VERSION=1.7.12
case "${1-}" in
  --print-version)
    printf '%s\n' "$ACTIONLINT_VERSION"
    exit 0
    ;;
esac

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) archive_name="actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz"; checksum=$(printf '%s  %s\n' 8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8 "$archive_name") ;;
  Darwin-arm64) archive_name="actionlint_${ACTIONLINT_VERSION}_darwin_arm64.tar.gz"; checksum=$(printf '%s  %s\n' aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f "$archive_name") ;;
  Darwin-x86_64) archive_name="actionlint_${ACTIONLINT_VERSION}_darwin_amd64.tar.gz"; checksum=$(printf '%s  %s\n' 5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644 "$archive_name") ;;
  *) echo "unsupported platform" >&2; exit 1 ;;
esac

archive_url="https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/${archive_name}"
workdir="$(mktemp -d)"
cleanup() { rm -rf "$workdir"; }
trap cleanup EXIT

curl -fsSL "$archive_url" -o "$workdir/$archive_name"
( cd "$workdir" && printf '%s\n' "$checksum" | shasum -a 256 --check - )
 tar -xzf "$workdir/$archive_name" -C "$workdir" actionlint
"$workdir/actionlint" -color
