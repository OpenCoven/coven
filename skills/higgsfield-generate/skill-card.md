## Description: <br>
Generate images and videos via Higgsfield AI through 30+ models, including general image and video generation plus Marketing Studio workflows for branded ad content. <br>

This skill is ready for commercial/non-commercial use. <br>

## Publisher: <br>
[higgsfield](https://clawhub.ai/user/higgsfield) <br>

### License/Terms of Use: <br>
MIT-0 <br>


## Use Case: <br>
External users and developers use this skill to generate images, videos, image edits, image-to-video clips, and branded marketing assets through the Higgsfield CLI. <br>

### Deployment Geography for Use: <br>
Global <br>

## Known Risks and Mitigations: <br>
Risk: The skill may install the Higgsfield CLI through an unpinned remote installer. <br>
Mitigation: Install or review the Higgsfield CLI from a trusted, versioned source before allowing the agent to use it. <br>
Risk: Generation commands can consume credits in the authenticated Higgsfield account. <br>
Mitigation: Use an account suitable for delegated generation actions and request cost estimates or confirmations when credit usage matters. <br>
Risk: Media files, product URLs, and avatar images may be uploaded to Higgsfield during generation workflows. <br>
Mitigation: Provide only content that is appropriate to upload to Higgsfield and avoid sensitive media unless approved for that service. <br>


## Reference(s): <br>
- [Higgsfield Generate on ClawHub](https://clawhub.ai/higgsfield/higgsfield-generate) <br>
- [Publisher profile](https://clawhub.ai/user/higgsfield) <br>
- [Model catalog](references/model-catalog.md) <br>
- [Media inputs](references/media-inputs.md) <br>
- [Prompt engineering](references/prompt-engineering.md) <br>
- [Troubleshooting](references/troubleshooting.md) <br>
- [Marketing Studio modes](references/marketing-modes.md) <br>
- [Marketing Studio products](references/marketing-products.md) <br>
- [Marketing Studio avatars](references/marketing-avatars.md) <br>


## Skill Output: <br>
**Output Type(s):** [text, markdown, shell commands, configuration, guidance] <br>
**Output Format:** [Markdown guidance with inline Higgsfield CLI commands and generated result URLs] <br>
**Output Parameters:** [1D] <br>
**Other Properties Related to Output:** [May run authenticated Higgsfield CLI generation jobs that upload user-provided media or product URLs and consume account credits.] <br>

## Skill Version(s): <br>
1.0.0 (source: server release metadata) <br>

## Ethical Considerations: <br>
Users should evaluate whether this skill is appropriate for their environment, review any generated or modified files before relying on them, and apply their organization's safety, security, and compliance requirements before deployment. <br>
