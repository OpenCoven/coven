---
summary: "Run the Coven daemon as a launchd user agent on macOS."
read_when:
  - Keeping the daemon up on macOS
title: "launchd service"
description: "Run the Coven daemon under launchd on macOS: write a plist, load it, and have launchctl supervise the daemon across reboots and crashes."
---

# launchd service

Use a launchd user agent when you want the Coven daemon started automatically for your macOS user. Install and verify Coven manually first:

```sh
npm install -g @opencoven/cli
coven doctor
coven daemon start
coven daemon status
coven daemon stop
```

## User agent plist

Create the LaunchAgents directory:

```sh
mkdir -p "$HOME/Library/LaunchAgents"
```

Write `~/Library/LaunchAgents/coven.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>coven</string>

  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/env</string>
    <string>coven</string>
    <string>daemon</string>
    <string>start</string>
  </array>

  <key>EnvironmentVariables</key>
  <dict>
    <key>COVEN_HOME</key>
    <string>$HOME/.coven</string>
  </dict>

  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
```

Replace `$HOME` with your absolute home path before loading the plist. launchd does not expand shell variables inside plist strings.

Load and verify:

```sh
launchctl bootstrap gui/UID ~/Library/LaunchAgents/coven.plist
launchctl kickstart -k gui/UID/coven
coven daemon status
```

Replace `UID` with the output of `id -u`.

Unload:

```sh
launchctl bootout gui/UID/coven
```

## PATH and harnesses

launchd uses a smaller environment than your interactive shell. If `coven`, `codex`, or `claude` is installed in a user-local directory, prefer absolute paths in `ProgramArguments` or add a `PATH` entry under `EnvironmentVariables`.

After changing the plist:

```sh
launchctl bootout gui/UID/coven
launchctl bootstrap gui/UID ~/Library/LaunchAgents/coven.plist
coven doctor
```

## Related

- [macOS install](/install/macos)
- [COVEN_HOME layout](/daemon/coven-home)
- [Updating Coven](/install/updating)
