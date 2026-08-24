---
summary: "Provider-owned login and explicitly consented verification for Codex, Claude Code, and GitHub Copilot CLI."
read_when:
  - Installing or authenticating a supported harness
  - Verifying provider access before a release
  - Producing a redacted setup certification report
title: "coven setup"
description: "Reference for coven setup: provider-owned login, optional bounded verification, terminal and privacy boundaries, outcomes, and redacted certification reports."
---

`coven setup` is the interactive setup path for Coven's three supported
built-in harnesses. It discovers the provider CLI on `PATH`, shows the exact
provider-owned command, asks for explicit consent, and then hands the terminal
to that CLI.

```sh
coven setup <codex|claude|copilot|all>
```

The default selector is `all`, so bare `coven setup` is equivalent to
`coven setup all`.

## Provider commands

| Harness | Install | Login command run by `coven setup` |
| --- | --- | --- |
| Codex | `npm install -g @openai/codex` or `brew install --cask codex` | `codex login` |
| Claude Code | `npm install -g @anthropic-ai/claude-code` | `claude auth login` |
| GitHub Copilot CLI | `npm install -g @github/copilot` or `brew install --cask copilot-cli` | `copilot login` |

The provider owns the login UI and credential store. Coven does not read,
proxy, copy, persist, or redact provider credentials. If a CLI is missing,
setup prints its official install and login guidance instead of trying another
executable.

## Modes

Login only is the default:

```sh
coven setup codex
coven setup claude
coven setup copilot
coven setup all
```

Add a verification turn after a successful login:

```sh
coven setup codex --verify
```

Skip login and run only the verification turn:

```sh
coven setup codex --verify-only
```

`--verify` and `--verify-only` conflict. Verification sends the fixed prompt
`Reply with OK.` through the provider's non-interactive command:

| Harness | Verification command |
| --- | --- |
| Codex | `codex exec --skip-git-repo-check --color never -- Reply with OK.` |
| Claude Code | `claude --print -- Reply with OK.` |
| GitHub Copilot CLI | `copilot --no-color --prompt=Reply with OK.` |

Login consent does not imply verification consent. With `--verify`, Coven asks
again before the provider turn and warns that it requires network access and
may incur provider usage or cost.

## Terminal and automation boundary

Setup is deliberately interactive:

- It refuses provider execution in a non-TTY environment and reports
  `non_tty`.
- After explicit consent, the provider process inherits stdin, stdout, and
  stderr directly. Coven does not capture, parse, or replay the provider's
  login UI or verification response.
- Coven prints the human outcome summary only after the provider exits. There
  is no machine JSON on stdout while a provider runs.
- Each selected provider receives an independent ordered result. `all` keeps
  processing the remaining providers after a decline, cancellation, missing
  executable, provider failure, or timeout.
- Each provider action is bounded to five minutes. Timeout cleanup terminates
  the owned provider process tree.

The command exits successfully only when every selected provider completes.
Outside report mode, human outcomes are `completed`, `not_installed`,
`declined`, `cancelled`, `provider_failed`, `timed_out`, `non_tty`, or
`verification_failed`.

Do not redirect stdout or stderr to create a report. Those streams belong to
the provider process and may contain its login UI, verification response, or
other provider-owned data. Use only `--report-json <path>` for a redacted
machine-readable artifact.

## Ephemeral verification

Verification runs from a unique temporary working directory with a temporary
`COVEN_HOME`. That state is removed before Coven reports completion. Existing
provider credentials remain owned by the provider CLI; Coven does not copy
them into the temporary directory.

This verifies only that the selected provider CLI completed the fixed turn at
that moment. It is not a durable claim about account identity, billing,
authorization, model selection, or future availability.

## Redacted certification report

`--report-json <path>` creates a machine-readable certification artifact for
one provider:

```sh
coven setup codex --verify-only --report-json <path>
```

The flag requires `--verify` or `--verify-only` and cannot be used with
`all`. The destination must not exist. Publication is atomic and
fail-if-exists, so Coven never replaces an earlier report or exposes a partial
file.

Report mode is success-only. If verification does not complete, the CLI version
cannot be established, ephemeral cleanup fails, or privacy validation rejects
the artifact, Coven publishes no report and exits nonzero. Run the same
verification without `--report-json` when you need its human outcome
classification.

The redacted JSON schema contains exactly:

```json
{
  "harness": "codex",
  "cli_version": "1.2.3",
  "platform": "macos-aarch64",
  "candidate_commit": "0123456789abcdef",
  "duration": 1234,
  "exit_class": "completed",
  "completed": true
}
```

The fields are `harness`, `cli_version`, `platform`, `candidate_commit`,
`duration` in milliseconds, `exit_class`, and `completed`. The report excludes
prompts, responses, stdout, stderr, paths, usernames, account identifiers,
tokens, cookies, authorization data, and model identifiers. A successful
verification, strict CLI version, valid build commit, completed ephemeral-state
cleanup, and privacy validation are all required before publication.

## CI and real certification

Automated CI uses fake harness executables to prove command arguments, terminal
ownership, ordering, failure handling, timeout cleanup, and report privacy. CI
receives no real provider credentials and does not certify provider access.

Real provider verification is an operator-run ceremony on the frozen candidate
commit, from an interactive terminal with an authenticated provider account.
The operator grants explicit verification consent and preserves the resulting
redacted report outside provider stdout and stderr.

## Doctor is separate

`coven doctor` is an offline, hermetic readiness check. It launches no provider
CLI process, performs no provider network request, does not inspect provider
tokens or credential stores, and does not verify authentication. Doctor can
report whether a supported executable is visible and point to `coven setup`;
only the explicitly consented verification mode runs a provider turn.

## Related

- [Doctor](/reference/cli-doctor)
- [Provider auth boundary](/harnesses/provider-auth)
- [Codex harness](/harnesses/codex)
- [Claude Code harness](/harnesses/claude-code)
- [Copilot CLI harness](/harnesses/copilot-cli)
