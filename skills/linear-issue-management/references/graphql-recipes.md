# Linear GraphQL recipes

Use these templates with GraphQL variables. Keep secrets out of logs.

## Minimal request wrapper

Create a payload file instead of inlining large text in the shell:

```bash
cat > /tmp/linear-query.json <<'JSON'
{
  "query": "query Viewer { viewer { id name email } }"
}
JSON

curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @/tmp/linear-query.json
```

## Discover teams

```graphql
query Teams($first: Int = 50) {
  teams(first: $first) {
    nodes { id key name }
  }
}
```

## Search issues

```graphql
query SearchIssues($term: String!, $first: Int = 20) {
  issueSearch(term: $term, first: $first) {
    nodes {
      id
      identifier
      title
      url
      state { id name type }
      team { id key name }
      labels { nodes { id name group { id name } } }
      updatedAt
    }
  }
}
```

Search with repo names, exact error strings, config keys, and likely issue titles before creating duplicates.

## Team metadata: labels, states, projects

```graphql
query TeamMetadata($teamId: String!, $first: Int = 100) {
  team(id: $teamId) {
    id
    key
    name
    states(first: $first) { nodes { id name type position } }
    labels(first: $first) {
      nodes { id name color group { id name } }
    }
    projects(first: $first) {
      nodes { id name state url }
    }
  }
}
```

## Create issue

```graphql
mutation CreateIssue($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { id identifier title url }
  }
}
```

Variables shape:

```json
{
  "input": {
    "teamId": "<team-id>",
    "title": "<title>",
    "description": "<markdown-body>",
    "projectId": "<optional-project-id>",
    "labelIds": ["<label-id>"],
    "priority": 2
  }
}
```

Priority is optional; confirm local team conventions before setting it. If unsure, omit it.

## Update issue

```graphql
mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue { id identifier title url state { name } labels { nodes { id name } } }
  }
}
```

Examples of update input:

```json
{
  "stateId": "<state-id>",
  "labelIds": ["<complete-label-set-after-conflict-resolution>"],
  "projectId": "<project-id>",
  "assigneeId": "<user-id>"
}
```

When editing labels, send the complete intended label set only after fetching current labels.

## Add comment

```graphql
mutation AddComment($issueId: String!, $body: String!) {
  commentCreate(input: { issueId: $issueId, body: $body }) {
    success
    comment { id url }
  }
}
```

## Fetch by issue key

```graphql
query IssueByKey($key: String!) {
  issue(id: $key) {
    id
    identifier
    title
    url
    description
    state { id name type }
    team { id key name }
    project { id name url }
    labels { nodes { id name group { id name } } }
    assignee { id name email }
  }
}
```

## Conflict handling checklist

Before mutation:

1. Group candidate labels by `group.id`.
2. If any group has more than one child label, select one.
3. Preserve existing non-conflicting labels unless replacement was requested.
4. Mention any dropped conflicting label in the handoff/result.

Known OpenClaw conflict: `Infra` and `Improvement` are mutually exclusive grouped child labels.
