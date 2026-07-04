# Coven PR Readiness Agent

This skill supports at-scale PR creation for OpenCoven/coven. It turns local
repo state, issue context, verification output, and risk notes into a complete
review packet.

Use it before opening or refreshing a PR when you need:

- A Context Bundle from the real changed files
- Template Assembly against `.github/pull_request_template.md`
- A Verification Matrix with commands and proof gaps
- A concise Agent Handoff for maintainers or follow-up agents

The skill is intentionally Coven-native. OpenClaw maintainer workflows remain
separate and should redirect OpenCoven PR creation back to this skill.
