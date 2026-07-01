# Pipeline Patterns Reference

## Data Validation

Run before any training. Catch problems early.

### Schema Validation

```python
# Validate incoming data against expected schema
def validate_schema(df, expected_schema):
    """Check column names, types, and nullable constraints."""
    errors = []
    for col, spec in expected_schema.items():
        if col not in df.columns:
            errors.append(f"Missing column: {col}")
            continue
        if df[col].dtype != spec["dtype"]:
            errors.append(f"{col}: expected {spec['dtype']}, got {df[col].dtype}")
        if not spec.get("nullable", True) and df[col].isnull().any():
            errors.append(f"{col}: contains nulls but nullable=False")
    return errors
```

### Distribution Checks

- Compare current data distribution against training distribution
- Use Population Stability Index (PSI) for overall shift detection
- Flag features with PSI > 0.2 as significantly shifted
- Use Kolmogorov-Smirnov test for individual feature distributions

### Data Quality Gates

```python
QUALITY_GATES = {
    "null_rate_max": 0.05,        # Max 5% nulls per column
    "duplicate_rate_max": 0.01,   # Max 1% exact duplicates
    "outlier_rate_max": 0.02,     # Max 2% outliers (IQR method)
    "min_rows": 1000,             # Minimum dataset size
    "schema_match": True,         # Must match expected schema
}
```

## Feature Engineering

### Feature Pipeline Pattern

```python
class FeaturePipeline:
    """Modular, versioned feature transformation pipeline."""
    
    def __init__(self, version: str):
        self.version = version
        self.steps = []
    
    def add_step(self, name: str, transform_fn, params: dict):
        self.steps.append({"name": name, "fn": transform_fn, "params": params})
    
    def run(self, df):
        for step in self.steps:
            df = step["fn"](df, **step["params"])
        return df
    
    def save(self, path):
        """Serialize pipeline config for reproducibility."""
        ...
```

### Feature Store Integration

**Online features** (real-time serving):
- Low-latency key-value lookup (Redis, DynamoDB)
- Point-in-time correct joins
- Sub-10ms retrieval

**Offline features** (training):
- Batch computation (Spark, Dask)
- Historical feature values
- Time-travel queries for point-in-time correctness

### Feature Consistency

The #1 source of training-serving skew is feature inconsistency:
- Use the SAME transformation code for training and serving
- Store transformations as serialized pipeline artifacts
- Test: transform a known input → compare training vs serving output

## Pipeline Orchestration

### Tool Selection

| Tool | Best For | Complexity |
|------|----------|-----------|
| Prefect | Python-native workflows, moderate scale | Low |
| Airflow | Complex DAGs, large teams, mature ecosystem | Medium |
| Kubeflow | K8s-native, GPU workloads, full MLOps | High |
| Ray | Distributed compute, training + serving | Medium-High |
| Dagster | Data-aware orchestration, software-defined assets | Medium |

### Pipeline Design Principles

1. **Idempotent stages** — re-running a stage produces the same output
2. **Artifact versioning** — every stage writes versioned artifacts
3. **Retry with backoff** — transient failures don't kill the pipeline
4. **Clear ownership** — each stage has a single responsibility
5. **Observability** — every stage emits metrics and logs
6. **Parameterized** — configs drive behavior, not code changes
