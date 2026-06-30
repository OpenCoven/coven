# Monitoring & Retraining Reference

## Drift Detection

### Population Stability Index (PSI)

Measures overall distribution shift between training and production data.

```python
import numpy as np

def compute_psi(expected, actual, bins=10):
    """PSI between two distributions. >0.2 = significant shift."""
    expected_pct = np.histogram(expected, bins=bins)[0] / len(expected)
    actual_pct = np.histogram(actual, bins=bins)[0] / len(actual)
    
    # Avoid log(0)
    expected_pct = np.clip(expected_pct, 1e-6, None)
    actual_pct = np.clip(actual_pct, 1e-6, None)
    
    psi = np.sum((actual_pct - expected_pct) * np.log(actual_pct / expected_pct))
    return psi

# Interpretation
# PSI < 0.1  → No significant shift
# 0.1-0.2    → Moderate shift, investigate
# > 0.2      → Significant shift, action required
```

### Kolmogorov-Smirnov Test (per feature)

```python
from scipy.stats import ks_2samp

def check_feature_drift(train_values, prod_values, alpha=0.05):
    """KS test for individual feature drift."""
    stat, p_value = ks_2samp(train_values, prod_values)
    return {
        "statistic": stat,
        "p_value": p_value,
        "drifted": p_value < alpha,
    }
```

### Prediction Drift

Monitor the output distribution, not just inputs:

```python
def monitor_predictions(recent_predictions, baseline_predictions):
    """Compare recent prediction distribution to baseline."""
    psi = compute_psi(baseline_predictions, recent_predictions)
    mean_shift = abs(np.mean(recent_predictions) - np.mean(baseline_predictions))
    
    alerts = []
    if psi > 0.2:
        alerts.append(f"Prediction PSI={psi:.3f} (threshold: 0.2)")
    if mean_shift > baseline_std * 2:
        alerts.append(f"Mean shift={mean_shift:.3f} (>2σ)")
    return alerts
```

## Alerting Configuration

### Tiered Alerts

| Severity | Condition | Action | Response Time |
|----------|-----------|--------|--------------|
| P0 Critical | Model serving errors >5% | Page on-call, auto-rollback | <15 min |
| P1 High | Accuracy drop >10% on labeled sample | Page on-call | <1 hour |
| P2 Medium | Feature drift detected (PSI >0.2) | Notify team, investigate | <24 hours |
| P3 Low | Prediction distribution shift (PSI 0.1-0.2) | Log, review next sprint | <1 week |

### Alert Fatigue Prevention

- Group related alerts (feature drift + prediction drift → single "model drift" alert)
- Set cooldown periods (don't re-alert for same issue within 4h)
- Auto-resolve when metrics return to normal
- Weekly alert review: tune thresholds, remove noisy alerts

## Retraining Automation

### Trigger Logic

```python
class RetrainingTrigger:
    def __init__(self, config):
        self.scheduled_interval = config["interval_days"]  # e.g., 7
        self.psi_threshold = config["psi_threshold"]        # e.g., 0.2
        self.accuracy_drop = config["accuracy_drop"]        # e.g., 0.05
    
    def should_retrain(self, metrics):
        # Scheduled
        if days_since_last_train > self.scheduled_interval:
            return True, "scheduled"
        
        # Drift-triggered
        if metrics["prediction_psi"] > self.psi_threshold:
            return True, "prediction_drift"
        
        # Performance-triggered
        if metrics["accuracy_drop"] > self.accuracy_drop:
            return True, "performance_decay"
        
        return False, None
```

### Retraining Pipeline

1. **Validate new data** — run quality gates before retraining
2. **Train candidate model** — same pipeline, new data
3. **Evaluate against current model** — must beat current on all key metrics
4. **Shadow deploy** — run alongside production model, compare outputs
5. **Canary release** — route 5% traffic to new model
6. **Full rollout** — if canary passes, promote to 100%
7. **Archive old model** — keep for rollback (retain last 3 versions)

### Safeguards

- Never auto-deploy a model that regresses on ANY key metric
- Require human approval for models trained on <80% of expected data volume
- Log full lineage: data version → feature version → model version → config
- Auto-rollback if production metrics degrade within 1h of deployment

## Incident Response

### Model Failure Playbook

1. **Detect:** Alert fires (serving errors, latency spike, accuracy drop)
2. **Assess:** Is it model-side or infrastructure-side?
3. **Mitigate:**
   - Model-side → rollback to previous model version
   - Infra-side → scale up, restart pods, check dependencies
4. **Investigate:** Pull logs, compare input distributions, check recent deployments
5. **Fix:** Retrain if data issue, patch if code issue, scale if capacity issue
6. **Prevent:** Add regression test, update monitoring, document in post-mortem

### Key Diagnostic Queries

```python
# What changed?
recent_deployments()           # Any new model in last 24h?
data_pipeline_status()         # Any upstream data issues?
feature_distribution_report()  # Any feature drift?
prediction_distribution_report()  # Output distribution normal?
error_log_analysis()           # What types of errors?
```
