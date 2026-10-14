---
summary: "Create a familiar from the terminal: append a roster entry to familiars.toml and seed its workspace."
read_when:
  - Creating a familiar without Cave
  - Scripting first-run setup for a Coven client
  - Looking up coven familiar create
title: "coven familiar create"
description: "Reference for coven familiar create: id rules, the roster entry it appends, the workspace files it seeds, how it writes safely, and its JSON output and error codes."
source_adjacent_reason: "Tracks the familiar create command implemented in crates/coven-cli/src/familiar_create.rs, including its id rules, roster write, and JSON contract."
---

`coven familiar create` adds a familiar to `familiars.toml` and creates its
workspace. A fresh install can then run `coven run <harness> --familiar <id>`
without Cave and without editing the roster by hand. `coven familiars create`
is the same command.

```sh
coven familiar create Astra
coven familiar create "Night Owl" --id owl --role Research --emoji 🦉 --description "Reads late."
coven familiar create Astra --json
```

## Options

| Option | Default | Notes |
| --- | --- | --- |
| `<DISPLAY_NAME>` | required | Quote a name that contains spaces. |
| `--id <ID>` | derived from the display name | See [Ids](#ids). |
| `--role <ROLE>` | `Familiar` | A blank value uses the default. |
| `--description <TEXT>` | empty | |
| `--emoji <EMOJI>` | none | A blank value is left out. |
| `--json` | off | See [Output](#output). |

Every value is trimmed and must be a single line: control characters,
including newlines, are refused.

## Ids

When `--id` is omitted, the id comes from the display name:

1. Lowercase it.
2. Turn every run of characters outside `a-z` and `0-9` into one `-`.
3. Trim dashes from both ends.
4. Cut it to 48 characters.

`Night Owl` becomes `night-owl`. A name with no ASCII letters or digits, such
as `🐈`, gives no id, so pass `--id`.

Every id, derived or explicit, must match `^[a-z0-9][a-z0-9_-]{0,63}$`. An id
the roster already has is refused.

`coven familiars create` always runs this command. To show the Ward surface of
a familiar whose id is `create`, run `coven familiars -- create`.

## What it writes

The command appends one entry to `<COVEN_HOME>/familiars.toml`, and creates
the file when it is missing:

```toml
[[familiar]]
id = "astra"
display_name = "Astra"
role = "Familiar"
description = ""
workspace = "/path/to/coven-home/workspaces/familiars/astra"
```

`emoji` is added when you pass `--emoji`. `workspace` is always an absolute
path under `COVEN_HOME`, so every reader finds the files below instead of
falling back to `<COVEN_HOME>/familiars/<id>/`.

It then creates `<COVEN_HOME>/workspaces/familiars/<id>/` with starter
`SOUL.md` and `IDENTITY.md` files. These are the two files
[`coven familiar-ledger register`](cli-familiar-ledger.md) needs. Edit them
freely. The command does not create `ward.toml` or `MEMORY.md`.

## How it writes safely

- It holds an exclusive lock on `<COVEN_HOME>/familiars.toml.lock` while it
  reads and writes the roster.
- It refuses a roster that does not parse, or one that cannot take another
  `[[familiar]]` entry, and changes nothing.
- It keeps every existing byte. The new roster is the old bytes, a blank line,
  and the new entry. It is staged in a hidden file beside the roster and
  renamed into place, and it keeps the old file's permissions. A new roster is
  readable only by its owner. A symlinked roster is written through to its
  target.
- If the roster changes after it was read, for example through a writer that
  does not take the lock, the command stops with `roster_changed` and leaves
  the roster alone.
- It never overwrites a workspace file. An existing `SOUL.md` or `IDENTITY.md`
  is kept, and the human output lists it.

## Output

```text
Created familiar `astra` (Astra).
  roster     /path/to/coven-home/familiars.toml
  workspace  /path/to/coven-home/workspaces/familiars/astra

Next: coven run <harness> --familiar astra "<task>"
```

With `--json`, stdout carries one JSON document:

```json
{
  "id": "astra",
  "displayName": "Astra",
  "workspace": "/path/to/coven-home/workspaces/familiars/astra",
  "created": true
}
```

A refused or failed create exits 1. With `--json`, the error is the only
output, on stdout:

```json
{
  "created": false,
  "error": {
    "code": "familiar_exists",
    "message": "familiar `astra` already exists in /path/to/coven-home/familiars.toml; choose another id with --id. Nothing was changed."
  }
}
```

Without `--json`, the message goes to stderr as `Error: …`. A usage error,
such as a missing display name, is reported by the argument parser on stderr
with exit code 2 in both modes.

| Code | Meaning |
| --- | --- |
| `invalid_argument` | A blank display name, a value with control characters, or a `COVEN_HOME` path that is not UTF-8. |
| `invalid_id` | The id does not match the pattern, or none could be derived. |
| `familiar_exists` | The roster already has the id. |
| `roster_invalid` | `familiars.toml` does not parse, or cannot take another entry. |
| `roster_changed` | `familiars.toml` changed while the command ran. Retry. |
| `io_error` | A file or directory could not be read or written. |
