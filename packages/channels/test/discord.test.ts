import assert from "node:assert/strict";
import { test } from "node:test";

import { DiscordConnector } from "../src/discord/index.ts";

function response(status: number, body = ""): Response {
  return new Response(body, { status });
}

test("DiscordConnector posts a channel message with bot authorization", async () => {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const connector = new DiscordConnector("secret-token", {
    fetch: async (url, init) => {
      calls.push({ url: String(url), init: init ?? {} });
      return response(200, "{}");
    },
  });

  await connector.send("123", { text: "hello coven" });

  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url, "https://discord.com/api/v10/channels/123/messages");
  assert.equal((calls[0]?.init.headers as Record<string, string>).Authorization, "Bot secret-token");
  assert.deepEqual(JSON.parse(String(calls[0]?.init.body)), { content: "hello coven" });
});

test("DiscordConnector resolves logical channel names before sending", async () => {
  const urls: string[] = [];
  const connector = new DiscordConnector("secret-token", {
    channels: { "coven-general": "456" },
    fetch: async (url) => {
      urls.push(String(url));
      return response(200, "{}");
    },
  });

  await connector.send("coven-general", { text: "hello mapped channel" });

  assert.deepEqual(urls, ["https://discord.com/api/v10/channels/456/messages"]);
});

test("DiscordConnector retries one Discord 5xx response", async () => {
  let attempts = 0;
  const connector = new DiscordConnector("secret-token", {
    sleep: async () => {},
    fetch: async () => {
      attempts += 1;
      return attempts === 1 ? response(502, "bad gateway") : response(200, "{}");
    },
  });

  await connector.send("123", { text: "retry me" });

  assert.equal(attempts, 2);
});

test("DiscordConnector retries one rate limit using retry-after", async () => {
  const sleeps: number[] = [];
  let attempts = 0;
  const connector = new DiscordConnector("secret-token", {
    sleep: async (ms) => {
      sleeps.push(ms);
    },
    fetch: async () => {
      attempts += 1;
      return attempts === 1
        ? new Response("rate limited", { status: 429, headers: { "retry-after": "0.25" } })
        : response(200, "{}");
    },
  });

  await connector.send("123", { text: "retry later" });

  assert.equal(attempts, 2);
  assert.deepEqual(sleeps, [250]);
});

test("DiscordConnector does not retry Discord 4xx errors", async () => {
  let attempts = 0;
  const connector = new DiscordConnector("secret-token", {
    fetch: async () => {
      attempts += 1;
      return response(401, "bad token");
    },
  });

  await assert.rejects(
    () => connector.send("123", { text: "auth failure" }),
    /Discord API error 401: bad token/,
  );
  assert.equal(attempts, 1);
});
