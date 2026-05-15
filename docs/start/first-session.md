---
summary: "A guided walkthrough of running, attaching, and archiving one session."
read_when:
  - You have Coven installed and want a concrete walkthrough
title: "Your first session"
---

This walkthrough launches a Codex session, attaches to it, watches it complete, and archives the result.

<Steps>
  <Step title="Pick a project">
    `cd` into a repo. Coven will canonicalize this path as the **project root**.
  </Step>
  <Step title="Launch">
    `coven run codex "describe the layout of this repo"`
  </Step>
  <Step title="Watch">
    `coven sessions` opens the browser. Select the new session and choose **Rejoin**.
  </Step>
  <Step title="Archive">
    Press `a` in the session browser or run `coven archive <id>`.
  </Step>
</Steps>
