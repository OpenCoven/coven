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
#   ./scripts/afs-mount-e2e.sh
#
# Exits non-zero on the first failed expectation, because unlike the probe this
# one is asserting a contract rather than discovering a platform's behaviour.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
export COVEN_HOME="$WORK/home"
SOCKET="$COVEN_HOME/coven.sock"
PROJECT="$WORK/project"
FAILED=0

cleanup() {
    # Unmount before stopping the daemon: a live mount whose export dies with
    # its parent leaves a stale mount point that needs force.
    if [ -n "${MOUNT_POINT:-}" ]; then
        umount "$MOUNT_POINT" 2>/dev/null \
            || diskutil unmount force "$MOUNT_POINT" 2>/dev/null \
            || true
    fi
    "$COVEN" daemon stop >/dev/null 2>&1 || true
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

step() { printf '\n== %s\n' "$1"; }
ok()   { printf '   ok    %s\n' "$1"; }
bad()  { printf '   FAIL  %s\n' "$1"; FAILED=1; }
api()  { curl -s --unix-socket "$SOCKET" "$@"; }

if [ "$(uname -s)" != "Darwin" ]; then
    echo "macOS only: no other platform advertises a mount backend"
    exit 0
fi

step "build the daemon and the export helper"
# The daemon locates the helper beside its own executable, so both must land in
# the same directory — which `cargo build` does for this workspace.
cargo build -q -p coven-cli || { bad "build coven-cli"; exit 1; }
cargo build -q -p coven-afs --features mount --bins || { bad "build helper"; exit 1; }
COVEN="$ROOT/target/debug/coven"
[ -x "$ROOT/target/debug/coven-afs-serve" ] || { bad "helper missing beside daemon"; exit 1; }
ok "coven + coven-afs-serve"

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
printf '%s' "$MOUNT_JSON" | grep -qiE '"port"|"token"|localhost:' \
    && bad "the mount response leaked a listener address or token" \
    || ok "response carries no port or token"

step "the mount is real"
mount | grep -q "$(basename "$MOUNT_POINT")" \
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
mount | grep -q "$(basename "$MOUNT_POINT")" \
    && bad "still mounted after DELETE" \
    || ok "gone from mount(8)"
MOUNT_POINT=""

step "unmounting again is idempotent"
AGAIN="$(api -X DELETE "http://localhost/api/v1/afs/sessions/$ID/mount" \
    -o /dev/null -w '%{http_code}')"
[ "$AGAIN" = "200" ] && ok "second DELETE is 200" || bad "second DELETE was $AGAIN"

printf '\n'
if [ "$FAILED" -eq 0 ]; then
    echo "end-to-end mount verified through the daemon routes"
else
    echo "end-to-end mount FAILED"
fi
exit "$FAILED"
