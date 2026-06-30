# @opencoven/channels

Harness-agnostic channel connectors for OpenCoven familiars.

The first connectors are Discord and Telegram. They send outbound messages
through bot APIs, read bot tokens through 1Password references, and let callers
use either raw target IDs or logical target names.

## Install

```sh
npm install @opencoven/channels
```

## Use

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

Telegram uses the same envelope and degrades embed content into readable text:

```ts
const telegram = await createConnector("telegram", {
  targets: {
    "val-dm": await readOnePasswordReference("op://VAULT/ITEM/val-dm-chat-id"),
  },
});

await telegram.send("val-dm", {
  text: "Coven Channels Telegram smoke test",
});
```

## Configuration

Store bot tokens in 1Password and expose only references:

```sh
export COVEN_DISCORD_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_TELEGRAM_TOKEN_REF='op://VAULT/ITEM/token'
```

## Test

```sh
npm test
npm run build
```

To send real smoke messages:

```sh
export COVEN_DISCORD_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_DISCORD_TEST_CHANNEL_NAME='coven-smoke-test'
export COVEN_TELEGRAM_TOKEN_REF='op://VAULT/ITEM/token'
export COVEN_TELEGRAM_TEST_TARGET_REF='op://VAULT/ITEM/test-chat-id'
npm run test:smoke
```

The smoke tests skip when their references are missing. Alternatively, set
`COVEN_DISCORD_TEST_CHANNEL_REF` to a 1Password reference containing the private
Discord test channel ID.
