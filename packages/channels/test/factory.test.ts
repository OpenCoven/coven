import assert from "node:assert/strict";
import { test } from "node:test";

import { DiscordConnector, TelegramConnector, createConnector } from "../src/index.ts";

test("createConnector creates a Discord connector", async () => {
  const connector = await createConnector("discord", {
    env: { COVEN_DISCORD_TOKEN_REF: "op://vault/item/token" },
    readOnePassword: async () => "bot-token",
  });

  assert.ok(connector instanceof DiscordConnector);
});

test("createConnector creates a Telegram connector", async () => {
  const connector = await createConnector("telegram", {
    env: { COVEN_TELEGRAM_TOKEN_REF: "op://vault/item/token" },
    readOnePassword: async () => "telegram-token",
  });

  assert.ok(connector instanceof TelegramConnector);
});

test("createConnector rejects unknown connector kinds", async () => {
  await assert.rejects(
    () =>
      createConnector("slack" as "discord", {
        env: { COVEN_DISCORD_TOKEN_REF: "op://vault/item/token" },
        readOnePassword: async () => "bot-token",
      }),
    /Unknown connector kind: slack/,
  );
});
