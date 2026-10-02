#!/bin/sh
# Regenerates crates/coven-cli/tests/fixtures/automations-release-stores
# (coven#1054): stores written by released daemons through their own control
# actions, then dumped with sqlite3.
#
# usage: scripts/generate-automation-release-stores.sh <coven v0.4.3> <coven v0.4.6> <coven at HEAD>
#
# Build each binary from its tag with `cargo build -p coven-cli --bin coven --locked`.
# Needs curl, sqlite3 and python3. Every daemon runs with an isolated HOME and
# COVEN_HOME and a PATH without harnesses, so a run-now fails at launch and
# records a failed run instead of starting one.
set -eu

[ $# -eq 3 ] || { sed -n 6p "$0" >&2; exit 2; }
v043=$1
v046=$2
current=$3
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/crates/coven-cli/tests/fixtures/automations-release-stores"
# A short base keeps each daemon socket inside the platform's SUN_LEN limit;
# set COVEN_STORES_TMP to choose another. The canonical path is the one the
# daemons echo back in failure text, which the dump step rewrites.
work=$(cd "$(mktemp -d "${COVEN_STORES_TMP:-/tmp}/cs.XXXXXX")" && pwd -P)
pid=
trap 'if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi; rm -rf "$work"' EXIT

serve() { # <binary> <store directory>
  mkdir -p "$2/h" "$2/c"
  env -i PATH=/usr/bin:/bin HOME="$2/h" COVEN_HOME="$2/c" TZ=UTC \
    "$1" daemon serve >"$2/serve.log" 2>&1 &
  pid=$!
  sock="$2/c/coven.sock"
  tries=0
  until curl -sf --unix-socket "$sock" http://localhost/api/v1/health >/dev/null 2>&1; do
    tries=$((tries + 1))
    [ "$tries" -lt 200 ] || { cat "$2/serve.log" >&2; exit 1; }
    sleep 0.1
  done
}

stop() {
  kill "$pid"
  wait "$pid" 2>/dev/null || true
  pid=
}

act() {
  curl -s --unix-socket "$sock" -H 'content-type: application/json' \
    -X POST http://localhost/api/v1/actions -d "$1"
  echo
}

dump() { # <store directory> <fixture>
  # Replaying the AUTOINCREMENT tables' rows already creates their
  # sqlite_sequence entries, and not every sqlite3 build clears them before
  # the dumped ones, so a restore would hold two rows per table.
  sqlite3 "$1/c/coven.sqlite3" .dump |
    awk '/^INSERT INTO sqlite_sequence / && !cleared { print "DELETE FROM sqlite_sequence;"; cleared = 1 } { print }' |
    sed "s#$1/c/#/coven-home/#g" >"$out/$2"
  if grep -q "$work" "$out/$2"; then
    echo "a local path leaked into $2" >&2
    exit 1
  fi
}

routine() { # <id> <name> <status> <rrule> <runtime> <prompt> [familiar]
  familiar=
  [ $# -lt 7 ] || familiar=",\"familiarId\":\"$7\""
  printf '{"schemaVersion":1,"id":"%s","name":"%s","status":"%s","rrule":"%s","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"%s","cwd":"/work/project","prompt":"%s"%s}' \
    "$1" "$2" "$3" "$4" "$5" "$6" "$familiar"
}

# One release's history: an active and a paused routine, a Codex import (and
# one it must skip), a tick, a run before and after each routine's last edit.
release_store() { # <binary> <store directory>
  mkdir -p "$2/h/.codex/automations/legacy-standup" "$2/h/.codex/automations/legacy-hourly"
  printf '%s\n' 'id = "legacy-standup"' 'name = "Legacy standup"' 'status = "ACTIVE"' \
    'rrule = "RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=8;BYMINUTE=0"' \
    'prompt = "Draft the standup notes."' >"$2/h/.codex/automations/legacy-standup/automation.toml"
  printf '%s\n' 'id = "legacy-hourly"' 'name = "Legacy hourly"' 'status = "ACTIVE"' \
    'rrule = "RRULE:FREQ=HOURLY;INTERVAL=1"' \
    'prompt = "Check the queue."' >"$2/h/.codex/automations/legacy-hourly/automation.toml"
  serve "$1" "$2"
  act "{\"action\":\"coven.automations.create\",\"definition\":$(routine nightly-review 'Nightly review' ACTIVE 'FREQ=DAILY;BYHOUR=3' codex 'Summarize the changes merged yesterday.')}"
  act "{\"action\":\"coven.automations.create\",\"definition\":$(routine weekly-digest 'Weekly digest' PAUSED 'FREQ=WEEKLY;BYDAY=MO;BYHOUR=9' claude 'Write the weekly digest.' charm)}"
  act '{"action":"coven.automations.import"}'
  act '{"action":"coven.automations.tick"}'
  act '{"action":"coven.automations.run","id":"nightly-review"}'
  act "{\"action\":\"coven.automations.update\",\"definition\":$(routine weekly-digest 'Weekly digest' PAUSED 'FREQ=WEEKLY;BYDAY=MO,TH;BYHOUR=9' claude 'Write the weekly digest and flag open risks.' charm)}"
  act "{\"action\":\"coven.automations.update\",\"definition\":$(routine nightly-review 'Nightly review' ACTIVE 'FREQ=DAILY;BYHOUR=3' codex 'Summarize the changes merged yesterday and list follow-ups.')}"
  act '{"action":"coven.automations.run","id":"weekly-digest"}'
  stop
}

release_store "$v043" "$work/a"
dump "$work/a" v0.4.3.sql
release_store "$v046" "$work/b"
dump "$work/b" v0.4.6.sql

# Rollback: this producer upgrades the v0.4.6 store and adds a rich draft, then
# v0.4.6 runs it again, revising that draft, running a routine and creating one.
envelope=$(python3 - "$root/spec/coven-automations/v1/test-vectors.json" <<'EOF'
import hashlib, json, sys
definition = json.load(open(sys.argv[1]))["fixtures"]["definition.golden"]
definition["policies"].pop("delivery", None)
definition.pop("activation", None)
definition["policies"]["retention"]["receipts"] = {"classification": "standard"}
definition.update(automationId="rich-briefing", revision=1, lifecycleState="draft")
covered = {key: value for key, value in definition.items() if key != "integrity"}
canonical = json.dumps(covered, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
definition["integrity"]["value"] = hashlib.sha256(canonical.encode()).hexdigest()
print(json.dumps({"action": "coven.automations.command.v1", "envelope": {
    "schemaVersion": "coven.automations.v1",
    "command": "definition.create.v1",
    "adoptionKey": "adopt:release-rollback:rich-create",
    "origin": {"principal": {"principalId": "principal:owner"}, "channel": "sdk",
               "correlationId": "corr-release-rollback"},
    "intent": {"statement": "Create a rich draft before rolling back."},
    "payload": {"definition": definition}}}))
EOF
)
mkdir -p "$work/r/c"
sqlite3 "$work/r/c/coven.sqlite3" <"$out/v0.4.6.sql"
serve "$current" "$work/r"
act "$envelope"
stop
serve "$v046" "$work/r"
act '{"action":"coven.automations.list"}'
act '{"action":"coven.automations.definition.revise.v1","adoptionKey":"adopt:release-rollback:v046-revise","expectedRevision":1,"definition":{"schemaVersion":1,"id":"rich-briefing","name":"Daily notes","status":"PAUSED","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","familiarId":"charm","cwd":"~/projects/notes","prompt":"Write the daily reflection and the open questions.","retry":{"backoffPolicy":"exponential","maxAttempts":3,"retryableClasses":["transient_dispatch"]},"tags":["notes","daily"]}}'
act '{"action":"coven.automations.run","id":"nightly-review"}'
act "{\"action\":\"coven.automations.create\",\"definition\":$(routine rollback-created 'Created after rollback' PAUSED 'FREQ=WEEKLY;BYDAY=FR;BYHOUR=17' codex 'Close out the week.')}"
stop
dump "$work/r" v0.4.6-rollback.sql
echo "wrote $out"
