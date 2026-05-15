---
summary: "Common problems, environment, and how to ask for help."
read_when:
  - Starting a troubleshooting session
  - Filing a Coven issue
title: "Help"
---

<Columns>
  <Card title="Troubleshooting" href="/help/troubleshooting" icon="wrench">
    Common setup, daemon, harness, session, and API problems.
  </Card>
  <Card title="Environment" href="/help/environment" icon="settings">
    Every Coven-specific environment variable.
  </Card>
  <Card title="Diagnostics bundle" href="/help/diagnostics-bundle" icon="briefcase">
    Generate a redacted diagnostics archive for a bug report.
  </Card>
</Columns>

## Quick triage

<Steps>
  <Step title="Run doctor">
    ```bash
    coven doctor
    ```
    The first stop. It will name what is missing or wrong.
  </Step>
  <Step title="Check the daemon">
    ```bash
    coven daemon status
    ```
    If the socket is unreachable, see [Daemon will not start](/help/daemon-wont-start).
  </Step>
  <Step title="Check the harness">
    ```bash
    coven run codex --help
    coven run claude --help
    ```
    If a harness is missing, see [Harness not found](/help/harness-not-found).
  </Step>
  <Step title="Bundle and ask">
    ```bash
    coven diagnostics bundle
    ```
    Attach the resulting tarball to a [GitHub issue](/help/filing-issues) or share it in [Discord](https://discord.gg/opencoven).
  </Step>
</Steps>

## Community

- [Discord](https://discord.gg/opencoven) — fastest synchronous help.
- [GitHub Discussions](https://github.com/OpenCoven/coven/discussions) — searchable Q&A.
- [GitHub Issues](https://github.com/OpenCoven/coven/issues) — bugs and feature requests.
- [@OpenCvn on X](https://x.com/OpenCvn) — announcements.
