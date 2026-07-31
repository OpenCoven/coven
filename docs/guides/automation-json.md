---
title: "Automate Coven with JSON output"
summary: "Use Coven's documented JSON commands safely in shell automation and local integrations."
read_when:
  - Writing a shell script around Coven
  - Building a local client or status check
description: "Examples for Coven doctor, daemon, and session JSON output, plus the local API boundary."
---

# Automate Coven with JSON output

Use documented `--json` modes for automation. Do not parse the human terminal UI or infer state from decorative output. Each command owns its JSON schema; inspect the matching reference page before depending on optional fields.

## Gate environment readiness

```sh
if coven doctor --json | jq -e '.ok' >/dev/null; then
  echo "Coven is ready"
else
  echo "Coven needs local setup" >&2
  exit 1
fi
```

The `doctor` envelope reports `ok`, `blocking`, checks, and next steps. A warning can be non-blocking; gate on `ok` rather than assuming that every warning is failure.

## Check daemon reachability

```sh
coven daemon status --json | jq -e '.ok and .status == "running"'
```

If a workflow may start the daemon, do so as a separate, explicit action:

```sh
coven daemon start
coven daemon status --json
```

## Read sessions without a terminal UI

```sh
coven sessions --json | jq '.sessions[] | { id, harness, status, title }'
```

Use `coven sessions --all --json` only when archived history is relevant. Treat ids as opaque strings and do not record session titles or event content in public logs without a privacy review.

## Pick the right integration boundary

- Use the CLI JSON commands for local shell automation and maintainer checks.
- Use the versioned local socket API when building a persistent client. Begin with the compatibility handshake described in the [API contract](/API-CONTRACT).
- Keep the daemon local and same-user. It is not a remote HTTP service and does not provide a provider-credential API.

## Output rules

- With `--json`, stdout is for the JSON document. Send your own diagnostics to stderr.
- Avoid `--json` modes that the focused command does not document.
- Preserve unknown fields when relaying a document; consumers should read only fields they own.
- Never paste real session output, paths, tokens, or provider environment data into fixtures or public issue reports.

## Related

- [Core access](/guides/core-access)
- [Developer core-functionality guide](/development/cli-core-functionality)
- [CLI reference](/reference/cli)
- [API contract](/API-CONTRACT)
