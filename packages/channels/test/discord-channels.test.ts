import assert from "node:assert/strict";
import { test } from "node:test";

import { resolveDiscordChannelIdByName } from "../src/discord/channels.ts";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

test("resolveDiscordChannelIdByName finds one accessible channel by name", async () => {
  const calls: string[] = [];

  const channelId = await resolveDiscordChannelIdByName("secret-token", "coven-smoke-test", {
    fetch: async (url) => {
      calls.push(String(url));
      if (String(url).endsWith("/users/@me/guilds")) {
        return jsonResponse(200, [{ id: "guild-1", name: "Coven Test" }]);
      }

      return jsonResponse(200, [
        { id: "channel-1", name: "general", type: 0 },
        { id: "channel-2", name: "coven-smoke-test", type: 0 },
      ]);
    },
  });

  assert.equal(channelId, "channel-2");
  assert.deepEqual(calls, [
    "https://discord.com/api/v10/users/@me/guilds",
    "https://discord.com/api/v10/guilds/guild-1/channels",
  ]);
});

test("resolveDiscordChannelIdByName rejects ambiguous accessible channel names", async () => {
  await assert.rejects(
    () =>
      resolveDiscordChannelIdByName("secret-token", "#coven-smoke-test", {
        fetch: async (url) => {
          if (String(url).endsWith("/users/@me/guilds")) {
            return jsonResponse(200, [{ id: "guild-1" }, { id: "guild-2" }]);
          }

          return jsonResponse(200, [{ id: String(url), name: "coven-smoke-test", type: 0 }]);
        },
      }),
    /Discord channel name "coven-smoke-test" matched 2 accessible channels/,
  );
});

test("resolveDiscordChannelIdByName rejects missing accessible channel names", async () => {
  await assert.rejects(
    () =>
      resolveDiscordChannelIdByName("secret-token", "coven-smoke-test", {
        fetch: async (url) => {
          if (String(url).endsWith("/users/@me/guilds")) {
            return jsonResponse(200, [{ id: "guild-1" }]);
          }

          return jsonResponse(200, [{ id: "channel-1", name: "general", type: 0 }]);
        },
      }),
    /Discord channel name "coven-smoke-test" was not found in accessible guild channels/,
  );
});
