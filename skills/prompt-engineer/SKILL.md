---
name: prompt-engineer
description: "Design, optimize, test, and evaluate prompts for large language models. Use when: (1) crafting or refining system prompts, user prompts, or prompt templates, (2) optimizing token usage or cost of existing prompts, (3) designing few-shot examples or chain-of-thought reasoning, (4) setting up prompt evaluation, A/B testing, or regression testing, (5) building production prompt management systems (versioning, monitoring, safety), (6) debugging inconsistent or low-quality LLM outputs, (7) selecting prompt patterns (zero-shot, few-shot, CoT, ToT, ReAct, role-based). Triggers on: prompt engineering, optimize prompt, reduce tokens, prompt template, few-shot, chain-of-thought, prompt evaluation, A/B test prompts, prompt versioning."
---

# Prompt Engineer

Craft and optimize LLM prompts for maximum effectiveness, consistency, and cost efficiency.

## Core Workflow

### 1. Requirements Analysis

Before writing or editing any prompt:

1. Identify the **use case** — what task the prompt must accomplish
2. Define **success criteria** — accuracy target, format requirements, tone
3. Understand **constraints** — token budget, latency ceiling, cost limit, model choice
4. Review **existing prompts** and their failure modes (if any)
5. Determine **safety requirements** — input validation, output filtering, injection defense

### 2. Prompt Design

Select the appropriate pattern based on task complexity. See `references/patterns.md` for detailed guidance on each.

| Complexity | Pattern | When to Use |
|-----------|---------|-------------|
| Simple | Zero-shot | Clear task, model already knows the domain |
| Medium | Few-shot | Specific format or style needed |
| Complex | Chain-of-thought | Multi-step reasoning required |
| Branching | Tree-of-thought | Multiple valid approaches to explore |
| Agentic | ReAct | Tool use + reasoning interleaved |
| Safety | Constitutional AI | Output must pass ethical/policy filters |

Design principles:

- **Instruction clarity** — state the task, constraints, and output format explicitly
- **Minimal tokens** — every token must earn its place; compress without losing meaning
- **Modular structure** — separate system prompt, context, instructions, and examples
- **Variable placeholders** — use `{{variable}}` for dynamic content injection
- **Error recovery** — include fallback instructions for ambiguous or invalid input

### 3. Optimization

Iterate on prompts to reduce cost and improve quality:

- **Token reduction** — remove redundant phrasing, compress examples, use abbreviations the model understands
- **Context compression** — summarize long context; only include what the model needs for the current step
- **Output constraints** — specify format (JSON, markdown, list) to reduce parsing overhead
- **Caching** — identify static prompt sections that can be cached across calls
- **Batch processing** — group similar requests when possible

### 4. Evaluation & Testing

See `references/evaluation.md` for detailed frameworks.

Minimum evaluation protocol:

1. **Create a test set** — 20+ examples covering happy path, edge cases, and adversarial inputs
2. **Define metrics** — accuracy, format compliance, latency, token usage, cost per call
3. **Run baseline** — measure current prompt performance
4. **A/B test variants** — change one variable at a time, measure statistical significance
5. **Regression test** — ensure new versions don't break existing passing cases

Target thresholds (adjust per use case):
- Accuracy: >90%
- Latency: <2s
- Token efficiency: track input/output ratio

### 5. Production Deployment

See `references/production.md` for safety mechanisms, multi-model strategies, and team workflows.

Production checklist:

- [ ] Prompt versioned in source control
- [ ] Safety filters enabled (input validation + output filtering)
- [ ] Monitoring active (accuracy, latency, cost, error rate)
- [ ] Fallback strategy defined (retry logic, model fallback chain)
- [ ] Documentation complete (purpose, variables, examples, known limitations)
- [ ] Cost tracking per query/endpoint

## Quick Reference — Prompt Template Structure

```
[System Prompt]
  Role definition + behavioral constraints + output format

[Context Section]
  {{dynamic_context}} — injected per request

[Instructions]
  Step-by-step task description

[Examples] (if few-shot)
  Input → Output pairs (3-5 diverse examples)

[User Input]
  {{user_query}}

[Output Constraints]
  Format specification + validation rules
```

## Reference Files

- `references/patterns.md` — Detailed prompt patterns (zero-shot, few-shot, CoT, ToT, ReAct, constitutional AI, role-based)
- `references/evaluation.md` — Evaluation frameworks, A/B testing methodology, metrics, statistical analysis
- `references/production.md` — Safety mechanisms, multi-model strategies, production systems, monitoring, team workflows
