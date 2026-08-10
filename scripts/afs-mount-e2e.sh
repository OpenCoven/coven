#!/usr/bin/env bash
# Drive a real AFS mount through the daemon routes, end to end.
#
# `scripts/afs-mount-smoke.sh` proves the *export* can be mounted; it starts an
# export by hand and calls `mount_nfs` itself. Nothing has ever exercised the
# path a caller actually uses: `POST /api/v1/afs/sessions/:id/mount`, where the
# daemon spawns the export, mounts it, rotates the token, records the mount,
# and hands back a mount point. `afsMount` advertises `"nfs"` on the strength
# of in-process tests and that export-level probe alone.
#
# This is a manual verification, not a CI gate. It starts a daemon, mounts a
# session, reads and writes through the mount, unmounts, and checks the daemon
# agreed at every step. Run it from a terminal on macOS:
#
#   ./scripts/afs-mount-e2e.sh              # build from this checkout
#   ./scripts/afs-mount-e2e.sh --installed  # use the globally installed package
#
# `--installed` is the post-release check. v0.3.0 published five packages at
# the right version and shipped a mount backend nobody could enable, because
# the platform package omitted `coven-afs-serve` and nothing ever inspected
# what users actually receive (coven-g2t). Testing the tree cannot catch that;
# testing the artifact can.
#
# Exits non-zero on the first failed expectation, because unlike the probe this
# one is asserting a contract rather than discovering a platform's behaviour.
set -uo pipefail

USE_INSTALLED=0
for arg in "$@"; do
    case "$arg" in
        --installed) USE_INSTALLED=1 ;;
        -h|--help)
            # Everything between the shebang and `set`, so the help cannot
            # drift out of sync as the header grows.
            sed -n '2,/^set -uo pipefail/p' "${BASH_SOURCE[0]}" \
                | sed '$d' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            printf 'unknown option: %s\n' "$arg" >&2
            exit 2
            ;;
    esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
export COVEN_HOME="$WORK/home"
SOCKET="$COVEN_HOME/coven.sock"
PROJECT="$WORK/project"

cleanup() {
    # Unmount before stopping the daemon: a live mount whose export dies with
    # its parent leaves a stale mount point that needs force.
    if [ -n "${MOUNT_POINT:-}" ]; then
        umount "$MOUNT_POINT" 2>/dev/null \
            || diskutil unmount force "$MOUNT_POINT" 2>/dev/null \
            || true
    fi
    [ -n "${COVEN:-}" ] && "$COVEN" daemon stop >/dev/null 2>&1 || true
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

step() { printf '\n== %s\n' "$1"; }
ok()   { printf '   ok    %s\n' "$1"; }
# Fail fast, as the header promises. Once an expectation is unmet the rest of
# the run describes a system already known to be wrong, and cascading failures
# obscure which one actually broke.
bad()  { printf '   FAIL  %s\n' "$1"; exit 1; }
api()  { curl -s --unix-socket "$SOCKET" "$@"; }

# Whether `mount(8)` reports something mounted at exactly this path.
#
# Matched as a fixed string against the fully resolved path rather than by
# grepping the basename: a basename can collide with an unrelated mount, and
# `mktemp -d` returns a /var path that `mount` reports resolved to /private/var,
# so an unresolved comparison silently never matches.
mounted_at() {
    local resolved
    # `--` so a path beginning with `-` is a path, not an option to cd.
    resolved="$(cd -- "$1" 2>/dev/null && pwd -P)" || return 1
    mount | grep -qF " on $resolved ("
}

if [ "$(uname -s)" != "Darwin" ]; then
    echo "macOS only: no other platform advertises a mount backend"
    exit 0
fi

if [ "$USE_INSTALLED" -eq 1 ]; then
    step "use the installed package"
    # Resolved through the wrapper's own resolution rather than a guessed
    # path, so this tests what `npm i -g` actually produced.
    WRAPPER="$(command -v coven || true)"
    [ -n "$WRAPPER" ] || { bad "no coven on PATH"; exit 1; }
    # realpath, not `readlink`: the wrapper is a RELATIVE symlink, and BSD
    # readlink resolves it against the caller's directory rather than the
    # link's own, which lands nowhere.
    WRAPPER_REAL="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$WRAPPER")"
    # Both macOS platform packages: `cli-macos` is Apple Silicon and
    # `cli-macos-x64` is Intel. Checking only the first would fail on every
    # Intel install, where the package is present under the other name.
    INSTALL_BIN=""
    wrapper_dir="$(dirname "$WRAPPER_REAL")"
    for pkg in cli-macos cli-macos-x64; do
        for candidate in \
            "$wrapper_dir/../node_modules/@opencoven/$pkg/bin" \
            "$wrapper_dir/../../node_modules/@opencoven/$pkg/bin"; do
            if [ -x "$candidate/coven" ]; then
                INSTALL_BIN="$(cd "$candidate" && pwd -P)"
                INSTALL_PKG="$pkg"
                break 2
            fi
        done
    done
    [ -n "$INSTALL_BIN" ] || { bad "could not locate the installed platform package from $WRAPPER"; exit 1; }
    COVEN="$INSTALL_BIN/coven"
    if [ ! -x "$INSTALL_BIN/coven-afs-serve" ]; then
        # The exact v0.3.0 failure, named rather than surfacing later as
        # afsMount=false with no explanation.
        bad "@opencoven/${INSTALL_PKG} ships no coven-afs-serve; the mount backend cannot work (coven-g2t)"
        exit 1
    fi
    ok "installed $("$COVEN" --version 2>/dev/null | head -1)"
    ok "helper shipped beside it in @opencoven/${INSTALL_PKG}"
