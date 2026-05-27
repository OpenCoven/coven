# Coven Channels — TECH

**Status:** Draft v1 · 2026-05-27
**Companion to:** [PRODUCT.md](./PRODUCT.md)

## Package location

```mermaid
graph TD
    root["coven/"]
    root --> packages["packages/"]
    packages --> channels["channels/"]
    channels --> pkgjson["package.json"]
    channels --> src["src/"]
    channels --> test["test/"]
    src --> index["index.ts"]
    src --> types["types.ts"]
    src --> discord["discord/"]
    discord --> di["index.ts"]
    discord --> format["format.ts"]
    discord --> auth["auth.ts"]
    test --> smoke["discord.smoke.ts"]
```

```
coven/
  packages/
    channels/
      package.json          # name: "@opencoven/channels"
      src/
        index.ts            # exports ChannelConnector, ChannelMessage, createConnector
        types.ts            # ChannelMessage, ChannelConnector interface
        discord/
          index.ts          # DiscordConnector class
          format.ts         # ChannelMessage → Discord API payload translation
          auth.ts           # token resolution (env → keychain → error)
      test/
        discord.smoke.ts    # smoke test: send to test channel
      docs/                 # symlink or source for coven/docs/channels/
```

The package is TypeScript (matching the existing `coven/packages/` conventions). It has no Rust dependency in v1 — it speaks directly to the Discord REST API over HTTPS. The Rust daemon does not mediate channel posts in v1; that's a future integration point if Coven needs to audit or throttle outbound messages.

## Types

```typescript
// src/types.ts

export interface ChannelMessage {
  text?: string;
  embed?: {
    title?: string;
    description?: string;
    color?: number;
    author?: {
      name: string;
      icon_url?: string;
    };
    fields?: Array<{ name: string; value: string; inline?: boolean }>;
    footer?: { text: string };
    timestamp?: string; // ISO 8601
  };
}

export interface ChannelConnector {
  send(channelId: string, message: ChannelMessage): Promise<void>;
}
```

## DiscordConnector

```typescript
// src/discord/index.ts
import { ChannelConnector, ChannelMessage } from '../types.js';
import { toDiscordPayload } from './format.js';
import { resolveToken } from './auth.js';

export class DiscordConnector implements ChannelConnector {
  private token: string;

  constructor(token: string) {
    this.token = token;
  }

  static async create(): Promise<DiscordConnector> {
    const token = await resolveToken();
    return new DiscordConnector(token);
  }

  async send(channelId: string, message: ChannelMessage): Promise<void> {
    const payload = toDiscordPayload(message);
    const res = await fetch(
      `https://discord.com/api/v10/channels/${channelId}/messages`,
      {
        method: 'POST',
        headers: {
          Authorization: `Bot ${this.token}`,
          'Content-Type': 'application/json',
          'User-Agent': 'OpenCovenChannels/1 (https://opencoven.dev)',
        },
        body: JSON.stringify(payload),
      }
    );
    if (!res.ok) {
      const body = await res.text().catch(() => '');
      throw new Error(`Discord API error ${res.status}: ${body}`);
    }
  }
}
```

## Payload translation

```typescript
// src/discord/format.ts
import { ChannelMessage } from '../types.js';

export interface DiscordPayload {
  content?: string;
  embeds?: DiscordEmbed[];
}

interface DiscordEmbed {
  title?: string;
  description?: string;
  color?: number;
  author?: { name: string; icon_url?: string };
  fields?: Array<{ name: string; value: string; inline?: boolean }>;
  footer?: { text: string };
  timestamp?: string;
}

export function toDiscordPayload(msg: ChannelMessage): DiscordPayload {
  const payload: DiscordPayload = {};
  if (msg.text) payload.content = msg.text;
  if (msg.embed) {
    payload.embeds = [msg.embed as DiscordEmbed];
  }
  return payload;
}
```

## Token resolution

```mermaid
flowchart TD
    A[resolveToken called] --> B{COVEN_DISCORD_TOKEN set?}
    B -->|Yes| C[Return env token]
    B -->|No| D{keychain available?}
    D -->|Yes| E{Secret in keychain?}
    E -->|Yes| F[Return keychain token]
    E -->|No| G[Throw: token not found]
    D -->|No| G
```

Token is **never** written to `daemon.json`, `coven.toml`, or any config file. Error messages never include the token value.

```typescript
// src/discord/auth.ts

export async function resolveToken(): Promise<string> {
  // 1. Explicit env var (CI, containers, dev override)
  if (process.env.COVEN_DISCORD_TOKEN) {
    return process.env.COVEN_DISCORD_TOKEN;
  }
  // 2. Keychain (macOS: security find-generic-password, Linux: libsecret via @opencoven/keychain if available)
  try {
    const { readSecret } = await import('@opencoven/keychain');
    const token = await readSecret('coven.discord.token');
    if (token) return token;
  } catch {
    // keychain not available; continue
  }
  throw new Error(
    'Discord bot token not found. Set COVEN_DISCORD_TOKEN or store it with: coven secrets set discord.token <token>'
  );
}
```

Token is **never** written to `daemon.json`, `coven.toml`, or any config file. Error messages never include the token value.

## Factory / public API

```typescript
// src/index.ts
export { ChannelConnector, ChannelMessage } from './types.js';
export { DiscordConnector } from './discord/index.js';

