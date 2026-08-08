# Coven CLI Settings

User settings live at `~/.config/coven/settings.json` (or `$XDG_CONFIG_HOME/coven/settings.json`).
Format is JSONC: `//` and `/* */` comments and trailing commas are allowed.

All keys live under `covenCli.*`.

## Precedence

Today, for keys in the `covenCli.*` namespace:

1. `~/.config/coven/settings.json` (highest)
2. `~/.coven/repos.toml` (legacy)

`COVEN_HOME` controls the local data directory (`~/.coven/...`) and the legacy
TOML files beneath it. It does not change where `settings.json` is discovered.

For privacy retention calculations, environment variables
(`COVEN_PERSIST_RAW_ARTIFACTS`, `COVEN_RAW_ARTIFACT_RETENTION_DAYS`,
`COVEN_LOG_RETENTION_DAYS`) override both `privacy.toml` and
`covenCli.privacy.*`.

When a key is set in both the JSONC file and a legacy TOML file, the JSONC
value wins and `coven` can print a one-time stderr warning naming the
shadowed keys (via `settings::warn_if_shadowed`; the doctor and shell entry
points will start emitting this warning in a follow-up commit).

## Schema

```jsonc
{
  "covenCli": {
    // Resolved by `coven patch` when no --repo flag and no positional repo name.
    "defaultRepo": "openclaw",

    // Named repo registry. Replaces / extends ~/.coven/repos.toml.
    // JSONC entries win when both files name the same repo.
    "repos": {
      "openclaw": { "path": "~/dev/openclaw" }
    },

    // Used by scheduled maintenance, storage-health reporting, and manual
    // log pruning. See the note below for event-ingestion behavior.
    "privacy": {
      "persistRawArtifacts": false,
      "rawArtifactRetentionDays": 7,
      "logRetentionDays": 30,
      "extraPatterns": ["(?i)bearer\\s+[a-z0-9]+"]
    },

    // Paths that should always be considered for file-reference globs
    // (used by Phase 3 `@glob/*.md` expansion). Bypasses .gitignore.
    "fuzzy": {
      "alwaysIncludePaths": [".env.example", "docs/secrets-redacted.md"]
    }
  }
}
```

## Privacy settings boundary

The JSONC privacy keys currently affect scheduled maintenance,
storage-health reporting, and `coven logs prune`. Event redaction, raw artifact
creation, and raw artifact retrieval still read `$COVEN_HOME/privacy.toml`
plus the privacy environment variables directly.

For security-sensitive behavior, configure `privacy.toml` or the environment
variables until every ingestion path uses the merged JSONC settings surface.

## Migration

The legacy repository registry at `~/.coven/repos.toml` is still read. You may
move repository entries and `defaultRepo` into the JSONC schema above; JSONC
values win when both files define the same repository.

Do **not** remove privacy values or custom redaction patterns from
`$COVEN_HOME/privacy.toml` yet. Event ingestion and raw artifact access still
read that file directly, so migrating those values only to JSONC can weaken
redaction or disable the intended raw-artifact policy.
