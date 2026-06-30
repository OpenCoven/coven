# Training & Optimization Reference

## Distributed Training

### When to Distribute

- Single GPU: model fits in memory, training <4h → don't distribute
- Data parallel: model fits on one GPU, but data is too large → distribute data across GPUs
- Model parallel: model doesn't fit on one GPU → shard model across GPUs
- Pipeline parallel: very deep models → split layers across GPUs

### Data Parallel (most common)

```python
# PyTorch DistributedDataParallel
import torch.distributed as dist
from torch.nn.parallel import DistributedDataParallel as DDP

def setup(rank, world_size):
    dist.init_process_group("nccl", rank=rank, world_size=world_size)

def train(rank, world_size, model, dataset):
    setup(rank, world_size)
    model = model.to(rank)
    model = DDP(model, device_ids=[rank])
    sampler = DistributedSampler(dataset, num_replicas=world_size, rank=rank)
    loader = DataLoader(dataset, sampler=sampler, batch_size=64)
    # Training loop as normal...
```

### Ray Train (multi-node)

```python
from ray.train.torch import TorchTrainer
from ray.train import ScalingConfig

trainer = TorchTrainer(
    train_func,
    scaling_config=ScalingConfig(
        num_workers=4,
        use_gpu=True,
        resources_per_worker={"CPU": 4, "GPU": 1}
    ),
)
result = trainer.fit()
```

## Hyperparameter Optimization

### Optuna (recommended default)

```python
import optuna

def objective(trial):
    lr = trial.suggest_float("lr", 1e-5, 1e-1, log=True)
    n_layers = trial.suggest_int("n_layers", 1, 5)
    dropout = trial.suggest_float("dropout", 0.1, 0.5)
    
    model = build_model(n_layers=n_layers, dropout=dropout)
    accuracy = train_and_evaluate(model, lr=lr)
    return accuracy

study = optuna.create_study(
    direction="maximize",
    pruner=optuna.pruners.MedianPruner(n_startup_trials=5),
)
study.optimize(objective, n_trials=100, timeout=3600)  # 1h budget
```

### HPO Strategy Selection

| Params | Strategy | Why |
|--------|----------|-----|
| 1-3 | Grid search | Exhaustive, interpretable |
| 3-10 | Bayesian (Optuna) | Efficient exploration |
| 10+ | Random search + pruning | Bayesian overhead too high |
| Expensive trials | Multi-fidelity (Hyperband) | Early stopping saves compute |

## Transfer Learning

### When to Use

- Limited labeled data (<10K examples)
- Target domain similar to pre-trained domain
- Need faster convergence

### Pattern

```python
# Fine-tune a pre-trained model
model = load_pretrained("base-model")

# Freeze base layers
for param in model.base.parameters():
    param.requires_grad = False

# Replace/add task-specific head
model.head = TaskHead(hidden_dim=768, num_classes=10)

# Train with lower learning rate
optimizer = Adam(model.head.parameters(), lr=1e-4)

# Optional: unfreeze base after head converges
for param in model.base.parameters():
    param.requires_grad = True
optimizer = Adam(model.parameters(), lr=1e-5)  # Much lower LR for base
```

## Ensemble Strategies

| Method | When | Complexity |
|--------|------|-----------|
| Voting (hard/soft) | Multiple models, simple combination | Low |
| Stacking | Want to learn optimal combination | Medium |
| Bagging (Random Forest) | Reduce variance | Low |
| Boosting (XGBoost, LightGBM) | Reduce bias iteratively | Medium |
| Blending | Large validation set available | Low |

## Training Best Practices

- **Checkpointing:** Save every N epochs + on best validation score. Resume from checkpoint on failure.
- **Early stopping:** Patience of 5-10 epochs on validation metric. Restore best weights.
- **Mixed precision:** Use `torch.cuda.amp` for ~2x speedup with minimal accuracy loss.
- **Gradient accumulation:** Simulate larger batch sizes when GPU memory is limited.
- **Learning rate scheduling:** Warmup + cosine decay or OneCycleLR for most tasks.
- **Reproducibility:** Set all seeds (random, numpy, torch, cuda). Log full config with every run.

## Advanced Techniques

### Online Learning
- Model updates incrementally as new data arrives
- Use for: recommendation systems, fraud detection, time-sensitive predictions
- Challenges: catastrophic forgetting, concept drift detection

### Active Learning
- Query the most informative unlabeled examples for human annotation
- Reduces labeling cost by 50-80% vs random sampling
- Strategies: uncertainty sampling, query-by-committee, expected model change

### Multi-Task Learning
- Share representations across related tasks
- Hard parameter sharing (shared backbone, task-specific heads)
- Improves generalization when tasks are related