export type ConnectorKind = 'discord'; // extend as connectors are added

export async function createConnector(kind: ConnectorKind): Promise<ChannelConnector> {
  switch (kind) {
    case 'discord':
      return DiscordConnector.create();
    default:
      throw new Error(`Unknown connector kind: ${kind}`);
  }
}
```

## How a familiar uses this

```typescript
import { createConnector } from '@opencoven/channels';

const discord = await createConnector('discord');

await discord.send('CHANNEL_ID_HERE', {
  embed: {
    title: 'Weekly Open Coven — May 25',
    description: 'Here's what shipped this week in the Coven...',
    color: 0x8E3DFF,  // coven violet
    author: {
      name: 'Charm ✨',
      icon_url: 'https://opencoven.dev/avatars/charm.jpg',
    },
    footer: { text: 'OpenCoven · open-coven.dev' },
    timestamp: new Date().toISOString(),
  },
});
```

## Channel ID mapping (config)

Familiars reference channels by logical name, not raw Discord IDs. The mapping lives in `coven.toml`:

```toml
[channels.discord.channels]
coven-general   = "1234567890123456789"
coven-updates   = "9876543210987654321"
```

The connector resolves logical names before calling the API. Familiars never hardcode Discord snowflakes.

## Error handling

```mermaid
flowchart TD
    A[send called] --> B[POST to Discord API]
    B --> C{Response status}
    C -->|2xx| D[Success]
    C -->|4xx| E[Throw immediately]
    C -->|429| F[Read Retry-After header]
    F --> G[Wait]
    G --> H[Retry once]
    H --> I{Status}
    I -->|2xx| D
    I -->|429| J[Throw: rate limited]
    C -->|5xx| K[Wait 2s]
    K --> L[Retry once]
    L --> M{Status}
    M -->|2xx| D
    M -->|5xx| N[Throw: server error]
    B -->|Network error| O[Throw with cause]
```

- `4xx` Discord errors (bad token, missing permissions, unknown channel) → throw immediately with message; do not retry
- `429 Rate limit` → read `Retry-After` header, wait, retry once; if second attempt also 429, throw
- `5xx` Discord errors → retry once after 2s; if still failing, throw
- Network errors → throw with original cause

## Smoke test

```typescript
// test/discord.smoke.ts
// Run with: COVEN_DISCORD_TOKEN=... COVEN_TEST_CHANNEL_ID=... node --test test/discord.smoke.ts

import { createConnector } from '../src/index.js';

const channelId = process.env.COVEN_TEST_CHANNEL_ID;
if (!channelId) throw new Error('COVEN_TEST_CHANNEL_ID required');

const connector = await createConnector('discord');
await connector.send(channelId, {
  embed: {
    title: '🧪 Coven Channels smoke test',
    description: 'If you see this, the connector works.',
    color: 0x8E3DFF,
    author: { name: 'Charm ✨' },
    timestamp: new Date().toISOString(),
  },
});
console.log('✅ Smoke test passed');
```

## Future v2 extension points

```mermaid
graph LR
    subgraph v1 ["v1 - Outbound only"]
        send1["connector.send()"] --> REST1["Discord REST API"]
    end
    subgraph v2 ["v2 - Bidirectional"]
        send2["connector.send()"] --> REST2["Discord REST API"]
        GW["Discord Gateway WebSocket"] --> listen2["connector.listen()"]
        listen2 --> router["Familiar router (daemon)"]
    end
    send1 -.->|non-breaking extension| send2
```

When bidirectional support arrives, `ChannelConnector` gains:

```typescript
listen(channelId: string, handler: (event: ChannelEvent) => void): () => void;
```

`DiscordConnector` will open a Discord Gateway WebSocket connection. The existing `send` implementation is unchanged. The `createConnector` factory returns the same type — callers that don't call `listen` are unaffected.

Familiar routing (which familiar handles which mentions) is a higher-level concern — it lives above the connector layer, likely in the Coven daemon's familiar orchestration logic.

## Dependencies

- `node-fetch` or native `fetch` (Node ≥18 has it built-in)
- `@opencoven/keychain` (optional peer dep, for keychain token resolution)
- No Rust FFI, no daemon dependency in v1

## package.json sketch

```json
{
  "name": "@opencoven/channels",
  "version": "0.1.0",
  "description": "Harness-agnostic channel connectors for OpenCoven familiars",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc",
    "test:smoke": "node --test test/discord.smoke.ts"
  },
  "license": "MIT"
}
```
