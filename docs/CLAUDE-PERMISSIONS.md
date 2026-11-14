---
title: "Claude Code permission policy"
description: "The one permission policy Coven gives every headless Claude Code process it starts, how it is provisioned, and what still sits above and below it."
---

# Claude Code permission policy

Coven starts Claude Code in two headless ways: as the `claude` harness, and as
the backend the `coven-code` engine uses to reach a Claude subscription. Both
carry one policy that ships inside the `coven` binary, so a chat behaves the
same on every machine. Issue [#1738](https://github.com/OpenCoven/coven/issues/1738)
has the measurements behind this design.

## What a headless launch gets

| Argument | Effect |
|---|---|
| `--settings <COVEN_HOME>/policy/claude-settings.json` | Coven's rules sit above project and user settings; administrator-managed settings still rank higher. |
| `--setting-sources project` | Claude Code does not read the operator's `~/.claude/settings.json` or the workspace's `.claude/settings.local.json`. |
| `--permission-mode <mode>` | `coven run --permission full` maps to `bypassPermissions`, and `read-only` to `plan`. With no `--permission`, Claude Code stays in its headless default, which refuses whatever no rule allows. |
| `--mcp-config <Claude config file>` | Only when the operator's Claude config file defines MCP servers. Leaving out the user source would otherwise drop them. |

The policy file is [`crates/coven-cli/policy/claude-settings.json`](../crates/coven-cli/policy/claude-settings.json):

- **Allowed without asking:** `trash`, `mkdir`, `touch`, `cp`, `mv`, `git add`, `git commit`.
- **Refused:** `rm`, `git push`, `git tag`.

A headless session has nobody to answer a prompt, so "refused" is final for
that turn. Under full access everything else runs. The allow list matters in
the other modes.

The refusals are a guardrail, not a security boundary: a shell command can
delete a file without calling `rm`. Use `--write-scope` when writes must be
confined.

## What is covered

| Launch | Covered |
|---|---|
| `coven run claude` with `--stream-json` or a one-shot prompt | Yes, in its arguments. |
| `coven run coven-code` with `--stream-json` or a one-shot prompt | Yes. Coven sets `COVEN_CLAUDE_POLICY` and the engine adds the arguments to its Claude Code backend. |
| `coven run claude --write-scope <file>` | No change. That launch already loads no settings files and carries its own rules. |
| Interactive terminal launches, including the bare `coven` engine | No change. A person can answer a prompt there, and an interactive engine can change its permission while it runs. |

`COVEN_CLAUDE_POLICY` holds a JSON object:

```json
{
  "version": 1,
  "settingsFile": "<COVEN_HOME>/policy/claude-settings.json",
  "settingSources": "project",
  "permissionMode": "bypassPermissions",
  "addDirs": ["/work/attachments"],
  "mcpConfigFile": "<HOME>/.claude.json"
}
```

The paths are absolute. `permissionMode` and `mcpConfigFile` may be `null`. An engine that cannot apply
the object in full refuses the turn and says to update Coven and the engine
together. It does not start Claude Code without the policy.

## Provisioning and migration

The policy is compiled into `coven`. The first covered launch after an install
or update writes it to `<COVEN_HOME>/policy/claude-settings.json` and prints one
line on stderr. Later launches compare the file with the compiled-in content
and leave it alone when they match, so an edited or outdated copy is replaced.

There is nothing to migrate by hand. An operator's own Claude settings files
are not edited or removed. Covered launches stop reading them.

To change the policy, edit the file in this repository and release Coven.

## What still applies besides the policy

- **The workspace's checked-in `.claude/settings.json`**, together with its
  `AGENTS.md`, `CLAUDE.md` and `.claude/rules`. Claude Code loads instructions
  and that settings file from the same source, and an empty source list drops
  the instructions too. The workspace file can add deny rules and hooks. It
  cannot undo a policy deny, and its allow rules add nothing under full access.
- **Administrator-managed settings.** They rank above `--settings`, and Coven
  respects them.

What a covered launch no longer loads: the operator's hooks, plugins and
permission rules from `~/.claude/settings.json`, and anything in a workspace's
`settings.local.json`. MCP servers that a plugin provides go with the plugin.
MCP servers in the operator's Claude config file and account connectors stay.

Claude Code's own config directory is not moved. `CLAUDE_CONFIG_DIR` relocates
sign-in and session history as well as settings, so changing it would sign the
operator out and break resuming existing chats.

## Managed settings, for administrators

Coven does not write Claude Code's managed settings: they apply to every Claude
Code session on the host, including work outside Coven, and need administrator
rights. An organization that wants a floor no user can lower can deploy one
itself. Claude Code reads it from:

| Platform | Path |
|---|---|
| macOS | `/Library/Application Support/ClaudeCode/managed-settings.json` |
| Linux and WSL | `/etc/claude-code/managed-settings.json` |
| Windows | `C:\Program Files\ClaudeCode\managed-settings.json` |

Each location also reads drop-in files from a `managed-settings.d/` directory
beside it. Policy delivered by MDM or from the organization's Claude console
takes precedence over these files unless the managed settings opt into merging.
