---
title: "Telegram channel connector"
summary: "How to post to Telegram from any OpenCoven familiar using @opencoven/channels."
read_when:
  - Setting up a familiar to post to Telegram
  - Migrating Telegram delivery from OpenClaw to OpenCoven
description: "Guide to the @opencoven/channels Telegram connector: bot setup, 1Password references, outbound delivery, and migration boundaries."
---

# Telegram channel connector

`@opencoven/channels` provides an outbound Telegram connector for OpenCoven
familiars. Telegram v1 sends messages through the Telegram Bot API. It does not
receive updates, run pairing, approve commands, manage groups, or replace the
full OpenClaw Telegram runtime yet.

Use this connector for safe outbound delivery first. Keep OpenClaw Telegram as
the inbound and compatibility fallback until OpenCoven has explicit inbound
routing, allowlists, transcript attribution, and rollback checks.

## Setup

For the full BotFather, 1Password, target mapping, and smoke-test checklist,
see [Telegram setup checklist](./telegram-setup.md).

### 1. Create or choose the Telegram bot

Use BotFather to create a bot token. Store it in 1Password immediately. Do not
paste it into chat, docs, commits, shell history, or config files.

### 2. Store references only

Expose only 1Password references to Coven:

```sh
export COVEN_TELEGRAM_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_TELEGRAM_TEST_TARGET_REF='op://VAULT/ITEM/test-chat-id'
```

### 3. Map logical targets

Familiars should send to logical target names rather than raw chat IDs:

```ts
import { createConnector, readOnePasswordReference } from "@opencoven/channels";

const telegram = await createConnector("telegram", {
  targets: {
    "val-dm": await readOnePasswordReference("op://VAULT/ITEM/val-dm-chat-id"),
  },
});

await telegram.send("val-dm", {
  embed: {
    title: "Weekly Open Coven",
    description: "Here is what shipped this week.",
    author: { name: "Charm" },
    timestamp: new Date().toISOString(),
  },
});
```

Telegram does not support Discord-style embeds, so the connector renders embed
fields into readable plain text and chunks long messages below Telegram limits.

## Smoke test

```sh
cd packages/channels
export COVEN_TELEGRAM_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_TELEGRAM_TEST_TARGET_REF='op://VAULT/ITEM/test-chat-id'
npm run test:smoke
```

The smoke test sends one harmless message and prints no token, target ID, or
resolved 1Password value. It skips when references are missing.

## Migration from OpenClaw

OpenClaw remains the reference implementation for mature Telegram behavior:

- direct-message policy and pairing;
- allowlists;
- groups and forum topics;
- native command handling;
- inbound transcript/session metadata;
- operator approvals.

The OpenCoven migration path is:

1. OpenCoven owns outbound Telegram sends.
2. Cave/Coven Board docs and settings show Telegram as an OpenCoven channel
   connection.
3. OpenCoven inventories OpenClaw compatibility semantics without copying live
   secrets or IDs.
4. OpenCoven adds fail-closed inbound DM polling.
5. Groups/topics and richer runtime behavior follow after direct-message parity.

Do not disable OpenClaw Telegram until OpenCoven inbound routing, allowlist
behavior, transcript attribution, and rollback checks are verified.

## Troubleshooting

| Error | Fix |
|---|---|
| `Telegram bot token reference not found` | Set `COVEN_TELEGRAM_TOKEN_REF` to a 1Password reference |
| `1Password reference could not be read` | Unlock 1Password CLI or fix the reference |
| `Telegram API error 401` | Token is invalid or expired; reset it in BotFather and update 1Password |
| `Telegram API error 403` | The bot cannot message the target; start the bot DM or check group permissions |
| Message is split | Long messages are chunked intentionally to stay below Telegram text limits |
