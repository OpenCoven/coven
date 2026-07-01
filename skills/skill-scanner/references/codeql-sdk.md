# codeql-sdk reference

## Purpose

`codeql-sdk` provides the `clawhub-audit` CLI for CodeQL-backed security scans of AgentSkills / ClawHub skills.

Local repo:

```text
/Users/buns/Documents/GitHub/BunsDev/codeql-sdk
```

Local CLI:

```text
/Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js
```

Package binary name:

```text
clawhub-audit
```

## Commands

Version:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js version
```

Console audit:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js audit /path/to/skill
```

JSON audit:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js audit /path/to/skill \
  --format json \
  --output /path/to/report.json
```

SARIF audit:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js audit /path/to/skill \
  --format sarif \
  --output /path/to/report.sarif
```

Fail on critical/high findings:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js audit /path/to/skill --fail-on-high
```

Parse existing SARIF:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js parse /path/to/results.sarif /path/to/skill
```

## Important options

- `--format console|json|sarif`
- `--output <file>`
- `--min-severity error|warning|recommendation|note`
- `--queries <query-ids...>`
- `--timeout <ms>`
- `--github-risk`
- `--github-owner <owner>`
- `--fail-on-high`

## Query IDs

Known query IDs from the SDK:

- `clawhub/prompt-injection`
- `clawhub/hardcoded-credentials`
- `clawhub/command-injection`
- `clawhub/path-traversal`
- `clawhub/insecure-api-call`
- `clawhub/unsafe-deserialization`
- `clawhub/sensitive-data-exposure`
- `clawhub/missing-input-validation`
- `clawhub/ssrf`
- `clawhub/missing-rate-limit`
- `clawhub/overly-permissive-cors`

## Runtime dependency

The SDK requires GitHub CodeQL CLI. Preflight:

```bash
command -v codeql || test -x "$CODEQL_PATH"
```

Expected install on macOS:

```bash
brew install --cask codeql
```

If CodeQL is installed outside PATH, set:

```bash
export CODEQL_PATH=/path/to/codeql
```

## Common blockers

- `CodeQL CLI not found`: install CodeQL CLI or set `CODEQL_PATH`.
- `outputFile is only supported with json or sarif`: use `--format json` or `--format sarif` when setting `--output`.
- Exit code `1`: scan ran and high/critical findings exist when `--fail-on-high` was used.
- Exit code `2`: scanner/runtime error, such as missing CodeQL CLI or invalid query ID.

## Submission checklist

Before saying a skill is ready:

1. `openclaw skills check` includes the skill under ready skills.
2. `package_skill.py <skill-dir> <dist-dir>` succeeds.
3. `codeql-sdk` scan succeeds, is explicitly marked blocked by missing CodeQL CLI, or is explicitly marked not applicable because CodeQL detected no analyzable source.
4. Reports live outside the skill directory, such as `skills/.scan-reports/<skill-name>/`, so packaged skills do not include scanner artifacts.
5. Findings, if any, are summarized without exposing secrets.
