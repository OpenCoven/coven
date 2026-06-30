---
title: "Discord setup checklist"
summary: "Discord Developer Portal, OAuth2, bot permissions, 1Password references, and smoke-test setup for @opencoven/channels."
read_when:
  - Creating the Discord bot for @opencoven/channels
  - Configuring OAuth2 scopes, bot permissions, and privileged intents
  - Preparing 1Password references for the Discord connector
description: "End-to-end setup checklist for the Coven Discord connector, including Discord application settings, OAuth2 install URL permissions, channel access, 1Password-backed secrets, and smoke-test verification."
---

# Discord setup checklist

Use this when creating or auditing the Discord bot used by `@opencoven/channels`.
The Coven connector is harness-agnostic and v1 is outbound-only: it sends
messages through the Discord Bot API. Do not paste the bot token or private
channel IDs into chat, docs, shell history, or committed config. Store live
values in 1Password and pass only `op://...` references to Coven.

## What Coven needs

- A Discord application with a bot user.
- The bot invited to the target server.
- Channel-level permission to see and send messages in the target channels.
- A 1Password reference for the bot token.
- A 1Password reference for any live smoke-test channel ID.
- Optional local-only logical channel mapping for familiar-friendly names such as `coven-general`.

## Discord application

1. Open the [Discord Developer Portal](https://discord.com/developers/applications).
2. Create a new application or select the existing OpenCoven application.
3. Set the application name, icon, and description so the server audit log is recognizable.
4. Open **Bot**.
5. Create the bot if it does not exist.
6. Set the bot username and avatar.
7. Generate or reset the bot token.
8. Store the token in 1Password immediately. Do not put it in a terminal command, issue, PR, chat message, or config file.

## Bot settings

Use the narrowest settings that support the current phase.

Required for v1 outbound posting:

- **Bot user:** enabled.
- **Public Bot:** off unless the bot is meant to be installable by other servers.
- **Requires OAuth2 Code Grant:** off for normal bot invites.
- **Token:** reset/copy only during setup, immediately store it in 1Password,
  and never paste it into shell history, docs, commits, or chat.
- **Authorization Flow:** no custom authorization flow is needed for v1.
- **Privileged Gateway Intents:** leave off unless one of the later features below
  explicitly needs the corresponding event stream.

Privileged Gateway Intents:

- **Message Content Intent:** not required for v1 outbound-only posting. Enable for v2 inbound message routing, mention handling, or any feature that reads message content.
- **Server Members Intent:** optional. Enable only when Coven needs member lists, role-based allowlists, or name-to-ID matching.
- **Presence Intent:** optional. Leave off unless a feature explicitly needs presence events.

Discord treats Message Content, Server Members, and Presence as privileged intents. Apps above Discord's review threshold may need review/approval before those intents can be used.

## OAuth2 install URL

Open **Installation** in the Developer Portal first.

Installation contexts:

- **Guild Install:** enable this for Coven v1. This is the context that installs
  the bot into a server and grants the bot channel permissions.
- **User Install:** leave this off for the v1 outbound connector. User-installed
  apps are for user-scoped app commands and do not install the bot into a server
  or grant it permission to post in guild channels.

If this bot later grows user-scoped slash commands, enable **User Install** then
and set its default scope to `applications.commands` only.

Install Link:

- **None** is the right choice for a private Coven-owned bot. This prevents a public
  **Add App** button on the bot profile or App Directory entry.
- Do not use **Discord Provided Link** for the v1 private connector; it is meant
  for apps that should expose Discord's default installation flow from the app
  profile.
- Do not use **Custom URL** unless Coven later has a hosted install/onboarding
  page. For one-time guild installation, use the generated OAuth2 authorize URL
  below instead.

Then open **OAuth2 → URL Generator**.

Redirects:

- Leave **OAuth2 → Redirects** empty for the v1 outbound connector.
- Do not configure a callback URL just to install the bot. Discord's bot
  authorization flow is callback-less for `bot`-only installs because Coven does
  not request or exchange a user's OAuth access token.
- Do not enable **Requires OAuth2 Code Grant** for this connector.
- Add redirect URIs only for a future hosted OAuth flow that requests user data
  scopes such as `identify`, `guilds`, or `guilds.members.read`. That future flow
  must use `state` validation and must keep the client secret in 1Password.

Scopes:

- `bot` is required.
- `applications.commands` is optional for the current Coven connector. Add it only if this bot will also expose slash commands or command-style interactions.

Recommended bot permissions for v1:

- **View Channels**: required so the bot can see the target channels.
- **Send Messages**: required for normal text-channel posts.
- **Send Messages in Threads**: required if posting inside public/private/forum/media threads.
- **Embed Links**: required for the embed-based familiar identity presentation.
- **Read Message History**: recommended for threaded/contextual workflows and future inbound work.

The generated OAuth2 URL should include these effective parameters:

- `scope=bot`
- `permissions=274877991936`

The `permissions` value above is the Discord bitfield for the recommended v1
set: View Channels, Send Messages, Send Messages in Threads, Embed Links, and
Read Message History. Prefer selecting the named permissions in Discord's URL
Generator and use the integer only as a sanity check.

Optional permissions for planned features:

- **Attach Files**: only if messages will include uploaded files.
- **Add Reactions**: only if acknowledgement or reaction workflows are used.
- **Use External Emoji** and **Use External Stickers**: only if message formatting uses them.
- **Create Public Threads** or **Create Private Threads**: only if Coven will create threads instead of posting into existing channels.
- **Manage Messages**, **Manage Threads**, **Manage Channels**, **Manage Roles**, **Administrator**: do not grant for the v1 outbound connector. Add only for a separately reviewed moderation/admin feature.

Copy the generated OAuth2 URL, open it in a browser, select the server, and install the bot.

## Server and channel permissions

After installing the bot:

1. Confirm the bot role is present in **Server Settings → Roles**.
2. Place the bot role high enough to see and post in the target channels, but do not grant broad admin privileges.
3. For each target channel, open **Edit Channel → Permissions**.
4. Confirm the bot role is allowed to **View Channel**, **Send Messages**, and **Embed Links**.
5. For thread targets, confirm **Send Messages in Threads** on the parent channel and thread.
6. For private channels, explicitly add the bot role or bot user to the channel permissions.
7. Send a manual test message from Discord as a human only after the bot role is visible in the member list.

If a channel denies permissions to `@everyone`, make sure the bot role has an explicit allow for the permissions above.

## 1Password references

Create a 1Password item for the Discord bot setup. Recommended fields:

- `token`: Discord bot token.
- `test-channel-id`: private smoke-test channel ID.
- Optional fields such as `application-id`, `guild-id`, and named production channel IDs.

Only references should leave 1Password:

```sh
export COVEN_DISCORD_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_DISCORD_TEST_CHANNEL_REF='op://VAULT/ITEM/test-channel-id'
```

For local smoke tests, you may use a channel name instead of a channel ID
reference:

```sh
export COVEN_DISCORD_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_DISCORD_TEST_CHANNEL_NAME='coven-smoke-test'
```

The channel-name path asks Discord for the bot's accessible guilds/channels and
uses the matched channel ID only in memory. If more than one accessible channel
has that name, use a 1Password channel ID reference instead.

To verify plain `op://VAULT/ITEM/field` references without printing values:

```sh
op read "$COVEN_DISCORD_TOKEN_REF" >/dev/null
op read "$COVEN_DISCORD_TEST_CHANNEL_REF" >/dev/null
```

For item titles that need CLI field lookup, such as titles containing a pipe,
use the package smoke test instead; it resolves those references internally
without printing the token or channel ID. If either check fails, unlock
1Password CLI or fix the reference. Do not run a command that prints the token
or channel ID.

## Coven package installation

Inside a Coven source checkout:

```sh
cd packages/channels
npm ci
npm run build
npm test
```

For package consumers after publish:

```sh
npm install @opencoven/channels
```

## Coven configuration

The connector can send to a raw channel ID value, but familiars should prefer logical names. Keep private channel IDs in local-only config or resolve them from 1Password in the calling layer.

Example local config shape:

```toml
[channels.discord]
enabled = true
token_ref = "op://VAULT/ITEM/token"

[channels.discord.channels]
coven-general = "op://VAULT/ITEM/coven-general-channel-id"
coven-updates = "op://VAULT/ITEM/coven-updates-channel-id"
```

When calling the package directly, pass resolved logical mappings to the connector:

```ts
import { createConnector, readOnePasswordReference } from "@opencoven/channels";

const discord = await createConnector("discord", {
  channels: {
    "coven-general": await readOnePasswordReference(
      "op://VAULT/ITEM/coven-general-channel-id",
    ),
  },
});

await discord.send("coven-general", {
  embed: {
    title: "Weekly Open Coven",
    description: "Here is what shipped this week.",
    color: 0x8e3dff,
    author: {
      name: "Charm",
      icon_url: "https://opencoven.ai/avatars/charm.png",
    },
    timestamp: new Date().toISOString(),
  },
});
```

## Smoke test

Run the smoke test only from a shell that has 1Password references, not raw values:

```sh
cd packages/channels
export COVEN_DISCORD_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_DISCORD_TEST_CHANNEL_NAME='coven-smoke-test'
npm run test:smoke
```

If the channel name is ambiguous, set `COVEN_DISCORD_TEST_CHANNEL_REF` to a
1Password reference containing the private channel ID instead. The smoke test
resolves secrets internally and does not print the resolved token or channel ID.

Expected result:

- A test embed appears in the private smoke-test channel.
- The terminal prints only that the smoke message was sent.

## Troubleshooting

`Discord bot token reference not found`

Set `COVEN_DISCORD_TOKEN_REF` to a valid 1Password reference.

`1Password reference resolved to an empty value`

The reference exists but the field is empty or unreadable. Check the 1Password item and CLI account.

`op: command not found`

Install and sign in to the 1Password CLI, then rerun the smoke test.

`401 Unauthorized`

The bot token is invalid or expired. Reset the token in the Developer Portal and update the 1Password item.

`403 Missing Permissions`

The bot is installed but lacks permission in the target channel. Check role order and channel permission overwrites.

`404 Unknown Channel`

The target channel reference is wrong, the bot is not in that server, or the bot cannot view the channel.

Message posts without familiar identity

Use an embed `author` field with the familiar display name and avatar URL.

## References

- [Discord Developer Portal](https://discord.com/developers/applications)
- [Discord permissions documentation](https://docs.discord.com/developers/topics/permissions)
- [Discord privileged intents support article](https://support-dev.discord.com/hc/en-us/articles/6207308062871-What-are-Privileged-Intents) <!-- external reference URL, not a secret -->
- [Discord API reference](https://docs.discord.com/developers/reference)
- [1Password CLI `op read`](https://developer.1password.com/docs/cli/reference/commands/read/)
