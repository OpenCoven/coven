#!/usr/bin/env bash
# One-shot stub generator. Mirrors the docs.json navigation tree.
# Idempotent: skips any file that already exists so flagship pages are preserved.
set -euo pipefail

cd "$(dirname "$0")"

write_stub() {
  local path="$1"
  local title="$2"
  local summary="$3"
  local read_when="$4"
  local body="$5"

  if [[ -f "$path" ]]; then
    return 0
  fi

  mkdir -p "$(dirname "$path")"
  cat > "$path" <<EOF
---
summary: "$summary"
read_when:
  - $read_when
title: "$title"
---

$body
EOF
}

# ---- start/ ----
write_stub start/showcase.md "Showcase" \
  "Highlights of what Coven can do today and where it is heading." \
  "Browsing for a one-page overview of Coven's value" \
"<div class=\"showcase-hero\">
  <p class=\"showcase-kicker\">Coven</p>
  <p class=\"showcase-lead\">A local-first runtime that supervises every coding-agent harness inside explicit project roots, with append-only events and rituals you can audit.</p>
  <div class=\"showcase-actions\">
    <a href=\"/start/getting-started\">Get started</a>
    <a href=\"/concepts/architecture\">See the architecture</a>
  </div>
</div>

## Highlights

<Columns>
  <Card title=\"Codex and Claude Code today\" icon=\"layers\" href=\"/harnesses\">
    Two supported harnesses, more on the way through the adapter spec.
  </Card>
  <Card title=\"comux JSON sessions\" icon=\"braces\" href=\"/sessions/comux-json\">
    Session shape that comux, OpenMeow, and external clients can replay.
  </Card>
  <Card title=\"Rituals over flags\" icon=\"moon\" href=\"/rituals\">
    Archive, summon, sacrifice — explicit verbs around destructive operations.
  </Card>
</Columns>"

write_stub start/getting-started.md "Getting started" \
  "Install Coven, run doctor, start the daemon, and launch your first harness session." \
  "First time setting up Coven on a workstation" \
"Install Coven, run \`coven doctor\`, and launch your first harness session in about five minutes. By the end you will have a running daemon, a project-rooted session record, and a working PTY attached to Codex or Claude Code.

## What you need

