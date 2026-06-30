import assert from "node:assert/strict";
import { test } from "node:test";

import { resolveTelegramToken } from "../src/telegram/auth.ts";

test("resolveTelegramToken reads the Telegram token from a 1Password reference", async () => {
  const token = await resolveTelegramToken({
    env: { COVEN_TELEGRAM_TOKEN_REF: "op://vault/item/token" },
    readOnePassword: async (reference) =>
      reference === "op://vault/item/token" ? "telegram-token" : null,
  });

  assert.equal(token, "telegram-token");
});

test("resolveTelegramToken accepts an explicit 1Password token reference", async () => {
  const token = await resolveTelegramToken({
    env: {},
    tokenRef: "op://vault/item/token",
    readOnePassword: async (reference) =>
      reference === "op://vault/item/token" ? "stored-telegram-token" : null,
  });

  assert.equal(token, "stored-telegram-token");
});

test("resolveTelegramToken fails without exposing token material or secret references", async () => {
  await assert.rejects(
    () =>
      resolveTelegramToken({
        env: {},
        readOnePassword: async () => null,
      }),
    /Telegram bot token reference not found\. Set COVEN_TELEGRAM_TOKEN_REF\./,
  );
});