else
    step "build the daemon and the export helper"
    # The daemon locates the helper beside its own executable, so both must
    # land in the same directory — which `cargo build` does for this workspace.
    cargo build -q -p coven-cli || { bad "build coven-cli"; exit 1; }
    cargo build -q -p coven-afs --features mount --bins || { bad "build helper"; exit 1; }
    COVEN="$ROOT/target/debug/coven"
    [ -x "$ROOT/target/debug/coven-afs-serve" ] || { bad "helper missing beside daemon"; exit 1; }
    ok "coven + coven-afs-serve"
fi

step "start a daemon on a throwaway COVEN_HOME"
mkdir -p "$COVEN_HOME" "$PROJECT"
git -C "$PROJECT" init -q 2>/dev/null
printf 'from the base\n' > "$PROJECT/hello.txt"
git -C "$PROJECT" add -A 2>/dev/null
git -C "$PROJECT" -c user.email=e2e@example.invalid -c user.name=e2e \
    commit -qm "base" 2>/dev/null
"$COVEN" daemon start >/dev/null 2>&1
for _ in $(seq 1 100); do [ -S "$SOCKET" ] && break; sleep 0.2; done
[ -S "$SOCKET" ] || { bad "daemon socket never appeared"; exit 1; }
ok "socket up"

step "the daemon advertises a mount backend"
BACKEND="$(api http://localhost/api/v1/health | python3 -c "
import json,sys
print(json.load(sys.stdin)['capabilities']['afsMount'])" 2>/dev/null)"
if [ "$BACKEND" = "nfs" ]; then ok "afsMount=nfs"; else
    bad "afsMount=$BACKEND — nothing to verify"; exit 1
fi

step "create an AFS session"
ID="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"projectRoot\":\"$PROJECT\"}" http://localhost/api/v1/afs/sessions \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])" 2>/dev/null)"
[ -n "$ID" ] || { bad "no session id"; exit 1; }
ok "session $ID"

step "mount it through the route"
MOUNT_JSON="$(api -X POST -H 'Content-Type: application/json' -d '{}' \
    "http://localhost/api/v1/afs/sessions/$ID/mount")"
MOUNT_POINT="$(printf '%s' "$MOUNT_JSON" | python3 -c "
import json,sys
d=json.load(sys.stdin); print(d.get('mountPoint',''))" 2>/dev/null)"
if [ -z "$MOUNT_POINT" ]; then
    bad "mount refused: $MOUNT_JSON"; exit 1
fi
ok "mountPoint $MOUNT_POINT"

# §3.3: the response must never carry a listener address.
if printf '%s' "$MOUNT_JSON" | grep -qiE '"port"|"token"|localhost:'; then
    bad "the mount response leaked a listener address or token"
fi
ok "response carries no port or token"

step "the mount is real"
mounted_at "$MOUNT_POINT" \
    && ok "visible in mount(8)" \
    || bad "not present in mount(8)"

step "read and write through it"
ls "$MOUNT_POINT" >/dev/null 2>&1 && ok "readdir" || bad "readdir"
if [ "$(cat "$MOUNT_POINT/hello.txt" 2>/dev/null)" = "from the base" ]; then
    # The merged view is the whole point: this file is in the base, and the
    # delta is empty, so a delta-only export would show nothing.
    ok "base file readable (merged view)"
else
    bad "base file not readable through the mount"
fi
printf 'written through the mount\n' > "$MOUNT_POINT/written.txt" 2>/dev/null \
    && ok "write" || bad "write"

step "the daemon reports the session as mounted"
SESSION_MOUNT="$(api "http://localhost/api/v1/afs/sessions/$ID" | python3 -c "
import json,sys; print(json.load(sys.stdin).get('mount') or '')" 2>/dev/null)"
[ "$SESSION_MOUNT" = "$MOUNT_POINT" ] \
    && ok "session.mount matches" \
    || bad "session.mount=$SESSION_MOUNT expected $MOUNT_POINT"

step "the write is visible to the daemon's diff"
CHANGES="$(api "http://localhost/api/v1/afs/sessions/$ID/diff" | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(','.join(sorted(c['path'] for c in d.get('changes',[]))))" 2>/dev/null)"
case "$CHANGES" in
    *written.txt*) ok "diff sees /written.txt" ;;
    *) bad "diff did not record the mounted write (changes: ${CHANGES:-none})" ;;
esac

step "unmount through the route"
UNMOUNTED="$(api -X DELETE "http://localhost/api/v1/afs/sessions/$ID/mount" | python3 -c "
import json,sys; print(json.load(sys.stdin).get('unmounted'))" 2>/dev/null)"
[ "$UNMOUNTED" = "True" ] && ok "unmounted" || bad "unmount reported $UNMOUNTED"
mounted_at "$MOUNT_POINT" \
    && bad "still mounted after DELETE" \
    || ok "gone from mount(8)"
MOUNT_POINT=""

step "unmounting again is idempotent"
AGAIN="$(api -X DELETE "http://localhost/api/v1/afs/sessions/$ID/mount" \
    -o /dev/null -w '%{http_code}')"
[ "$AGAIN" = "200" ] && ok "second DELETE is 200" || bad "second DELETE was $AGAIN"

printf '\nend-to-end mount verified through the daemon routes\n'
exit 0
