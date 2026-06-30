---
name: ml-engineer
description: "Build production ML systems: model training pipelines, serving infrastructure, performance optimization, and automated retraining. Use when: (1) designing or building ML pipelines (data validation → training → deployment), (2) optimizing model training (hyperparameter search, distributed training, checkpointing), (3) deploying models to production (REST/gRPC endpoints, batch/stream processing, canary releases), (4) setting up ML monitoring (prediction drift, feature drift, performance decay), (5) implementing feature engineering or feature stores, (6) automating retraining triggers, (7) debugging model performance or serving latency issues. Triggers on: ML pipeline, model training, model serving, feature engineering, hyperparameter tuning, model deployment, inference optimization, model monitoring, MLOps, retraining."
---

# ML Engineer

Build and operate production ML systems across the full lifecycle: data → features → training → validation → deployment → monitoring → retraining.

## Core Workflow

### 1. System Analysis

Before building anything:

1. **Define the problem** — classification, regression, ranking, generation, etc.
2. **Assess data** — volume, quality, drift patterns, labeling status
3. **Set targets** — accuracy, latency (<50ms inference), training time (<4h), cost ceiling
4. **Map infrastructure** — compute (GPU/CPU), storage, orchestration, serving platform
5. **Choose deployment strategy** — real-time, batch, streaming, edge
6. **Plan monitoring** — what metrics, what thresholds, who gets paged

### 2. Pipeline Development

Build modular, versioned pipelines. Each stage should be independently testable and retriable.

```
Data Validation → Feature Engineering → Training → Validation → Deployment → Monitoring
      ↑                                                                          |
      └──────────────────── Retraining Trigger ←──────────────────────────────────┘
```

**Pipeline principles:**
- Data validation FIRST — catch schema drift, missing values, distribution shifts before training
- Version everything: data, features, models, configs, code
- Each stage writes artifacts to a versioned store (MLflow, DVC, W&B)
- Fail fast with clear error messages; never silently produce bad models

See `references/pipelines.md` for stage-by-stage implementation patterns.

### 3. Training & Optimization

Select the right approach based on complexity:

| Data Size | Complexity | Approach |
|-----------|-----------|----------|
| Small (<10K) | Low | Scikit-learn, XGBoost, single-machine |
| Medium (10K-1M) | Medium | PyTorch/TF, single GPU, Optuna HPO |
| Large (1M+) | High | Distributed training (Ray, DeepSpeed), transfer learning |
| Huge (100M+) | Very high | Multi-node, model sharding, mixed precision |

**Hyperparameter optimization:** Use Optuna (Bayesian) by default. Grid search only for ≤3 params with known ranges. Always set a trial budget and time ceiling.

**Validation:** k-fold cross-validation for small data, holdout + temporal split for time-series, stratified for imbalanced classes.

See `references/training.md` for distributed training, transfer learning, and advanced optimization patterns.

### 4. Deployment

Match deployment pattern to use case:

| Pattern | When | Trade-off |
|---------|------|-----------|
| Blue-green | Zero-downtime upgrades | 2x infrastructure cost |
| Canary | Gradual rollout with safety | Slower full deployment |
| Shadow | Test new model on live traffic without serving | Extra compute |
| A/B test | Compare models on real users | Needs traffic splitting infra |
| Batch | Periodic predictions on stored data | Higher latency, lower cost |
| Real-time | Sub-100ms predictions | Requires serving infrastructure |

**Serving checklist:**
- [ ] Model serialized (ONNX, TorchScript, SavedModel)
- [ ] Inference endpoint tested (load test, edge cases)
- [ ] Health check endpoint active
- [ ] Fallback model configured
- [ ] Auto-scaling rules set
- [ ] Request/response logging enabled

See `references/deployment.md` for serving frameworks, scaling, and reliability patterns.

### 5. Monitoring & Retraining

**Monitor these signals continuously:**

| Signal | Detection Method | Action |
|--------|-----------------|--------|
| Prediction drift | PSI, KS test on output distribution | Alert → investigate |
| Feature drift | KS test per feature vs training distribution | Alert → retrain if confirmed |
| Performance decay | Accuracy/F1 on labeled sample drops >5% | Trigger retraining |
| Data quality | Schema validation, null rate, outlier count | Block pipeline |
| Latency spike | p95 > 2x baseline | Scale up or optimize |

**Retraining triggers:**
- Scheduled (weekly/monthly baseline)
- Drift-triggered (automated when metrics cross threshold)
- Manual (new labeled data, new features, architecture change)

See `references/monitoring.md` for drift detection implementations and alerting patterns.

## Quick Reference — ML System Template

```
project/
├── data/
│   ├── raw/              # Immutable source data
│   ├── processed/        # Feature-engineered datasets
│   └── validation/       # Test sets, golden sets
├── features/
│   ├── pipelines/        # Feature transformation code
│   └── store/            # Feature store config
├── training/
│   ├── configs/          # Hyperparameter configs (versioned)
│   ├── scripts/          # Training entry points
│   └── experiments/      # MLflow/W&B experiment logs
├── serving/
│   ├── model/            # Serialized model artifacts
│   ├── api/              # Inference endpoint code
│   └── tests/            # Load tests, integration tests
├── monitoring/
│   ├── drift/            # Drift detection scripts
│   ├── alerts/           # Alert configs
│   └── dashboards/       # Monitoring dashboard configs
└── pipelines/
    ├── train.py          # End-to-end training pipeline
    ├── deploy.py         # Deployment automation
    └── retrain.py        # Retraining trigger logic
```

## Reference Files

- `references/pipelines.md` — Data validation, feature engineering, pipeline orchestration (Kubeflow, Airflow, Prefect)
- `references/training.md` — Distributed training, HPO, transfer learning, ensemble strategies, advanced techniques
- `references/deployment.md` — Serving frameworks (BentoML, Seldon, TorchServe), scaling, reliability, A/B testing
- `references/monitoring.md` — Drift detection, alerting, retraining automation, incident response
