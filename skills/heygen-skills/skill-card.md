## Description: <br>
Create HeyGen avatar videos via the v3 Video Agent pipeline, including avatar resolution, aspect ratio correction, prompt engineering, and voice selection. <br>

This skill is ready for commercial/non-commercial use. <br>

## Publisher: <br>
[eve-builds](https://clawhub.ai/user/eve-builds) <br>

### License/Terms of Use: <br>
MIT-0 <br>


## Use Case: <br>
Developers and agent users use this skill to create HeyGen avatar identities and personalized avatar videos through HeyGen MCP, CLI, or OpenClaw plugin workflows. It helps agents gather missing context, prepare scripts and prompts, select avatars and voices, and return finished HeyGen video links. <br>

### Deployment Geography for Use: <br>
Global <br>

## Known Risks and Mitigations: <br>
Risk: The skill handles identity media, avatar creation, voice selection, and remote uploads to HeyGen. <br>
Mitigation: Use it only for faces, voices, and documents the user is authorized to upload, and avoid third-party likenesses or confidential materials unless the user accepts sending them to HeyGen. <br>
Risk: The skill requires HeyGen credentials or OAuth access and can trigger paid video or avatar actions. <br>
Mitigation: Prefer MCP/OAuth or a scoped API key for the intended HeyGen account, and review billing and account permissions before generating videos. <br>
Risk: The skill may persist local avatar identity files and a generation log. <br>
Mitigation: Periodically delete AVATAR files and heygen-video-log.jsonl when the user no longer wants local identity state or generation history retained. <br>
Risk: Installation paths can involve a remote CLI installer. <br>
Mitigation: Inspect the installer before running it and install only in environments where the HeyGen CLI and API access are approved. <br>


## Reference(s): <br>
- [ClawHub listing](https://clawhub.ai/eve-builds/heygen-skills) <br>
- [HeyGen API quick start](https://developers.heygen.com/docs/quick-start) <br>
- [HeyGen CLI](https://developers.heygen.com/cli) <br>
- [HeyGen MCP remote docs](https://developers.heygen.com/docs/mcp-remote) <br>
- [OpenClaw HeyGen plugin](https://github.com/heygen-com/openclaw-plugin-heygen) <br>
- [Asset routing reference](references/asset-routing.md) <br>
- [Avatar discovery reference](references/avatar-discovery.md) <br>
- [Troubleshooting reference](references/troubleshooting.md) <br>


## Skill Output: <br>
**Output Type(s):** [text, markdown, shell commands, configuration, guidance] <br>
**Output Format:** [Markdown guidance with inline shell commands and structured local files] <br>
**Output Parameters:** [1D] <br>
**Other Properties Related to Output:** [May create local AVATAR files, append a video generation log, and return HeyGen video links when configured with user-authorized HeyGen access.] <br>

## Skill Version(s): <br>
2.2.1 (source: server release metadata and frontmatter) <br>

## Ethical Considerations: <br>
Users should evaluate whether this skill is appropriate for their environment, review any generated or modified files before relying on them, and apply their organization's safety, security, and compliance requirements before deployment. <br>
