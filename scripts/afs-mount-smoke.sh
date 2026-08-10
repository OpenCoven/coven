#!/usr/bin/env bash
# Probe whether this macOS host can mount an AFS export and read through it.
#
# Informational, never a gate. `coven-x77` and MOUNT-SPIKE.md §4 established
# that macOS refuses `open()` on network volumes for processes without privacy
# consent, and a CI runner is exactly such an unconsented process. The point of
# running this anyway is to replace a guess with a recorded answer: mounting
# unprivileged is known to work from a consented Terminal, and nobody has ever
# checked what an unconsented one gets.
#
# Prints a verdict for each stage and always exits 0. The workflow step that
# calls it is `continue-on-error` as well, so a refusal here is evidence rather
# than a broken build.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
MOUNT_POINT="$WORK/mnt"
SOURCE="$WORK/src"
SERVE_LOG="$WORK/serve.log"
SERVER_PID=""

cleanup() {
    # Unconditional rather than guarded on a `mount` grep: `mktemp -d` hands
    # back a path under /var, which `mount` reports resolved to /private/var,
    # so matching the literal path silently skips the unmount and leaves a live
    # NFS mount behind. Failing to unmount something that was never mounted is
    # harmless; the reverse is not.
    umount "$MOUNT_POINT" 2>/dev/null \
        || diskutil unmount force "$MOUNT_POINT" 2>/dev/null \
        || true
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
    # The unmount is not always visible to the next syscall, so a busy
    # directory here is expected rather than a leak.
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

verdict() { printf '%-28s %s\n' "$1" "$2"; }

if [ "$(uname -s)" != "Darwin" ]; then
    verdict "platform" "SKIP (not Darwin)"
    exit 0
fi

mkdir -p "$SOURCE" "$MOUNT_POINT"
printf 'mounted read-back works\n' > "$SOURCE/hello.txt"

cargo build -q -p coven-afs --features mount --example afs_serve || {
    verdict "build" "FAIL"
    exit 0
}
verdict "build" "ok"

EXAMPLE="$ROOT/target/debug/examples/afs_serve"
AFS_IMPORT="$SOURCE" "$EXAMPLE" "$WORK/smoke.db" 0 > "$SERVE_LOG" 2>&1 &
SERVER_PID=$!

# The export prints its port once bound. Waiting on that line rather than
# sleeping keeps the probe honest about which stage failed.
PORT=""
for _ in $(seq 1 100); do
    PORT="$(sed -n 's/^afs_serve: port=\([0-9]*\)$/\1/p' "$SERVE_LOG" | head -1)"
    [ -n "$PORT" ] && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.2
done
if [ -z "$PORT" ]; then
    verdict "export bound" "FAIL"
    sed 's/^/    /' "$SERVE_LOG"
    exit 0
fi
verdict "export bound" "ok (port $PORT)"

TOKEN_FILE="$(sed -n 's/^afs_serve: export token written to \(.*\)$/\1/p' "$SERVE_LOG" | head -1)"
if [ ! -r "$TOKEN_FILE" ]; then
    verdict "token file" "FAIL"
    exit 0
fi

# The token reaches `mount_nfs` in argv because it offers no alternative; the
# daemon's answer is to rotate immediately afterwards. Here the export dies
# with the probe, so the exposure ends with it.
if mount_nfs -o "vers=3,tcp,port=$PORT,mountport=$PORT,nolock,soft" \
    "localhost:/$(cat "$TOKEN_FILE")" "$MOUNT_POINT" 2>"$WORK/mount.err"; then
    verdict "mount_nfs" "ok"
else
    verdict "mount_nfs" "REFUSED"
    sed 's/^/    /' "$WORK/mount.err"
    exit 0
fi

# The two halves fail separately under TCC: metadata passes while `open()` is
# refused, which is the signature MOUNT-SPIKE.md recorded.
if ls -la "$MOUNT_POINT" > "$WORK/ls.out" 2>"$WORK/ls.err"; then
    verdict "readdir through mount" "ok"
else
    verdict "readdir through mount" "REFUSED"
    sed 's/^/    /' "$WORK/ls.err"
fi

if cat "$MOUNT_POINT/hello.txt" > "$WORK/cat.out" 2>"$WORK/cat.err"; then
    verdict "read through mount" "ok"
else
    verdict "read through mount" "REFUSED (TCC signature)"
    sed 's/^/    /' "$WORK/cat.err"
fi

if printf 'written by CI\n' > "$MOUNT_POINT/written.txt" 2>"$WORK/write.err"; then
    verdict "write through mount" "ok"
else
    verdict "write through mount" "REFUSED"
    sed 's/^/    /' "$WORK/write.err"
fi

exit 0