- **Rust stable** — only if you build from source. The published \`@opencoven/cli\` wrapper bundles binaries for macOS and Linux.
- **At least one harness CLI on \`PATH\`** — Codex or Claude Code today. \`coven doctor\` will report what is missing and how to install it.

<Tip>
Coven does not store provider credentials. Each harness keeps using its own local auth flow (\`codex login\`, \`claude doctor\`).
</Tip>

## Quick setup

<Steps>
  <Step title=\"Install Coven\">
    <Tabs>
      <Tab title=\"npm\">
        \`\`\`bash
        npm install -g @opencoven/cli
        \`\`\`
      </Tab>
      <Tab title=\"From source\">
        \`\`\`bash
        git clone https://github.com/OpenCoven/coven
        cd coven
        cargo build --workspace --release
        \`\`\`
      </Tab>
    </Tabs>
    <Note>
    Other install methods: [Install](/install).
    </Note>
  </Step>
  <Step title=\"Run doctor\">
    \`\`\`bash
    coven doctor
    \`\`\`
    \`doctor\` checks the store, project boundary, and harness readiness. Follow its hints before continuing.
  </Step>
  <Step title=\"Start the daemon\">
    \`\`\`bash
    coven daemon start
    coven daemon status
    \`\`\`
    The daemon binds a Unix socket under \`\$COVEN_HOME\`. Default: \`~/.coven/coven.sock\`.
  </Step>
  <Step title=\"Launch your first session\">
    \`\`\`bash
    cd /path/to/your/project
    coven run codex \"describe this repo\"
    \`\`\`
    Or open the human session browser:
    \`\`\`bash
    coven sessions
    \`\`\`
  </Step>
</Steps>

## What to do next

<Columns>
  <Card title=\"Sessions and rituals\" href=\"/sessions/lifecycle\" icon=\"folder-tree\">
    Attach, archive, summon, sacrifice — the safe ways to manage live and finished work.
  </Card>
  <Card title=\"Familiars\" href=\"/familiars\" icon=\"sparkles\">
    Name your agents, give them roles, and let them remember.
  </Card>
  <Card title=\"Local API\" href=\"/daemon/socket-api\" icon=\"plug\">
    Build a client that handshakes with \`GET /api/v1/health\`.
  </Card>
</Columns>

## Related

- [Install overview](/install)
- [Doctor](/start/doctor)
- [Coven TUI](/start/coven-tui)"

write_stub start/quickstart.md "Quickstart" \
  "The shortest copy-pasteable path to a live Coven session." \
  "You already know what Coven is and want commands" \
"\`\`\`bash
npm install -g @opencoven/cli
coven doctor
coven daemon start
cd /path/to/your/project
coven run codex \"fix the failing tests\"
coven sessions
\`\`\`

See [Getting started](/start/getting-started) for context."

write_stub start/onboarding.md "Onboarding" \
  "Guided first run, project selection, harness verification, and ritual safety." \
  "Walking a teammate through their first Coven setup" \
"\`coven\` opens the interactive menu by default. The onboarding flow:

1. Confirms \`\$COVEN_HOME\` and creates it if missing.
2. Runs \`coven doctor\` and surfaces install hints.
3. Asks for the project root and validates it.
4. Picks a harness (\`codex\` or \`claude\`) and verifies its CLI.
5. Suggests the safest first command.

See [Coven TUI](/start/coven-tui) for the slash-command palette."

write_stub start/doctor.md "Doctor" \
  "What coven doctor checks and how to read its output." \
  "Diagnosing a fresh install or a broken environment" \
"\`coven doctor\` is the first command to run after install. It reports:

- Whether \`\$COVEN_HOME\` is writable.
- Whether the daemon socket can bind.
- Whether \`codex\` and \`claude\` are on \`PATH\` and what version they are.
- Whether the SQLite store is reachable.

Each finding includes a remediation hint. Re-run \`coven doctor\` after fixing any line marked \`needs attention\`."

write_stub start/first-session.md "Your first session" \
  "A guided walkthrough of running, attaching, and archiving one session." \
  "You have Coven installed and want a concrete walkthrough" \
"This walkthrough launches a Codex session, attaches to it, watches it complete, and archives the result.

<Steps>
  <Step title=\"Pick a project\">
    \`cd\` into a repo. Coven will canonicalize this path as the **project root**.
  </Step>
  <Step title=\"Launch\">
    \`coven run codex \"describe the layout of this repo\"\`
  </Step>
  <Step title=\"Watch\">
    \`coven sessions\` opens the browser. Select the new session and choose **Rejoin**.
  </Step>
  <Step title=\"Archive\">
    Press \`a\` in the session browser or run \`coven archive <id>\`.
  </Step>
</Steps>"

write_stub start/openclaw-rescue.md "OpenClaw rescue loop" \
  "Use Coven to repair a broken OpenClaw checkout without a healthy OpenClaw runtime." \
  "Your local OpenClaw is broken and you need a repair room" \
"\`\`\`bash
coven patch openclaw
coven patch openclaw \"fix Codex auth profile order after invalidated OAuth token\"
coven patch openclaw --repo ~/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
\`\`\`

\`coven patch openclaw\` detects the repo, asks what is broken, launches a supervised Codex or Claude Code session, runs verification, and reports changed files. Coven does not commit or push in v0."

write_stub start/coven-tui.md "Coven TUI" \
  "The prompt-first interactive menu launched by `coven` or `coven tui`." \
  "Browsing what the interactive Coven menu can do" \
"\`coven\` or \`coven tui\` opens the prompt-first interface. It accepts:

- Free-form task text (\`fix the failing tests\`).
- Slash commands (\`/run codex <task>\`, \`/sessions\`, \`/archive\`).
- Arrow-key navigation through ritual menus.

The TUI is the recommended starting point for new users."

write_stub start/automation.md "Automation overview" \
  "Where automation lives in the Coven stack and how it relates to OpenMeow." \
  "Choosing where to put automation that calls Coven" \
"Coven is the canonical shared local runtime for reusable automation. OpenMeow stays a chat UI and intent layer. The flow is:

\`\`\`text
user -> OpenMeow -> Coven -> adapters -> desktop/apps
\`\`\`

See [Automation](/automation) for the full surface."

# ---- concepts/ ----
write_stub concepts/features.md "Features" \
  "What Coven can do today — harnesses, sessions, rituals, capabilities, and the local API." \
  "Comparing Coven's surface against another runtime" \
"<Columns>
  <Card title=\"Project-rooted launches\" icon=\"folder-tree\">
    Every session pins a canonical project root. Cwd must canonicalize inside that root.
  </Card>
  <Card title=\"Harness-neutral PTYs\" icon=\"terminal\">
    Codex and Claude Code today; Hermes, Aider, Gemini, Cline tomorrow.
  </Card>
  <Card title=\"Append-only event log\" icon=\"scroll\">
    Output, exit, and metadata events stored in SQLite for replay.
  </Card>
  <Card title=\"Rituals\" icon=\"moon\">
    Archive, summon, sacrifice — explicit, beginner-safe verbs around destructive operations.
  </Card>
  <Card title=\"Local socket API\" icon=\"plug\">
    Versioned HTTP-over-Unix-socket contract under \`/api/v1\`.
  </Card>
  <Card title=\"Control plane\" icon=\"compass\">
    Capability discovery + action routing for clients like comux and OpenMeow.
  </Card>
</Columns>"

write_stub concepts/runtime-topology.md "Runtime topology" \
  "How the daemon, harnesses, store, and clients fit together." \
  "Understanding which Coven component owns which responsibility" \
"\`\`\`mermaid
flowchart LR
  User[Developer] --> CLI[coven CLI / TUI]
  CLI --> Daemon[Coven daemon]
  Comux[comux] --> Daemon
  OpenMeow[OpenMeow] --> Daemon
  Plugin[@opencoven/coven plugin] --> Daemon
  Daemon --> Adapter[Adapter router]
  Adapter --> Codex[Codex PTY]
  Adapter --> Claude[Claude Code PTY]
  Daemon --> Store[(SQLite)]
  Daemon --> Events[(Event log)]
\`\`\`

See [Architecture](/concepts/architecture) for the full picture and [Authority boundary](/concepts/authority-boundary) for trust rules."

write_stub concepts/authority-boundary.md "Authority boundary" \
  "The Rust daemon is Rank 0. Clients can ask; only the daemon decides." \
  "Auditing what Coven validates vs. trusts from clients" \
"\`\`\`mermaid
flowchart TD
  Client[CLI, TUI, comux, OpenClaw plugin] --> Request[Launch / input / kill / list request]
  Request --> Rust[Rank 0 authority: Rust daemon]
  Rust --> RootCheck{projectRoot explicit?}
  RootCheck -- no --> RejectRoot[Reject]
  RootCheck -- yes --> CwdCheck{cwd canonicalized inside root?}
  CwdCheck -- no --> RejectCwd[Reject]
  CwdCheck -- yes --> HarnessCheck{harness allowlisted?}
  HarnessCheck -- no --> RejectHarness[Reject with install hint]
  HarnessCheck -- yes --> Spawn[Spawn harness with argv APIs]
\`\`\`

Clients are convenience layers. The Rust daemon is the only thing allowed to spawn a PTY, canonicalize a path, or mutate the session ledger."

write_stub concepts/control-plane.md "Control plane" \
  "Capability discovery and action routing for clients that don't want to know which adapter handles what." \
  "Adding a new client that integrates with Coven" \
"The control plane sits in front of adapters. It lets clients:

- Discover what Coven can do with \`GET /api/v1/capabilities\`.
- Send known intents via \`POST /api/v1/actions\`.
- Stay decoupled from brittle OS automation APIs.

Unknown action ids fail closed."

write_stub concepts/store.md "Store" \
  "Coven's local SQLite database: session ledger plus append-only event log." \
  "Inspecting Coven state on disk or recovering after a crash" \
"The store lives under \`\$COVEN_HOME\` and holds two logical tables:

- **Sessions** — id, project root, harness, status, exit code, archive state, timestamps.
- **Events** — append-only output/exit/metadata records keyed by session id.

Do not commit \`.coven/\`, databases, sockets, logs, or environment files to source control."

# ---- install/ ----
for pair in \
  "install/index|Install overview|All ways to install Coven on a workstation or server.|Choosing how to install Coven" \
  "install/npm|Install via npm|Install the @opencoven/cli wrapper from npm.|Using npm or pnpm to install Coven" \
  "install/cargo|Install via cargo|Build and install Coven directly from crates.io with cargo.|You prefer building Rust binaries yourself" \
  "install/from-source|Install from source|Clone the repo and build coven with cargo.|Developing Coven or running unreleased changes" \
  "install/updating|Updating Coven|How to update Coven and what release channels exist.|Moving to a newer version of Coven" \
  "install/uninstall|Uninstalling Coven|How to remove Coven cleanly without losing project sessions.|Removing Coven from a workstation" \
  "install/development-channels|Development channels|Nightly, beta, and stable channel rules.|Following pre-release Coven builds" \
  "install/docker|Docker|Run the Coven daemon inside a Docker container.|Containerizing Coven for CI or homelab use" \
  "install/nix|Nix|Reproducible Coven environment with Nix flakes.|You use Nix to manage tooling" \
  "install/podman|Podman|Run Coven under Podman with rootless containers.|Daemonless container hosting" \
  "install/macos|macOS install|Install Coven on macOS via npm, Homebrew, or source.|Installing on macOS" \
  "install/linux|Linux install|Install Coven on common Linux distros.|Installing on Linux" \
  "install/windows|Windows install|Install Coven on native Windows.|Installing on Windows" \
  "install/wsl2|WSL2 install|Install Coven inside WSL2 for the full Unix-socket experience.|Installing on WSL2" \
  "install/raspberry-pi|Raspberry Pi|Run Coven on Raspberry Pi as a low-power home agent host.|Hosting Coven on a Pi" \
  "install/headless-server|Headless server|Install Coven on a headless Linux server with systemd.|Running Coven without a desktop" \
  "install/coven-home|COVEN_HOME layout|What lives under COVEN_HOME and how to relocate it.|Customizing where Coven keeps state" \
  "install/launchd|launchd service|Run the Coven daemon as a launchd user agent on macOS.|Keeping the daemon up on macOS" \
  "install/systemd|systemd unit|Run the Coven daemon as a systemd user unit.|Keeping the daemon up on Linux"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in with Coven-specific install steps. See [Install overview](/install/index) for the canonical layout."
done

# ---- harnesses/ ----
for pair in \
  "harnesses/index|Harnesses|Coven launches and supervises coding-agent CLIs through PTY adapters.|Introducing the harness concept" \
  "harnesses/what-is-a-harness|What is a harness?|A harness is an external coding-agent CLI that Coven launches inside a project root.|Explaining the difference between a familiar and a harness" \
  "harnesses/codex|Codex|Run OpenAI Codex CLI under Coven supervision.|Setting up Codex" \
  "harnesses/claude-code|Claude Code|Run Anthropic Claude Code under Coven supervision.|Setting up Claude Code" \
  "harnesses/hermes|Hermes (planned)|Adapter direction for Hermes when its CLI ships.|Tracking the Hermes adapter roadmap" \
  "harnesses/aider|Aider (planned)|Adapter direction for Aider once an adapter spec lands.|Tracking the Aider adapter roadmap" \
  "harnesses/gemini-cli|Gemini CLI (planned)|Adapter direction for Google's Gemini CLI.|Tracking the Gemini CLI adapter roadmap" \
  "harnesses/cline|Cline (planned)|Adapter direction for Cline.|Tracking the Cline adapter roadmap" \
  "harnesses/custom|Custom harness adapter|Build your own harness adapter against the Coven adapter spec.|Adding a new harness yourself" \
  "harnesses/installing|Installing harness CLIs|How Coven detects harness CLIs and what to install for each.|Resolving missing harness errors" \
  "harnesses/provider-auth|Provider auth boundary|Coven does not store provider credentials. Each harness keeps using its own login.|Auditing where credentials live" \
  "harnesses/project-root|Project root|The explicit boundary for a session — what it is and why it cannot be widened.|Understanding why launches reject" \
  "harnesses/working-directory|Working directory|Launch cwd must canonicalize inside the project root.|Choosing a cwd for a session" \
  "harnesses/title-and-metadata|Title and metadata|Readable titles, custom metadata, and how clients display them.|Naming sessions for humans" \
  "harnesses/troubleshooting|Harness troubleshooting|When a harness misbehaves under Coven supervision.|Debugging a misbehaving harness"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in. See [Harness adapters](/reference/harness-adapters) for the spec."
done

# ---- familiars/ ----
for pair in \
  "familiars/index|Familiars|Persistent named agents with memory, tools, identity, roles, and continuity.|Introducing the familiar concept" \
  "familiars/what-is-a-familiar|What is a familiar?|The OpenCoven concept layer above harnesses — named, memory-bearing agents.|Distinguishing familiars from raw harnesses" \
  "familiars/naming-and-voice|Naming and voice|Names, voices, and the brand promise of personal-not-pretending-human agents.|Designing a new familiar" \
  "familiars/roles|Roles|Roles a familiar can take inside a workflow.|Assigning a role to a familiar" \
  "familiars/personas|Personas|Persona definition, examples, and reuse across projects.|Reusing personas across familiars" \
  "familiars/identity|Identity|How identity is persisted across sessions, devices, and harness swaps.|Understanding familiar identity" \
  "familiars/multi-familiar|Multi-familiar|Running more than one familiar in a workspace.|Coordinating multiple familiars" \
  "familiars/handoff|Handoff (Phase 1)|Explicit transfer of a task plus full context from one familiar to another.|Designing a handoff between harnesses" \
  "familiars/orchestration|Orchestration (Phase 2)|Capability-based routing and load balancing across familiars.|Planning multi-harness orchestration" \
  "familiars/parallel-lanes|Parallel specialist lanes|Run specialist familiars in parallel on the same task.|Splitting work across familiars"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in. See [Familiars overview](/familiars)."
done

# ---- sessions/ ----
for pair in \
  "sessions/index|Sessions|A session is a Coven-owned record of one harness run.|Introducing sessions" \
  "sessions/lifecycle|Session lifecycle|Launch, run, attach, exit, archive, summon, sacrifice.|Understanding what happens across a session's life" \
  "sessions/events|Events|Append-only output/exit/metadata records keyed by session id.|Querying or replaying session events" \
  "sessions/comux-json|comux JSON sessions|The on-disk session format comux and external clients can consume.|Building a client that replays Coven sessions" \
  "sessions/compaction|Compaction|How long-running sessions condense without losing the audit trail.|Managing long-lived sessions"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in. See [Session lifecycle](/sessions/lifecycle)."
done

# ---- memory/ ----
for pair in \
  "memory/index|Memory overview|How familiars remember across sessions.|Choosing a memory backend" \
  "memory/working-memory|Working memory|In-session memory tied to a single PTY run.|Designing in-session prompts" \
  "memory/persistent-memory|Persistent memory|Cross-session memory that survives daemon restarts.|Giving a familiar continuity" \
  "memory/episodic|Episodic memory|Remembering specific events and turns.|Building recall over events" \
  "memory/semantic|Semantic memory|Embeddings and concept-based recall.|Adding semantic recall" \
  "memory/search|Memory search|Querying memory from a familiar or a client.|Surfacing memory results"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in. See [Memory overview](/memory)."
done

# ---- rituals/ ----
for pair in \
  "rituals/index|Rituals|Archive, summon, sacrifice — Coven's explicit verbs around destructive operations.|Introducing rituals" \
  "rituals/archive|Archive|Hide a non-running session without deleting events.|Cleaning up the active session list" \
  "rituals/summon|Summon|Restore an archived session to the active list.|Bringing back archived work" \
  "rituals/sacrifice|Sacrifice|Permanently delete a non-running session and its events.|Removing a session for good"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in. Rituals refuse live sessions; sacrifice requires \`--yes\`."
done

# ---- capabilities/, tools/, automation/, plugins/ ----
for pair in \
  "capabilities/index|Capabilities|Discoverable daemon and adapter features exposed via the control plane.|Introducing the capability system" \
  "capabilities/discovery|Capability discovery|GET /api/v1/capabilities, what is returned, and how to use it.|Building a client that reads capabilities" \
  "capabilities/action-routing|Action routing|POST /api/v1/actions, owned adapters, and fail-closed unknown ids.|Sending intents through Coven" \
  "tools/index|Tools overview|Built-in and adapter-supplied tools available to familiars.|Browsing what familiars can call" \
  "tools/exec|exec|Run shell commands inside a session's project root.|Calling shell commands from a familiar" \
  "tools/apply-patch|apply-patch|Apply unified diffs through a structured tool.|Using patch-shaped edits" \
  "tools/desktop-automation|Desktop automation|Coven-owned adapters for keyboard, mouse, window, and AppleScript.|Driving the desktop from a familiar" \
  "tools/web-fetch|web-fetch|Fetch a URL through a controlled adapter.|Reading the web from a familiar" \
  "tools/thinking|Thinking|Structured reasoning blocks in transcripts.|Reading what a familiar was thinking" \
  "tools/skills|Skills|Reusable named capabilities a familiar can invoke.|Reusing logic across familiars" \
  "tools/creating-skills|Creating skills|Author a new skill that lives inside a project.|Writing your first skill" \
  "tools/slash-commands|Slash commands|Slash commands in the TUI and how to extend them.|Adding a slash command" \
  "automation/index|Automation overview|Cron, hooks, and standing orders.|Choosing how to automate Coven work" \
  "automation/cron|Cron|Schedule recurring Coven sessions.|Setting up a recurring job" \
  "automation/hooks|Hooks|React to lifecycle events with hook scripts.|Writing a hook script" \
  "automation/standing-orders|Standing orders|Persistent intents a familiar acts on whenever conditions match.|Encoding a familiar's standing intent" \
  "plugins/index|Plugins overview|Bundled and external plugins that extend Coven.|Browsing the plugin surface" \
  "plugins/manage|Manage plugins|Install, enable, disable, and remove plugins.|Adjusting installed plugins" \
  "plugins/building-plugins|Building plugins|Write a plugin against Coven's plugin SDK.|Authoring a new plugin" \
  "plugins/openclaw-bridge|OpenClaw bridge|The external @opencoven/coven package that lets OpenClaw consume Coven.|Integrating with OpenClaw"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in."
done

# ---- models/ ----
for pair in \
  "models/index|Models overview|Coven does not own provider credentials. Each harness keeps using its own login.|Auditing where model credentials live" \
  "models/provider-boundary|Provider boundary|Why the harness, not Coven, holds provider auth.|Designing client/credential isolation" \
  "models/why-coven-does-not-own-credentials|Why Coven does not own credentials|The local-first rationale.|Explaining the credential boundary to a stakeholder" \
  "models/anthropic|Anthropic|Using Claude through Claude Code under Coven.|Connecting Anthropic" \
  "models/openai|OpenAI|Using OpenAI through Codex under Coven.|Connecting OpenAI" \
  "models/google|Google|Using Google models through the Gemini CLI adapter.|Connecting Google" \
  "models/local-models|Local models|Running with local-model backends.|Going fully offline"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in."
done

# ---- platforms/ ----
for pair in \
  "platforms/macos|macOS|Coven on macOS — launchd, accessibility prompts, and Unix-socket behavior.|Operating on macOS" \
  "platforms/linux|Linux|Coven on Linux — systemd, AppArmor/SELinux, and socket permissions.|Operating on Linux" \
  "platforms/windows|Windows|Coven on Windows — caveats and supported flows.|Operating on Windows" \
  "platforms/wsl2|WSL2|Recommended path for Windows users — Coven inside WSL2.|Bridging Coven into a Windows desktop" \
  "platforms/headless|Headless servers|Run Coven without a desktop, behind SSH or a Tailscale tunnel.|Hosting Coven on a server" \
  "platforms/raspberry-pi|Raspberry Pi|Notes on running Coven on Raspberry Pi.|Hosting on a Pi" \
  "platforms/cloud-vm|Cloud VM|Patterns for cloud-hosted Coven daemons.|Hosting on a cloud VM"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in."
done

# ---- daemon/ ----
for pair in \
  "daemon/index|Daemon overview|The Coven daemon is the Rank 0 authority for sessions, PTYs, and the socket API.|Introducing the daemon" \
  "daemon/lifecycle|Daemon lifecycle|start, status, restart, stop.|Managing the daemon process" \
  "daemon/health|Health|GET /api/v1/health and what every field means.|Building a health probe" \
  "daemon/configuration|Configuration|coven.toml, environment variables, and overrides.|Configuring the daemon" \
  "daemon/coven-home|COVEN_HOME|What lives under COVEN_HOME and how to relocate it.|Customizing where Coven keeps state" \
  "daemon/socket-api|Socket API|HTTP-over-Unix-socket contract under /api/v1.|Building a client" \
  "daemon/api-versioning|API versioning|Compatibility rules for clients.|Pinning your client to a Coven version" \
  "daemon/error-envelope|Error envelope|The structured {error: {code, message, details}} contract.|Handling errors in a client" \
  "daemon/capabilities-handshake|Capabilities handshake|Use GET /api/v1/health to negotiate apiVersion and capabilities before depending on response shapes.|Writing a client handshake" \
  "daemon/auth-posture|Auth posture|Same-user local access over the Unix socket. No daemon OAuth, JWTs, or browser cookies.|Auditing daemon auth" \
  "daemon/trust-boundary|Trust boundary|Clients can ask; only the daemon decides.|Documenting trust for a security review" \
  "daemon/remote-access|Remote access|Tailscale and SSH patterns for reaching a remote daemon.|Exposing Coven to another machine you own" \
  "daemon/safety-model|Safety model|Secret handling, socket posture, and automation approvals.|Understanding the safety promises" \
  "daemon/logs|Logs|Where the daemon writes logs and what it logs.|Reading daemon logs" \
  "daemon/diagnostics|Diagnostics|coven diagnostics, bundle generation, and what to include in a bug report.|Filing a useful issue" \
  "daemon/orphan-recovery|Orphan recovery|How sessions that lost their PTY are handled.|Cleaning up after a crash" \
  "daemon/upgrades|Upgrades|Safe upgrade flow for the daemon and store schema.|Upgrading without losing work"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in. See [Daemon overview](/daemon/index)."
done

# ---- reference/ ----
for pair in \
  "reference/api|API reference|Index of the local socket API.|Looking up an endpoint" \
  "reference/api-contract|API contract|The named coven.daemon.v1 contract under /api/v1.|Pinning a client to the contract" \
  "reference/api-sessions|Sessions endpoints|GET, POST, kill, input for sessions.|Looking up the sessions API" \
  "reference/api-events|Events endpoint|GET /api/v1/events for replay.|Looking up the events API" \
  "reference/api-capabilities|Capabilities endpoint|GET /api/v1/capabilities discovery.|Looking up the capabilities API" \
  "reference/api-actions|Actions endpoint|POST /api/v1/actions intent dispatch.|Looking up the actions API" \
  "reference/cli|CLI reference|Index of every coven command.|Looking up a CLI flag" \
  "reference/cli-coven|coven|The default interactive command.|Looking up coven" \
  "reference/cli-doctor|coven doctor|Environment readiness check.|Looking up doctor" \
  "reference/cli-daemon|coven daemon|Daemon lifecycle subcommands.|Looking up daemon subcommands" \
  "reference/cli-run|coven run|Launch a harness session inside a project root.|Looking up run" \
  "reference/cli-sessions|coven sessions|Browse, filter, and act on sessions.|Looking up sessions" \
  "reference/cli-attach|coven attach|Replay and follow a live session.|Looking up attach" \
  "reference/cli-archive|coven archive|Archive a non-running session.|Looking up archive" \
  "reference/cli-summon|coven summon|Restore an archived session.|Looking up summon" \
  "reference/cli-sacrifice|coven sacrifice|Permanently delete a non-running session.|Looking up sacrifice" \
  "reference/cli-patch|coven patch|The rescue loop, including the OpenClaw repair flow.|Looking up patch" \
  "reference/harness-adapters|Harness adapters|The adapter shape Codex and Claude Code use today.|Looking up adapter expectations" \
  "reference/adapter-spec|Adapter spec|What a new harness adapter must implement.|Authoring a new adapter" \
  "reference/future-harnesses|Future harnesses|Adapter direction after Codex and Claude Code.|Tracking the harness roadmap" \
  "reference/releasing|Releasing|Release flow for @opencoven/cli and platform packages.|Cutting a release" \
  "reference/roadmap|Roadmap|Public roadmap for Coven, comux, and integrations.|Reading the public roadmap" \
  "reference/changelog|Changelog|Per-release notes.|Looking up what changed" \
  "reference/glossary|Glossary|Short definitions for recurring product and architecture terms.|Looking up a term" \
  "reference/brand|Brand|Logo, palette, typography, and asset pack.|Using the OpenCoven brand correctly" \
  "reference/credits|Credits|Project origins and contributors.|Crediting the people behind Coven" \
  "reference/license|License|Coven is MIT licensed.|Checking license terms"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in."
done

# ---- help/ ----
for pair in \
  "help/index|Help overview|Common problems, environment, and how to ask for help.|Starting a troubleshooting session" \
  "help/troubleshooting|Troubleshooting|Common setup, daemon, harness, session, and API problems.|Diagnosing a broken Coven setup" \
  "help/daemon-wont-start|The daemon will not start|Socket binding, permission, and stale-pid checks.|The daemon refuses to start" \
  "help/harness-not-found|Harness not found|How Coven discovers harnesses and what to install.|coven run says the harness is missing" \
  "help/session-stuck|A session is stuck|Recovering live sessions that no longer respond.|A session stopped responding" \
  "help/environment|Environment variables|Every Coven-specific environment variable.|Looking up an env var" \
  "help/paths|Paths and state directories|Where Coven keeps state on each OS.|Auditing where Coven writes" \
  "help/permissions|Permissions|Socket permissions, accessibility prompts, and sandboxing notes.|Resolving permission errors" \
  "help/diagnostics-bundle|Diagnostics bundle|Generate a redacted diagnostics archive for an issue.|Filing a useful bug report" \
  "help/community|Community|Discord, X, and where to ask.|Asking for help" \
  "help/filing-issues|Filing issues|What to include in a Coven GitHub issue.|Filing a Coven issue"; do
  IFS='|' read -r path title summary when <<< "$pair"
  write_stub "$path" "$title" "$summary" "$when" "Stub — fill in."
done

# ---- announcements/ ----
write_stub announcements/comux-demo-loop.md "comux + Coven demo loop" \
  "The Coven-side CLI/API contract for the visible comux cockpit flow." \
  "Building or auditing the comux demo loop" \
"See \`docs/COMUX-DEMO-LOOP.md\` for the canonical demo-loop contract. This page summarises the moving parts for new contributors."

echo "Scaffold complete."
