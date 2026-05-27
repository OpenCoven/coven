# Coven CLI Settings

User settings live at `~/.config/coven/settings.json` (or `$XDG_CONFIG_HOME/coven/settings.json`).
Format is JSONC: `//` and `/* */` comments and trailing commas are allowed.

All keys live under `covenCli.*`.

## Precedence

Today, for keys in the `covenCli.*` namespace:

1. `~/.config/coven/settings.json` (highest)
2. `~/.coven/repos.toml` (legacy)

The only environment variable that affects settings discovery today is
`COVEN_HOME`, which controls the local data dir (`~/.coven/...`) — it does not
override any `covenCli.*` value.

Forward-looking: once the security branch lands and privacy gains
`load_with_settings`, env vars (`COVEN_PERSIST_RAW_ARTIFACTS`,
`COVEN_RAW_ARTIFACT_RETENTION_DAYS`, `COVEN_LOG_RETENTION_DAYS`) will trump
both files for the `covenCli.privacy.*` keys only.

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

    // Forward-looking. Not honored on main yet; will start working when
    // the upstream security/private-session-logs branch lands and ships
    // privacy.rs with load_with_settings(). Until then these keys parse
    // without error but do not affect runtime retention or redaction.
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

## Migration

The legacy TOML files (`~/.coven/repos.toml`, `~/.coven/privacy.toml`) are still read. To migrate,
copy values into the JSONC schema above and delete the corresponding lines from the TOML file. A
`coven config migrate` command is on the roadmap.
