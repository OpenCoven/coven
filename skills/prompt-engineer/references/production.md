# Production Systems Reference

## Safety Mechanisms

### Input Validation

- Sanitize user inputs before prompt injection (strip control characters, limit length)
- Detect and reject known prompt injection patterns ("ignore previous instructions", role hijacking)
- Validate expected input schema (type, length, format) before sending to model
- Log all rejected inputs for review

### Output Filtering

- Parse model output against expected schema; reject malformed responses
- Check for PII leakage in outputs (emails, phone numbers, SSNs)
- Flag harmful/biased content using a lightweight classifier or keyword filter
- Apply content policy rules before surfacing to users

### Injection Defense

Techniques:
- **Delimiter isolation** — wrap user input in clear delimiters (`<user_input>...</user_input>`)
- **Instruction hierarchy** — system prompt > context > user input; reinforce this in prompt
- **Output validation** — never trust model output as executable code without sanitization
- **Canary tokens** — embed hidden markers to detect prompt leakage

## Multi-Model Strategies

### Model Selection

Match model tier to task complexity:

| Task Tier | Model Class | Use Case |
|-----------|------------|----------|
| Simple | Small/fast (Haiku, GPT-4o-mini) | Classification, extraction, formatting |
| Medium | Mid-tier (Sonnet, GPT-4o) | Summarization, generation, analysis |
| Complex | Frontier (Opus, o3) | Multi-step reasoning, novel synthesis |

### Fallback Chains

```
Primary model (best quality)
  ↓ on failure/timeout
Secondary model (good quality, different provider)
  ↓ on failure/timeout
Tertiary model (fast, cheap, reliable)
  ↓ on failure
Cached/default response
```

### Routing Logic

Route dynamically based on:
- Input complexity (token count, detected task type)
- Current model latency/availability
- Budget remaining for the billing period
- Quality requirements for the endpoint

## Prompt Version Management

### Versioning

- Store prompts in version control (git) alongside code
- Tag each prompt version with a semver: `v1.0.0`
- Include metadata: author, date, test results, reason for change
- Never edit production prompts in place; deploy new versions

### Deployment

```
dev → staging → canary (5% traffic) → production (100%)
```

- Test on staging with full test set before canary
- Monitor canary metrics for 24h before full rollout
- Keep rollback capability (instant revert to previous version)

### Prompt Registry

Maintain a catalog of all production prompts:

```yaml
- name: customer-support-classifier
  version: v2.3.1
  model: gpt-4o-mini
  accuracy: 94.2%
  avg_tokens: 320
  cost_per_call: $0.0003
  owner: team-support
  last_updated: 2026-03-15
```

## Monitoring Setup

### Alerts

| Metric | Warning | Critical |
|--------|---------|----------|
| Accuracy (sampled) | <90% | <80% |
| Latency p95 | >2s | >5s |
| Error rate | >2% | >5% |
| Cost per call | >2x baseline | >5x baseline |
| Token usage | >1.5x baseline | >3x baseline |

### Incident Response

1. Detect anomaly via alerts
2. Check: is it model-side or prompt-side?
3. If model-side: switch to fallback model
4. If prompt-side: rollback to last known-good version
5. Investigate root cause
6. Add regression test for the failure case
7. Deploy fix through normal pipeline

## Team Workflows

### Prompt Review Process

- All prompt changes require peer review (like code review)
- Reviewer checks: clarity, token efficiency, test coverage, safety
- Changes to safety-critical prompts require 2 reviewers

### Documentation Standards

Each production prompt should have:
- **Purpose** — what task it serves
- **Variables** — all `{{placeholders}}` with types and descriptions
- **Examples** — 2-3 representative input/output pairs
- **Known limitations** — failure modes and workarounds
- **Change log** — history of versions and reasons

### Knowledge Sharing

- Maintain a shared prompt pattern library
- Document anti-patterns (what NOT to do, with examples)
- Run periodic prompt reviews to identify optimization opportunities
- Share A/B test results across the team
