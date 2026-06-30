---
name: research-ingestion
description: Extract insights from research papers (PDF/URL) and generate OpenClaw agent skills, workflow improvements, and actionable techniques. Use when asked to analyze a paper, extract research findings, turn a paper into a skill, or apply academic insights to agent workflows.
---

# Research Ingestion

Analyze research papers and extract actionable insights for OpenClaw agent workflows.

## Trigger Phrases
- "analyze this paper", "read this research", "extract insights from"
- "turn this paper into a skill", "what can we learn from this paper"
- "research to skill", "/research"

## Workflow

### 1. Obtain Paper Text

**From URL (arXiv, PDF link, blog post):**
```bash
# Use the summarize skill for URLs
# Or fetch + extract directly:
curl -sL "<url>" | python3 -c "
import sys
try:
    import fitz  # PyMuPDF
    doc = fitz.open(stream=sys.stdin.buffer.read(), filetype='pdf')
    print('\n'.join(page.get_text() for page in doc))
except ImportError:
    # Fallback: use pdftotext if available
    import subprocess
    result = subprocess.run(['pdftotext', '-', '-'], input=sys.stdin.buffer.read(), capture_output=True)
    print(result.stdout.decode())
"
```

**From local file:**
```bash
python3 -c "
import fitz
doc = fitz.open('$FILE_PATH')
for page in doc: print(page.get_text())
"
```

**Fallback (no PDF tools):**
Use the `summarize` skill or `web_fetch` tool on the paper's URL. For arXiv papers, use the HTML version: `https://arxiv.org/html/<id>`.

### 2. Analyze with Structured Extraction

After obtaining the text, extract these categories:

```
## Paper: <title>
Authors: <authors>
Published: <date>
Source: <url>

### Core Contribution
<1-2 sentence summary of what's new>

### Key Techniques
- <technique 1>: <how it works, 2-3 sentences>
- <technique 2>: ...

### Agent Workflow Implications
- <how this applies to OpenClaw agent behavior>
- <specific workflow improvements suggested>

### Actionable Insights
1. <concrete thing we can implement>
2. <concrete thing we can implement>

### Skill Candidates
- <potential skill name>: <what it would do, trigger phrases>

### Limitations & Caveats
- <what doesn't apply or needs adaptation>
```

### 3. Generate Skill (if requested)

When asked to turn insights into a skill, use this scaffold:

```markdown
---
name: <skill-name>
description: <one-line description derived from paper insight>
---

# <Skill Name>

Based on: <paper title> (<url>)

## When to Use
<trigger conditions>

## Technique
<extracted technique adapted for OpenClaw context>

## Workflow
<step-by-step procedure>

## Example
<concrete usage example>
```

Write the skill to `~/.openclaw/workspace/skills/<skill-name>/SKILL.md`.

### 4. Save Insights

Store extracted insights for future reference:

```bash
# Append to the research log
cat >> ~/.openclaw/workspace/memory/research-insights.md << 'EOF'

## <Paper Title> (<date analyzed>)
Source: <url>
Key insight: <1-liner>
Applied to: <skill name or workflow>
EOF
```

## Output Format

Always structure output as:

```
📄 Paper: <title>

🔬 Core Contribution
<summary>

⚡ Actionable for OpenClaw
1. <action item with expected impact>
2. <action item>

🛠️ Skill Potential: <Yes/No>
<if yes, skill name + description>

📝 Next Step
<specific command or action to take>
```

## Examples

**Input:** "Analyze the ParaThinker paper on parallel reasoning"

**Output:** Analysis showing parallel chain-of-thought technique → suggests spawning multiple reasoning paths as subagents → generates a `parallel-reasoning` skill scaffold.

**Input:** "What can we learn from this PDF about code review?"

**Output:** Extracts review heuristics → maps to CodeFlow's review phase → suggests phase-aware prompt improvements.

## Dependencies

- `python3` with `PyMuPDF` (`pip3 install pymupdf`) — preferred
- Or `pdftotext` (from `poppler`) — fallback
- Or `summarize` skill — for URL-based papers
- Or `web_fetch` tool — for HTML papers (arXiv)

Install PDF support:
```bash
pip3 install pymupdf
```
