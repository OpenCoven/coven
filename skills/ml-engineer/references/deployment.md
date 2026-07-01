# Deployment Reference

## Serving Frameworks

| Framework | Best For | Protocol | Auto-scaling |
|-----------|----------|----------|-------------|
| BentoML | Python-native, rapid prototyping | REST, gRPC | Yes (BentoCloud) |
| Seldon Core | K8s-native, multi-model | REST, gRPC | Yes (K8s HPA) |
| TorchServe | PyTorch models | REST, gRPC | Manual |
| TF Serving | TensorFlow models | REST, gRPC | Yes (K8s) |
| Triton | Multi-framework, GPU optimization | REST, gRPC | Yes (K8s) |
| Ray Serve | Python-native, composable | REST | Yes (Ray autoscaler) |
| ONNX Runtime | Cross-framework, optimized inference | Library | N/A (embedded) |

### BentoML (recommended for most cases)

```python
import bentoml

@bentoml.service(
    resources={"gpu": 1, "memory": "4Gi"},
    traffic={"timeout": 30, "concurrency": 32},
)
class MyModelService:
    def __init__(self):
        self.model = bentoml.models.get("my-model:latest").load()
    
    @bentoml.api
    async def predict(self, input_data: dict) -> dict:
        result = self.model.predict(input_data)
        return {"prediction": result}
```

## Model Serialization

Optimize model format before deployment:

| Format | Framework | Benefit |
|--------|-----------|---------|
| ONNX | Any → ONNX Runtime | Cross-platform, optimized inference |
| TorchScript | PyTorch | No Python dependency at inference |
| SavedModel | TensorFlow | TF Serving compatible |
| GGUF/GGML | LLMs | Quantized, CPU-friendly |

### ONNX Export (PyTorch)

```python
import torch.onnx

torch.onnx.export(
    model,
    dummy_input,
    "model.onnx",
    input_names=["input"],
    output_names=["output"],
    dynamic_axes={"input": {0: "batch_size"}},
    opset_version=17,
)
```

## Scaling Patterns

### Request Batching
- Collect requests over a short window (5-50ms)
- Process as a single batch for GPU efficiency
- 3-10x throughput improvement for GPU models

### Model Caching
- Cache predictions for repeated inputs (Redis, Memcached)
- Use input hash as cache key
- Set TTL based on data freshness requirements

### Auto-scaling Rules
```yaml
# K8s HPA example
metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Pods
    pods:
      metric:
        name: inference_queue_depth
      target:
        type: AverageValue
        averageValue: 10
```

## Reliability Patterns

### Health Checks

```python
@app.get("/health")
async def health():
    return {"status": "healthy", "model_loaded": model is not None}

@app.get("/ready")
async def ready():
    # Verify model can produce predictions
    test_output = model.predict(CANARY_INPUT)
    return {"status": "ready", "canary_passed": validate(test_output)}
```

### Circuit Breaker

```python
class CircuitBreaker:
    def __init__(self, failure_threshold=5, recovery_time=60):
        self.failures = 0
        self.threshold = failure_threshold
        self.recovery_time = recovery_time
        self.state = "closed"  # closed=normal, open=failing, half-open=testing
    
    def call(self, fn, *args, **kwargs):
        if self.state == "open":
            if time_since_open > self.recovery_time:
                self.state = "half-open"
            else:
                return self.fallback()
        
        try:
            result = fn(*args, **kwargs)
            self.on_success()
            return result
        except Exception:
            self.on_failure()
            return self.fallback()
```

### Fallback Models

Always have a fallback:
1. **Primary:** Full model (best accuracy, higher latency)
2. **Fallback:** Smaller model or heuristic (lower accuracy, guaranteed low latency)
3. **Last resort:** Cached most-common prediction or graceful error

## A/B Testing for Models

### Setup

```python
def route_request(user_id, experiment_config):
    """Deterministic routing based on user hash."""
    bucket = hash(user_id) % 100
    if bucket < experiment_config["treatment_pct"]:
        return "model_b"  # Treatment
    return "model_a"      # Control
```

### Metrics to Track

- Primary: business metric (revenue, engagement, conversion)
- Secondary: model metric (accuracy, latency)
- Guardrail: safety metric (error rate, harmful outputs)

### Decision Framework

- Run for minimum 7 days (captures weekly patterns)
- Require p < 0.05 on primary metric
- Guardrail metrics must not regress
- Document decision and archive experiment config
