#!/usr/bin/env bash
# Produce the v0.4.1 three-harness certification packet (coven-v041-cert).
#
# Run from the repo root, in a real terminal:   ./certify-v0.4.1.sh
#
# Needs a TTY: `coven setup` asks for explicit network/cost consent and runs one
# bounded real turn per provider. It exits nonzero and writes nothing when piped.
# This script is untracked scratch -- delete it when the packet is filed.

set -euo pipefail

EXPECTED_TAG="v0.4.1"
OUT_DIR="${OUT_DIR:-$HOME/coven-cert-v0.4.1}"

say()  { printf '\n\033[1m== %s\033[0m\n' "$*"; }
ok()   { printf '   \033[32mok\033[0m   %s\n' "$*"; }
bad()  { printf '   \033[31mFAIL\033[0m %s\n' "$*" >&2; }
die()  { bad "$*"; exit 1; }

# Read one field from a report without requiring jq.
field() { # field <file> <key>
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2],""))' "$1" "$2"
}

# ---------------------------------------------------------------------------
say "1/5  repo root and expected commit"
# ---------------------------------------------------------------------------
[ -d .git ] || die "run this from the repo root (no .git here)"
git rev-parse --verify "$EXPECTED_TAG" >/dev/null 2>&1 \
  || die "tag $EXPECTED_TAG not found locally -- run: git fetch origin --tags"
EXPECTED_SHA="$(git rev-list -n 1 "$EXPECTED_TAG")"
ok "$EXPECTED_TAG -> $EXPECTED_SHA"

# ---------------------------------------------------------------------------
say "2/5  the coven on PATH is the released build"
# ---------------------------------------------------------------------------
# Multiple global installs are common and are exactly how the wrong build gets
# certified. Show every candidate with its version, then require either a single
# one on PATH or an explicit COVEN_BIN.
PATHS="$(type -aP coven 2>/dev/null | sort -u || true)"
if [ -n "$PATHS" ]; then
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    printf '        %-56s %s\n' "$p" "$("$p" --version 2>/dev/null | head -1 || echo '(failed)')"
  done <<< "$PATHS"
fi

if [ -n "${COVEN_BIN:-}" ]; then
  [ -x "$COVEN_BIN" ] || die "COVEN_BIN is not executable: $COVEN_BIN"
  COVEN="$COVEN_BIN"
  ok "using explicit COVEN_BIN=$COVEN"
else
  COUNT="$(printf '%s\n' "$PATHS" | grep -c . || true)"
  [ "$COUNT" -ge 1 ] || die "coven is not on PATH -- npm install -g @opencoven/cli@0.4.1"
  if [ "$COUNT" -ne 1 ]; then
    bad "$COUNT coven binaries on PATH -- certifying the wrong one is silent and undetectable later"
    die "re-run pinned to the v0.4.1 one, e.g.:  COVEN_BIN=/path/to/coven ./certify-v0.4.1.sh"
  fi
  COVEN="$(printf '%s\n' "$PATHS" | head -1)"
  ok "exactly one coven on PATH"
fi

VERSION_LINE="$("$COVEN" --version)"
printf '        %s\n' "$VERSION_LINE"
case "$VERSION_LINE" in
  *"v0.4.1"*) ok "version is v0.4.1" ;;
  *) die "expected v0.4.1, got: $VERSION_LINE  (npm install -g @opencoven/cli@0.4.1)" ;;
esac

# ---------------------------------------------------------------------------
say "3/5  output directory"
# ---------------------------------------------------------------------------
# Coven publishes reports atomically and fail-if-exists, and does NOT create
# parent directories. Both are why this step exists.
mkdir -p "$OUT_DIR"
for h in codex claude copilot; do
  [ -e "$OUT_DIR/$h.json" ] && die "$OUT_DIR/$h.json already exists -- move it aside; coven will not overwrite"
done
ok "$OUT_DIR is ready and empty of reports"

# ---------------------------------------------------------------------------
say "4/5  certify each harness (real provider turns, consent prompts follow)"
# ---------------------------------------------------------------------------
# Codex first, then verify the baked commit before spending the other two turns.
FAILED=""
for h in codex claude copilot; do
  printf '\n   --- %s setup %s --verify-only --report-json %s/%s.json\n\n' "$COVEN" "$h" "$OUT_DIR" "$h"
  if "$COVEN" setup "$h" --verify-only --report-json "$OUT_DIR/$h.json"; then
    ok "$h completed"
  else
    bad "$h did NOT complete (no report written) -- this is a real finding about the shipped build, not a retry"
    FAILED="$FAILED $h"
  fi

  if [ "$h" = "codex" ] && [ -f "$OUT_DIR/codex.json" ]; then
    GOT="$(field "$OUT_DIR/codex.json" candidate_commit)"
    if [ "$GOT" != "$EXPECTED_SHA" ]; then
      bad "candidate_commit is $GOT, expected $EXPECTED_SHA"
      die "this binary was built from a different commit; stopping before spending the other two provider turns"
    fi
    ok "candidate_commit matches $EXPECTED_TAG"
  fi
done

# ---------------------------------------------------------------------------
say "5/5  verify the packet"
# ---------------------------------------------------------------------------
PACKET_OK=1
for h in codex claude copilot; do
  f="$OUT_DIR/$h.json"
  if [ ! -f "$f" ]; then
    bad "$h.json missing"; PACKET_OK=0; continue
  fi
  C="$(field "$f" completed)"; S="$(field "$f" candidate_commit)"; V="$(field "$f" cli_version)"
  printf '   %-8s completed=%-5s cli_version=%-10s commit=%s\n' "$h" "$C" "$V" "$S"
  [ "$C" = "True" ] || [ "$C" = "true" ] || { bad "$h did not complete"; PACKET_OK=0; }
  [ "$S" = "$EXPECTED_SHA" ] || { bad "$h certifies $S, not $EXPECTED_SHA"; PACKET_OK=0; }
done

echo
if [ "$PACKET_OK" -eq 1 ] && [ -z "$FAILED" ]; then
  printf '\033[32mPACKET COMPLETE\033[0m  %s\n' "$OUT_DIR"
  echo "Paste the 5/5 block above back to close coven-v041-cert and coven-v041-rc."
  exit 0
fi
printf '\033[31mPACKET INCOMPLETE\033[0m  failed:%s\n' "${FAILED:- (see above)}"
echo "Do not retry blindly -- a harness failing verification is a finding about the released build."
exit 1
