---
name: openclaw-trust
description: Apply OpenClaw’s official threat model and trust guidance (MITRE ATLAS–mapped) when evaluating risky workflows, skills, channel exposure, prompt injection, and tool execution safety.
---

# OpenClaw Trust

Use this skill when the user asks about security posture, threat modeling, attack vectors, trust boundaries, or safe rollout decisions for OpenClaw automations.

## Source of truth

- Repo: `openclaw/trust`
- Core files:
  - `threats.yaml` — machine-readable threat model
  - `CONTRIBUTING.md` — how to submit threats/mitigations/chains
- Public render: `trust.openclaw.ai/trust/threatmodel`
- Vulnerability reporting: `security@openclaw.ai`

## What to apply in practice

### 1) Default to the highest-risk classes first
Prioritize controls for risks repeatedly marked high/critical in the trust model:

- Prompt injection (direct + indirect)
- Malicious skills / compromised skill updates
- Token theft from local config
- Tool argument injection and command execution abuse
- MCP/tool over-permission

### 2) Translate trust model into concrete controls
When recommending hardening, prefer specific actions:

- Default-deny sender/channel access (`AllowFrom` least privilege)
- Approval gates for state-changing actions
- Strict tool argument validation (no free-form shell interpolation)
- Limit/monitor skill install/update sources
- Least-privilege MCP server config
- Audit logs for new sender authorization + sensitive actions
- Token hygiene (rotation, minimize plaintext exposure)

### 3) Separate threat reporting vs vuln disclosure
- Threat model additions/updates → `openclaw/trust` issues/PRs
- Live exploitable vulnerabilities → responsible disclosure via Trust page/security email

## Contribution pattern (for writing trust updates)

When drafting a new threat proposal, include:

- scenario + attack path
- affected OpenClaw components
- rough risk estimate
- suggested mitigation(s)

Maintainers will map to MITRE ATLAS tactic/technique + assign IDs.

## Fast mental checklist for risky automation

- Can untrusted content reach prompt/tool parameters?
- Is there a hard approval gate before side effects?
- Could this run with broader permissions than required?
- Is sender/tool/source authenticated and allowlisted?
- Are outputs validated before execution/posting/sending?

If any answer is “no/unknown”, treat as medium+ risk and recommend guardrails before rollout.
