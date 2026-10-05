---
summary: "Record familiar identities in the daemon's familiar ledger: register, adopt changed declarations, retire, revoke, restore, and read history."
read_when:
  - Looking up familiar-ledger
  - Registering a familiar for Runtime Authority
  - A binding was refused because a familiar's declarations changed
  - Retiring, revoking or restoring a familiar identity
title: "coven familiar-ledger"
description: "Reference for coven familiar-ledger: the owner's commands for the familiar root and revision ledger that Runtime Authority binds automation runs to."
source_adjacent_reason: "Tracks the familiar ledger CLI and the coven.familiars.ledger.*.v1 actions implemented in this repository."
---

`coven familiar-ledger` records the identity of each familiar that Runtime
Authority binds an execution to. The ledger keeps a stable root per familiar
and a revision for every identity it has had. Each revision retains the
familiar's roster identity fields (`id`, `name`, `display_name`, `role`,
`pronouns`, `person`, `coven`) and the exact text of its `IDENTITY.md`,
`SOUL.md` and, when present, `ward.toml`.

Nothing changes the ledger except these commands. Editing a familiar's roster
entry or declaration files does not record a new identity on its own: a
binding for that familiar is refused until the owner adopts the change.

```sh
coven familiar-ledger register <familiar>   # first identity: a new root at revision 0
coven familiar-ledger adopt <familiar>      # record changed declarations as the next revision
coven familiar-ledger history <familiar>    # every root for the familiar id, with revisions
coven familiar-ledger history <root-id>     # one root (familiar:…)
coven familiar-ledger retire <familiar>     # retire the live root; history stays readable
coven familiar-ledger restore <root-id>     # bring a retired root back with a new revision
coven familiar-ledger revoke <revision-id> --reason "…"   # a revision can never be embodied
```

Every command accepts `--json` to print the ledger's response.

## Requirements

- The familiar must appear exactly once in `~/.coven/familiars.toml`.
- Its workspace must contain `IDENTITY.md` and `SOUL.md` as UTF-8 text. A
  `ward.toml`, when present, must parse.
- The command runs as the owner. Over the daemon API the same actions are
  `coven.familiars.ledger.*.v1`, and they need owner-local IPC; see
  `docs/architecture/coven-automations-runtime-authority.md`.

## How changes are recorded

- `register` mints an opaque root (`familiar:<hex>`) that never reuses the
  familiar id. Registering an id that already has a live root is refused.
- `adopt` compares the current declarations with the head revision. If they
  match, it reports `Unchanged`. Otherwise it records the next revision, signs
  the lineage transition with the daemon's `familiar-binding` key, and marks
  the previous revision superseded. Emoji, icon, description and workspace
  path are not identity, so changing them alone records nothing.
- `retire` closes the live root and frees the familiar id; registering it again
  mints a new root. `restore` instead continues a retired root, as long as no
  other root holds the id.
- `revoke` is permanent. A revoked head can only be replaced by adopting
  changed declarations.

Commands that change a familiar's head read the current head first and send it
as the expected revision, so a concurrent change is reported as
`REVISION_CONFLICT` rather than overwritten. Errors print the ledger's code
and message:

```text
Error: VALIDATION_FAILED: familiar `sage` has no SOUL.md; add it before recording its identity
```
