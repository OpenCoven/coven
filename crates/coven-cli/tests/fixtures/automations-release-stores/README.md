# Automation stores written by released daemons

`sqlite3 .dump` output of whole Coven stores. A released `coven daemon serve`
wrote each one through its own control actions. `automations/release_upgrade_tests.rs`
restores each dump and upgrades it with the current store initialization
(coven#1054).

| Fixture | Written by | Contents |
| --- | --- | --- |
| `v0.4.3.sql` | v0.4.3, before the Automations v1 contract | `nightly-review` (ACTIVE), `weekly-digest` (PAUSED) and the Codex import `legacy-standup` (PAUSED, `local` timezone). A failed manual run of each routine; `nightly-review` was edited after its run, `weekly-digest` before. |
| `v0.4.6.sql` | v0.4.6, the last release before rich definitions and transition events | The same history, with revisions, digests, pins and definition events written by v0.4.6 itself. |
| `v0.4.6-rollback.sql` | v0.4.6 after a rollback | `v0.4.6.sql` upgraded by the current producer, given the rich draft `rich-briefing` through the command envelope, then run by v0.4.6 again. v0.4.6 revised the draft with its versioned revise, ran `nightly-review`, and created `rollback-created`. |

Each daemon ran with an isolated `HOME` and `COVEN_HOME` and with `PATH` limited to
`/usr/bin:/bin`. Every run-now therefore failed at launch and recorded a failed run
without starting a harness. Routine working directories are the placeholder
`/work/project`. The one local path the daemons wrote, `COVEN_HOME` inside a
familiar-lookup failure reason, is rewritten to `/coven-home`. No digest covers
that text.

## Regenerating

Build `coven` at `v0.4.3`, at `v0.4.6`, and at the commit under test
(`cargo build -p coven-cli --bin coven --locked`), then run:

```sh
scripts/generate-automation-release-stores.sh <v0.4.3 coven> <v0.4.6 coven> <current coven>
```

Ids, timestamps and digests change on every run. The tests assert structure and
relationships, never those values. Set `COVEN_STORES_TMP` if the default `/tmp`
base is unavailable. Daemon sockets must fit the platform's Unix socket path
limit, so keep that base short.
