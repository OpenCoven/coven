---
name: skill-scanner
description: Scan, audit, validate, package, or pre-submit AgentSkills using the local BunsDev codeql-sdk/clawhub-audit scanner and OpenClaw skill validation. Use when a user asks to run codeql-sdk, scan a skill before submitting, security-audit a SKILL.md package, produce JSON/SARIF reports, or ensure a skill is ready for Cody/ClawHub handoff.
---

# Skill Scanner

Use this skill for pre-submit scanning and validation of AgentSkills.

## Prime directive

Do not call a skill ready until both gates have been attempted:

1. **Skill structure gate** — `openclaw skills check` and/or `package_skill.py` validation.
2. **Security scan gate** — `codeql-sdk` / `clawhub-audit` scan.

If CodeQL CLI is missing, report that as a blocker and give the exact install/rerun command. If CodeQL runs but reports no analyzable source code, mark the security scan as `not applicable`, not passed. Do not silently replace the scan with weaker checks.

## Default local paths

- SDK repo: `/Users/buns/Documents/GitHub/BunsDev/codeql-sdk`
- SDK CLI source: `/Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js`
- Skill creator package validator: `~/.nvm/versions/node/v24.13.0/lib/node_modules/openclaw/skills/skill-creator/scripts/package_skill.py`
- Workspace skills root: `/Users/buns/.openclaw/workspace/skills`

Read `references/codeql-sdk.md` for CLI details, report formats, and troubleshooting.

## Standard scan workflow

Given a skill directory:

```bash
SKILL_DIR=/absolute/path/to/skill
REPORT_DIR="$(dirname "$SKILL_DIR")/.scan-reports/$(basename "$SKILL_DIR")"
mkdir -p "$REPORT_DIR"
```

1. Preflight:

```bash
test -f "$SKILL_DIR/SKILL.md"
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js version
command -v codeql || test -x "$CODEQL_PATH"
```

2. Validate structure:

```bash
openclaw skills check
python3 ~/.nvm/versions/node/v24.13.0/lib/node_modules/openclaw/skills/skill-creator/scripts/package_skill.py "$SKILL_DIR" /tmp/skill-packages
```

3. Run CodeQL SDK scan:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js audit "$SKILL_DIR" \
  --format json \
  --output "$REPORT_DIR/codeql-sdk-results.json" \
  --fail-on-high
```

4. If the JSON scan passes, optionally generate SARIF for CI/ClawHub:

```bash
node /Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js audit "$SKILL_DIR" \
  --format sarif \
  --output "$REPORT_DIR/codeql-sdk-results.sarif"
```

## Expected result summary

Return a compact status block:

```text
Skill: <name>
Structure validation: passed|failed
Package validator: passed|failed
CodeQL SDK scan: passed|failed|blocked|not applicable
Report: <path>
Ready to submit: yes|no
Notes: <critical findings or blocker>
```

## No analyzable source code

If CodeQL errors with `CodeQL did not detect any code`, the SDK scan was attempted but not applicable to that skill's contents. This often happens for markdown-only skills. Report: `CodeQL SDK scan: not applicable (no analyzable source code detected)`. Do not call it a pass.

## CodeQL CLI missing blocker

If scan fails with `CodeQL CLI not found`, say exactly that the SDK is installed but its runtime dependency is missing. On this Mac, the expected install is:

```bash
brew install --cask codeql
```

Then rerun the audit command. Installing CodeQL is a machine-level state change; ask before doing it unless the user explicitly approved install/update actions.

## Report handling

- JSON reports are useful for agent parsing and memory.
- SARIF reports are useful for CI, GitHub, and security tooling.
- Do not commit `.scan-reports/` reports unless the user explicitly asks.
- Do not print secrets found in reports. Summarize rule IDs, severity, file paths, and line numbers only.

## Handoff guidance

When handing a scanned skill to Cody or another agent, include:

- skill directory path
- packaged `.skill` path if available
- scan report path
- whether CodeQL SDK passed or was blocked
- any critical/high finding summaries
- exact rerun command
