---
summary: "Archive, summon, sacrifice — Coven's explicit verbs around destructive session operations."
read_when:
  - Cleaning up the active session list
  - Explaining destructive operations to a new user
title: "Rituals"
---

Rituals are Coven's human-friendly session-management verbs. Ritual names are product language. The safety behavior underneath them stays precise and conservative.

<Columns>
  <Card title="Archive" href="/rituals/archive" icon="archive">
    Hide a non-running session. Reversible. Events preserved.
  </Card>
  <Card title="Summon" href="/rituals/summon" icon="moon-star">
    Restore an archived session, then replay/follow it.
  </Card>
  <Card title="Sacrifice" href="/rituals/sacrifice" icon="flame">
    Permanently delete a non-running session and its events.
  </Card>
</Columns>

## Why ritual names?

The three operations on finished sessions have very different consequences:

- **Archive** is reversible and keeps the ledger.
- **Summon** brings an archived session back into the active list.
- **Sacrifice** is destructive, refuses live sessions, and requires `--yes` so beginners do not delete work by accident.

Plain `delete` would invite muscle-memory mistakes. Plain `hide` would lose the symmetry with **summon**. Ritual names make the intent visible.

## Safety rules

| Ritual | Refuses live? | Reversible? | Confirmation? |
|---|---|---|---|
| Archive | yes, refuses live | yes, via summon | no |
| Summon | n/a | n/a | no |
| Sacrifice | yes, refuses live | **no** | requires `--yes` |

The session browser surfaces every ritual as a labeled action. The CLI exposes them as explicit verbs (`coven archive <id>`, `coven summon <id>`, `coven sacrifice <id> --yes`).

## Related

- [Session lifecycle](/sessions/lifecycle)
- [CLI: coven archive](/reference/cli-archive)
- [CLI: coven summon](/reference/cli-summon)
- [CLI: coven sacrifice](/reference/cli-sacrifice)
