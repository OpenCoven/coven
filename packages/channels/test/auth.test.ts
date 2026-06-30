import assert from "node:assert/strict";
import { test } from "node:test";

import { resolveToken } from "../src/discord/auth.ts";

test("resolveToken reads the Discord token from a 1Password reference", async () => {
  const token = await resolveToken({
    env: { COVEN_DISCORD_TOKEN_REF: "op://vault/item/token" },
    readOnePassword: async (reference) =>
      reference === "op://vault/item/token" ? "bot-token" : null,
  });

  assert.equal(token, "bot-token");
});

test("resolveToken accepts an explicit 1Password token reference", async () => {
  const token = await resolveToken({
    env: {},
    tokenRef: "op://vault/item/token",
    readOnePassword: async (reference) =>
      reference === "op://vault/item/token" ? "stored-token" : null,
  });

  assert.equal(token, "stored-token");
});

test("resolveToken fails without exposing token material or secret references", async () => {
  await assert.rejects(
    () =>
      resolveToken({
        env: {},
        readOnePassword: async () => null,
      }),
    /Discord bot token reference not found\. Set COVEN_DISCORD_TOKEN_REF\./,
  );
});
