## Description: <br>
Discord archive: search, sync freshness, DMs, channel slices, SQL counts, and Discrawl repo work. <br>

This skill is ready for commercial/non-commercial use. <br>

## Publisher: <br>
[openclaw](https://clawhub.ai/user/openclaw) <br>

### License/Terms of Use: <br>
MIT-0 <br>


## Use Case: <br>
Developers and engineers use this skill to inspect local Discord archive data, check sync freshness, search messages, review bounded channel or DM slices, and produce exact counts or date spans without relying first on live Discord APIs. <br>

### Deployment Geography for Use: <br>
Global <br>

## Known Risks and Mitigations: <br>
Risk: The skill can access sensitive Discord archive content, including private messages or secrets captured in local snapshots. <br>
Mitigation: Install only from a trusted Discrawl source, avoid sharing snapshots that contain secrets or private DM data, and report only the minimum archive details needed for the task. <br>
Risk: Bot sync requires Discord credentials and could exceed the intended access boundary if configured too broadly. <br>
Mitigation: Use least-privilege bot credentials and do not provide Discord user tokens or call Discord as the user. <br>
Risk: SQL access could mutate the archive database if unsafe mutation flags are used. <br>
Mitigation: Keep SQL read-only unless the user explicitly requests and reviews a database mutation. <br>


## Reference(s): <br>
- [Discrawl homepage](https://github.com/openclaw/discrawl) <br>
- [Discrawl ClawHub page](https://clawhub.ai/openclaw/discrawl) <br>
- [OpenClaw publisher profile](https://clawhub.ai/user/openclaw) <br>


## Skill Output: <br>
**Output Type(s):** [text, markdown, shell commands, configuration, guidance] <br>
**Output Format:** [Markdown with inline shell commands and concise status or query results] <br>
**Output Parameters:** [1D] <br>
**Other Properties Related to Output:** [Outputs should report absolute date spans, channel or DM names, counts, and known archive gaps when relevant.] <br>

## Skill Version(s): <br>
1.0.0 (source: server release evidence) <br>

## Ethical Considerations: <br>
Users should evaluate whether this skill is appropriate for their environment, review any generated or modified files before relying on them, and apply their organization's safety, security, and compliance requirements before deployment. <br>
