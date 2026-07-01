# Evaluation & Testing Reference

## Test Set Design

**Minimum viable test set:** 20+ examples across these categories:

| Category | % of Set | Purpose |
|----------|----------|---------|
| Happy path | 40% | Standard expected inputs |
| Edge cases | 25% | Boundary conditions, unusual formats |
| Adversarial | 15% | Prompt injection, malformed input, attempts to bypass instructions |
| Ambiguous | 10% | Inputs where reasonable disagreement exists |
| Empty/null | 10% | Missing data, empty strings, unexpected types |

**Test set hygiene:**
- Label each example with expected output and acceptance criteria
- Version the test set alongside the prompt
- Add new cases whenever a production failure occurs

## Metrics

### Core Metrics

- **Accuracy** — % of outputs matching expected result (exact match or semantic equivalence)
- **Format compliance** — % of outputs matching the specified format (JSON validity, field presence, etc.)
- **Token usage** — input + output tokens per call (track separately)
- **Latency** — end-to-end response time (p50, p95, p99)
- **Cost per call** — (input_tokens × input_price) + (output_tokens × output_price)

### Derived Metrics

- **Consistency** — run same input 5x; measure output variance (lower is better)
- **Robustness** — accuracy on adversarial/edge-case subset
- **Efficiency ratio** — accuracy ÷ tokens_used (higher is better)

## A/B Testing Methodology

### Process

1. **Hypothesis** — "Changing X will improve Y by Z%"
2. **Isolate variable** — change exactly ONE thing between variants
3. **Sample size** — minimum 100 runs per variant for statistical significance
4. **Metric selection** — pick 1 primary metric, ≤3 secondary metrics
5. **Run test** — randomize assignment, log all results
6. **Analyze** — compute confidence interval, check p-value < 0.05
7. **Decision** — ship winner, document learnings

### Common A/B Variables

- System prompt wording
- Number of few-shot examples
- Example ordering
- Output format specification
- Temperature / top-p settings
- Model choice (cost/quality tradeoff)
- CoT vs direct prompting

### Statistical Significance

For binary outcomes (pass/fail):
- Use chi-squared test or Fisher's exact test
- Require p < 0.05 for production decisions
- For continuous metrics (latency, token count): use t-test or Mann-Whitney U

## Regression Testing

Run the full test set on every prompt change. Track:

- **New passes** — improvements from the change
- **New failures** — regressions introduced
- **Net change** — (new passes) - (new failures)

Rule: never ship a prompt change with a negative net change unless the failures are in lower-priority categories.

## Evaluation Automation

Template for automated eval script:

```python
# eval_prompt.py
import json

def evaluate(prompt_version, test_set, model):
    results = []
    for case in test_set:
        output = call_model(model, prompt_version, case["input"])
        passed = check_criteria(output, case["expected"], case["criteria"])
        results.append({
            "input": case["input"],
            "expected": case["expected"],
            "actual": output,
            "passed": passed,
            "tokens": count_tokens(output),
            "latency_ms": measure_latency()
        })
    return compute_metrics(results)
```

## Continuous Monitoring

Track these in production dashboards:

- Accuracy (sampled, weekly human review)
- Token usage trend (daily)
- Cost per call trend (daily)
- Latency p95 (real-time alert if >2s)
- Error rate (real-time alert if >5%)
- User satisfaction scores (if available)
