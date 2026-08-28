#!/usr/bin/env bash
# Produce a three-harness release certification packet.
#
#   scripts/certify-release.sh <tag>        e.g. scripts/certify-release.sh v0.4.1
#
# Runs `coven setup <provider> --verify-only --report-json` against real Codex,
# Claude Code and GitHub Copilot CLI accounts and checks that every report
# certifies the tagged commit.
#
# Needs a TTY: `coven setup` asks for explicit network/cost consent and runs one
# bounded real turn per provider, so it exits nonzero and writes nothing when
# piped. Each turn costs real provider usage; the script is ordered to fail
# before spending turns it does not need to.
#
# Environment:
#   COVEN_BIN  pin a specific coven binary (required when several are on PATH)
#   OUT_DIR    where reports are written (default ~/coven-cert-<tag>)

set -euo pipefail

usage() {
  printf 'usage: %s <tag>\n  e.g. %s v0.4.1\n' "$0" "$0" >&2
  exit 2
}

[ "$#" -eq 1 ] || usage
case "$1" in
  -h|--help) usage ;;
  v[0-9]*) ;;
  *) printf 'error: tag must look like v1.2.3, got %s\n' "$1" >&2; usage ;;
esac

EXPECTED_TAG="$1"
EXPECTED_VERSION="${EXPECTED_TAG#v}"
OUT_DIR="${OUT_DIR:-$HOME/coven-cert-$EXPECTED_TAG}"

say()  { printf '\n\033[1m== %s\033[0m\n' "$*"; }
ok()   { printf '   \033[32mok\033[0m   %s\n' "$*"; }
bad()  { printf '   \033[31mFAIL\033[0m %s\n' "$*" >&2; }
die()  { bad "$*"; exit 1; }

# Read one field from a report without requiring jq. Never fails: an unreadable
# or malformed report yields an empty value so the caller reports it as a packet
# problem instead of aborting the run under `set -e` with a traceback.
field() { # field <file> <key>
  python3 -c 'import json, sys
try:
    value = json.load(open(sys.argv[1])).get(sys.argv[2], "")
except Exception:
    value = ""
print(value)' "$1" "$2" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
say "1/5  repo root and expected commit"
# ---------------------------------------------------------------------------
# `.git` is a *file* in a linked worktree and absent in subdirectories, so ask
# git instead of stat-ing a directory.
git rev-parse --is-inside-work-tree >/dev/null 2>&1 \
  || die "run this from inside the coven checkout (no git work tree here)"
# Resolve through refs/tags explicitly: a bare tag-shaped rev also matches a local
# *branch* of that name, which would sail past the "fetch your tags" guard.
EXPECTED_SHA="$(git rev-parse --verify --quiet "refs/tags/$EXPECTED_TAG^{commit}")" \
  || die "tag $EXPECTED_TAG not found locally -- run: git fetch origin --tags"
ok "$EXPECTED_TAG -> $EXPECTED_SHA"

# Every report read below goes through python3. Fail here, before any consent
# prompt, rather than after a paid provider turn has already been spent.
command -v python3 >/dev/null 2>&1 \
  || die "python3 is required to read the reports -- install it before running this"

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
    # No `| head -1` here: under `pipefail` a multi-line writer takes SIGPIPE,
    # the pipeline reports 141, and the `|| echo` fallback would append
    # "(failed)" *after* a version line that was actually read fine.
    ver="$("$p" --version 2>/dev/null || true)"
    ver="${ver%%$'\n'*}"
    printf '        %-56s %s\n' "$p" "${ver:-(failed)}"
  done <<< "$PATHS"
fi

if [ -n "${COVEN_BIN:-}" ]; then
  [ -x "$COVEN_BIN" ] || die "COVEN_BIN is not executable: $COVEN_BIN"
  COVEN="$COVEN_BIN"
  ok "using explicit COVEN_BIN=$COVEN"
else
  COUNT="$(printf '%s\n' "$PATHS" | grep -c . || true)"
  [ "$COUNT" -ge 1 ] || die "coven is not on PATH -- npm install -g @opencoven/cli@$EXPECTED_VERSION"
  if [ "$COUNT" -ne 1 ]; then
    bad "$COUNT coven binaries on PATH -- certifying the wrong one is silent and undetectable later"
    die "re-run pinned to the $EXPECTED_TAG one, e.g.:  COVEN_BIN=/path/to/coven $0"
  fi
  COVEN="$(printf '%s\n' "$PATHS" | head -1)"
  ok "exactly one coven on PATH"
fi

VERSION_LINE="$("$COVEN" --version)" \
  || die "$COVEN --version failed -- the install is broken; fix it before certifying"
printf '        %s\n' "$VERSION_LINE"
# `coven --version` always prints "coven <desc> (engine ...)". Anchor on the
# leading token: an unanchored *v0.4.1* also matches v0.4.10 and v0.4.1-rc1.
case "$VERSION_LINE" in
  "coven $EXPECTED_TAG "*) ok "version is $EXPECTED_TAG" ;;
  *) die "expected $EXPECTED_TAG, got: $VERSION_LINE  (npm install -g @opencoven/cli@$EXPECTED_VERSION)" ;;
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

  if [ "$h" = "codex" ]; then
    # No codex report means the baked commit is unverifiable, which is exactly
    # the state the codex-first ordering exists to stop at -- don't fall through
    # and spend the other two provider turns on an unidentified build.
    [ -f "$OUT_DIR/codex.json" ] \
      || die "no codex report, so this binary's candidate_commit is unverifiable; stopping before spending the other two provider turns"
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
