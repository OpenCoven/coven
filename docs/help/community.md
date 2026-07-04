---
summary: "Discord, X, and where to ask."
read_when:
  - Asking for help
title: "Community"
description: "Where to get community help with Coven: GitHub discussions, the Discord, and the public channels OpenCoven maintainers monitor for questions and bugs."
---

Use the channel that matches the kind of help you need. Include the output of
`coven doctor` whenever the question is about setup, daemon state, or harness
detection.

## Fast setup help

Discord is the fastest path for interactive setup questions:

```text
https://discord.gg/opencoven
```

Share:

```sh
coven --version
coven doctor
coven daemon status
```

Redact project names, prompts, tokens, and private paths before posting.

## Searchable questions

Use GitHub Discussions for questions that should be searchable later:

```text
https://github.com/OpenCoven/coven/discussions
```

Good fits:

- Which install path to use for a platform.
- How to run Coven under systemd, launchd, tmux, SSH, WSL2, or containers.
- How adapters and harnesses should be modeled.
- Clarifying docs before filing a bug.

## Bugs

Use GitHub Issues for reproducible bugs:

```text
https://github.com/OpenCoven/coven/issues
```

Before filing:

```sh
coven doctor
coven daemon status
```

Then follow [Filing issues](/help/filing-issues).

## Announcements

Follow the public OpenCoven account for release notes and project updates:

```text
https://x.com/OpenCvn
```

Do not use social replies for private logs or support bundles. Use Discord,
Discussions, or Issues depending on whether you need live help, searchable Q&A,
or bug tracking.
