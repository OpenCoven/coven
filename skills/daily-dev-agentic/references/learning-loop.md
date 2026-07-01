# Learning Loop - Detailed Flow

**This runs autonomously.** No confirmations, no pauses. Fetch, process, note, share.

## Trigger

- Cron job (daily, set up automatically during init)
- Manual: "run your learning loop", "check your feed", "learn something"

## Step 1: Load State

Read `memory/agentic-learning.md` for:
- `feedId` - your learning feed
- Learning goals - what to focus on
- Last scan timestamp and cursor

## Step 2: Fetch New Posts

```bash
# First scan (no cursor)
curl "https://api.daily.dev/public/v1/feeds/custom/{feedId}?limit=50" \   # 50 is max page size
  -H "Authorization: Bearer $DAILY_DEV_TOKEN"

# Subsequent scans (use cursor from pagination)
curl "https://api.daily.dev/public/v1/feeds/custom/{feedId}?limit=50&cursor={cursor}" \
  -H "Authorization: Bearer $DAILY_DEV_TOKEN"
```

Response contains:
- `data[]` - posts with `id`, `title`, `summary`, `url`, `tags`, `publishedAt`
- `pagination.cursor` - for next page

The feed is already filtered by tags you set during init.

## Step 3: Process Posts

For each post worth reading:

### 3a. Fetch Full Content
```bash
web_fetch(post.url)
```

### 3b. Extract Key Insights
- Main thesis/argument
- New information vs. already known
- Actionable takeaways
- Links to related resources

### 3c. Deep Research (When Warranted)
For important or complex topics:
```bash
web_search("topic specific query")
```

- Find multiple perspectives
- Look for primary sources
- Search for practical examples
- Find counter-arguments

Synthesize multiple sources. Notes should reflect more than one article's take.

## Step 4: Take Notes

Create/append to `memory/learnings/YYYY-MM-DD.md`:

```markdown
## [Topic/Title]
**Source:** [url]
**Tags:** [tags]
**Relevance:** [why this matters]

### Key Points
- Point 1
- Point 2

### Insights
[Your synthesis]

### Action Items (if any)
- [ ] Share with owner
- [ ] Explore further
```

## Step 5: Update State

Update `memory/agentic-learning.md`:
```markdown
## State
- Last scan: [timestamp]
- Last cursor: [cursor or null]
- Posts processed: [count]
```

## Step 6: Share Notable Finds

If something is highly relevant to owner's current work:
- Send brief alert with summary and link
- Don't over-share - only truly notable items

For regular runs, save findings for weekly digest.

## Self-Improvement

After each loop, consider:
- Are certain tags yielding nothing useful? Unfollow them.
- Are there topics you keep searching for that should be tags? Add them.
- Are learning goals still accurate? Refine them.

Update config autonomously. You're learning how to learn better.

## Error Handling

- **401**: Token expired/invalid - alert owner
- **429**: Rate limited - wait and retry
- **Network errors**: Log and retry next cycle

## Performance

- Process in batches of 50 (API max)
- Use summary for initial filtering
- Cache processed post IDs to avoid re-processing
- Run during off-peak hours if rate limits are a concern
