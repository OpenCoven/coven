---
title: "Telegram setup checklist"
summary: "BotFather, 1Password references, target mapping, and smoke-test setup for @opencoven/channels Telegram."
read_when:
  - Creating the Telegram bot for @opencoven/channels
  - Preparing 1Password references for Telegram outbound delivery
  - Migrating Telegram delivery from OpenClaw to OpenCoven
description: "End-to-end setup checklist for the Coven Telegram outbound connector, including BotFather setup, target refs, security boundaries, smoke testing, and OpenClaw migration notes."
---

# Telegram setup checklist

Use this when creating or auditing the Telegram bot used by
`@opencoven/channels`. The v1 connector is outbound-only: it sends messages
through the Telegram Bot API. It does not receive Telegram updates or replace
OpenClaw's full Telegram runtime yet.

## What Coven needs

- A Telegram bot token from BotFather.
- A 1Password reference for the bot token.
- A 1Password reference for each private smoke-test or production target ID.
- Optional local-only logical target mapping for familiar-friendly names such as
  `val-dm` or `release-updates`.

## BotFather setup

1. Open Telegram and chat with BotFather.
2. Create or choose a bot.
3. Copy the token only long enough to store it in 1Password.
4. Do not paste the token into chat, docs, commits, shell history, or config
   files.

For one-owner Coven bots, keep the bot private in practice by not publishing the
bot username and by using fail-closed allowlists when inbound support lands.

## 1Password references

Recommended 1Password fields:

- `token`: Telegram bot token.
- `test-chat-id`: private smoke-test target ID.
- Optional named target IDs such as `val-dm-chat-id` or `release-chat-id`.

Only references should leave 1Password:

```sh
export COVEN_TELEGRAM_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_TELEGRAM_TEST_TARGET_REF='op://VAULT/ITEM/test-chat-id'
```

Do not use raw `TELEGRAM_BOT_TOKEN` examples in Coven docs or scripts. Use
`COVEN_TELEGRAM_TOKEN_REF` so runtime code resolves the secret internally.

## Target mapping

Telegram direct messages, groups, and topics use numeric target IDs. Keep those
IDs out of docs and commits. Resolve them through 1Password and pass logical
names to familiars:

```ts
const telegram = await createConnector("telegram", {
  targets: {
    "val-dm": await readOnePasswordReference("op://VAULT/ITEM/val-dm-chat-id"),
  },
});
```

For group/topic migration, preserve OpenClaw session-key semantics in the
compatibility plan before moving inbound routing.

## Smoke test

Run from a shell that has references, not raw values:

```sh
cd packages/channels
export COVEN_TELEGRAM_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_TELEGRAM_TEST_TARGET_REF='op://VAULT/ITEM/test-chat-id'
npm run test:smoke
```

Expected result:

- One harmless message appears in the private test target.
- The terminal prints only that the Telegram smoke message was sent.

If either reference is missing, the Telegram smoke test skips.

## OpenClaw migration guardrails

OpenClaw currently owns the mature Telegram runtime behavior. Keep it as the
fallback while OpenCoven adds native pieces.

Do not disable OpenClaw Telegram until all of these are verified:

- OpenCoven outbound smoke passes.
- OpenCoven inbound DM routing exists and is fail-closed.
- Allowlist behavior matches the intended one-owner policy.
- Cave transcript attribution still shows the human and `telegram` source
  cleanly.
- A rollback path is documented and tested.

## Troubleshooting

`Telegram bot token reference not found`

Set `COVEN_TELEGRAM_TOKEN_REF` to a valid 1Password reference.

`1Password reference resolved to an empty value`

The reference exists but the field is empty or unreadable. Check the 1Password
item and CLI account.

`Telegram API error 401`

The bot token is invalid or expired. Reset the token in BotFather and update
the 1Password item.

`Telegram API error 403`

The bot cannot message the target. For DMs, start the bot conversation from the
human account first. For groups, check that the bot is a member and has posting
permission.
