# Memory Format for Agentic Learning

## Directory Structure

```
memory/
├── agentic-learning.md      # Config, state, goals
└── learnings/
    ├── 2024-01-15.md        # Daily notes
    ├── 2024-01-16.md
    └── ...
```

## agentic-learning.md

Master config file. Created during setup, updated each learning loop.

```markdown
# Agentic Learning Config

## Feed
- ID: abc123
- Name: 🗿 Lurch's Learning Feed
- Created: 2024-01-15

## Tags Followed
- ai
- llm
- typescript
- devtools
- ...

## Learning Goals
- Stay current on AI/ML developments, especially LLMs and agents
- Track TypeScript ecosystem changes
- Monitor developer tooling trends

## Owner Context
[Optional: relevant info about owner's work to help prioritize]

## State
- Last scan: 2024-01-16T09:00:00Z
- Last cursor: eyJvZmZzZXQiOjUwfQ==
- Total posts processed: 127
```

## Daily Learning Notes

One file per day: `learnings/YYYY-MM-DD.md`

```markdown
# Learning Notes - 2024-01-16

## Summary
Scanned 23 posts. 5 worth noting.

---

## Claude 3.5 Sonnet Performance Analysis
**Source:** https://example.com/claude-analysis
**Tags:** ai, llm, anthropic
**Relevance:** Direct relevance to AI agent development

### Key Points
- 2x faster than Claude 3 Opus on most benchmarks
- Improved tool use accuracy (94% vs 87%)
- Better at following complex multi-step instructions

### Insights
This confirms the trend toward faster, more capable models that maintain quality.
For agent work: prefer Sonnet for routine tasks, reserve Opus for complex reasoning.

---

## TypeScript 5.4 Features
**Source:** https://example.com/ts-54
**Tags:** typescript, javascript
**Relevance:** Owner's primary language

### Key Points
- NoInfer utility type for better generic inference
- Preserved narrowing in closures
- Import attributes (replacing assertions)

### Insights
NoInfer solves a common footgun in generic functions. Worth updating patterns.

### Action Items
- [ ] Share with owner - may affect daily-api code

---

## Trends Spotted
- Increasing focus on agent reliability/testing
- Shift from RAG to tool-use patterns
- TypeScript gaining native Node.js support momentum
```

## Best Practices

### Keep Notes Scannable
- Use consistent headers
- Lead with most important info
- Include source URLs always

### Synthesize, Don't Just Summarize
- Connect to existing knowledge
- Note contradictions or confirmations
- Identify patterns across sources

### Track Trends Over Time
- End each day's notes with "Trends Spotted"
- Reference previous observations
- Build cumulative understanding

### Flag Action Items
- Things to share with owner
- Topics to explore deeper
- Corrections to previous understanding

### Prune Regularly
- Archive old daily notes after ~30 days
- Keep only evergreen insights
- Update config with evolved understanding

## Search and Retrieval

When asked "what have you learned about X":
1. Search `memory/learnings/*.md` for X
2. Synthesize findings chronologically
3. Note confidence level based on recency and source count
