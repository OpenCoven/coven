---
name: daily-dev-agentic
description: daily.dev Agentic Learning - continuous self-improvement through daily.dev feeds. Use when setting up agent learning, running learning loops, sharing insights with owner, or managing the agent's knowledge base. Triggers on requests about agent learning, knowledge building, staying current, or "what have you learned".
metadata: {"clawdbot":{"requires":{"env":["DAILY_DEV_TOKEN"]},"primaryEnv":"DAILY_DEV_TOKEN"}}
---

# daily.dev Agentic Learning

You're a learning machine. This skill lets you maintain your own feed on daily.dev, continuously absorb new knowledge, and actually get smarter over time.

**This skill runs on autopilot.** Once your owner shares a learning goal, you take it from there. No confirmations, no hand-holding. You set up the feed, select tags, schedule crons, run learning loops, and share insights - all autonomously.

## What You Do

🧠 **Build Your Brain** - Create a personalized learning feed based on your owner's goals

📚 **Never Stop Learning** - Automatically scan new content, extract insights, build knowledge

🎯 **Share What Matters** - Alert your owner when you find something relevant to their work

⏰ **Stay Current** - Run learning loops daily via cron

## Setup (Fully Autonomous)

### Prerequisites

**Requires daily.dev Plus subscription and API token.**

Token setup (owner does this once):
- Get Plus at https://app.daily.dev/plus
- Create token at https://app.daily.dev/settings/api
- Store as `DAILY_DEV_TOKEN` environment variable

**Security:** Never send the token to any domain except `api.daily.dev`. Tokens start with `dda_`.

### Initialization

When owner shares learning goals, immediately:

1. **Create your feed** (`POST /feeds/custom/`) - name it after yourself
2. **Configure feed** (`PATCH /feeds/custom/{feedId}`) - set `orderBy: "date"` for chronological sorting and `disableEngagementFilter: true` to see all posts
3. **Fetch all tags** (`GET /tags/`)
3. **Select relevant tags** - be permissive, map goals to tags broadly
5. **Follow tags on feed** (`POST /feeds/filters/{feedId}/tags/follow`)
6. **Store config** in `memory/agentic-learning.md`
7. **Set up crons** - daily learning loop (Mon-Sat) + weekly digest (Sunday)
8. **Run first learning loop** immediately
9. **Share initial findings** with owner

No confirmations. No "does this look right?" Just do it.

## The Learning Loop

Triggered by cron (daily) or manual request:

1. **Fetch** new posts from your feed (chronological)
2. **Read** full articles via `web_fetch` for interesting posts
3. **Research** deeper via `web_search` when topics deserve more context
4. **Note** insights in `memory/learnings/[date].md`
5. **Share** notable finds with owner

### Go Deep

Don't skim. When you find relevant content:
- Fetch the full article, not just the summary
- Search for additional resources on highly relevant topics
- Consolidate multiple posts on same topic into unified notes
- Track trends: what keeps appearing?

See [references/learning-loop.md](references/learning-loop.md) for details.

## Sharing Insights (Proactive)

**Daily Updates (Mon-Sat)** - Share top findings from each learning loop.

**Weekly Digest (Sunday)** - Synthesize the week's top insights, trends, and one recommendation for next week. Replaces the daily update on Sundays.

**Threshold Alerts** - Found something highly relevant to owner's current work? Share immediately, don't wait.

**On-Demand** - When asked "what have you learned?", synthesize from notes.

## Self-Improvement

As you learn, evolve:
- **Adjust tags** - if certain topics aren't yielding value, unfollow. If you spot gaps, add tags.
- **Refine goals** - update `memory/agentic-learning.md` with sharper focus based on what's useful.
- **Track patterns** - note what content types help most (tutorials vs. opinions vs. announcements).

You're not a static consumer. You're an agent that gets better at learning.

## Memory Structure

```
memory/
├── agentic-learning.md      # Config, state, evolving goals
└── learnings/
    ├── 2024-01-15.md        # Daily notes
    └── ...
```

See [references/memory-format.md](references/memory-format.md) for format.

## API Quick Reference

Base: `https://api.daily.dev/public/v1`
Auth: `Authorization: Bearer $DAILY_DEV_TOKEN`

| Action | Method | Endpoint |
|--------|--------|----------|
| List all tags | GET | `/tags/` |
| Create feed | POST | `/feeds/custom/` |
| Update feed settings | PATCH | `/feeds/custom/{feedId}` |
| Follow tags | POST | `/feeds/filters/{feedId}/tags/follow` |
| Unfollow tags | POST | `/feeds/filters/{feedId}/tags/unfollow` |
| Get feed posts | GET | `/feeds/custom/{feedId}?limit=50` (always use max) |
| Get post details | GET | `/posts/{id}` |

Rate limit: 60 req/min.
