import assert from "node:assert/strict";
import { test } from "node:test";

import { TelegramConnector } from "../src/telegram/index.ts";

function response(status: number, body = ""): Response {
  return new Response(body, { status, headers: { "content-type": "application/json" } });
}

test("TelegramConnector sends a channel message with bot authorization in the API URL", async () => {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const connector = new TelegramConnector("secret-token", {
    fetch: async (url, init) => {
      calls.push({ url: String(url), init: init ?? {} });
      return response(200, JSON.stringify({ ok: true, result: { message_id: 1 } }));
    },
  });

  await connector.send("123", { text: "hello coven" });

  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url, "https://api.telegram.org/botsecret-token/sendMessage");
  assert.deepEqual(JSON.parse(String(calls[0]?.init.body)), {
    chat_id: "123",
    text: "hello coven",
    disable_web_page_preview: true,
  });
});

test("TelegramConnector resolves logical target names before sending", async () => {
  const bodies: unknown[] = [];
  const connector = new TelegramConnector("secret-token", {
    targets: { "val-dm": "456" },
    fetch: async (_url, init) => {
      bodies.push(JSON.parse(String(init?.body)));
      return response(200, JSON.stringify({ ok: true }));
    },
  });

  await connector.send("val-dm", { text: "hello mapped target" });

  assert.deepEqual(bodies, [
    {
      chat_id: "456",
      text: "hello mapped target",
      disable_web_page_preview: true,
    },
  ]);
});

test("TelegramConnector chunks long messages into multiple sends", async () => {
  const bodies: Array<{ text: string }> = [];
  const connector = new TelegramConnector("secret-token", {
    fetch: async (_url, init) => {
      bodies.push(JSON.parse(String(init?.body)));
      return response(200, JSON.stringify({ ok: true }));
    },
  });

  await connector.send("123", { text: "a".repeat(8200) });

  assert.equal(bodies.length, 3);
  assert.equal(bodies.map((body) => body.text).join(""), "a".repeat(8200));
});

test("TelegramConnector errors redact bot tokens and API URLs", async () => {
  const connector = new TelegramConnector("secret-token", {
    fetch: async () =>
      response(
        401,
        "unauthorized via https://api.telegram.org/botsecret-token/sendMessage",
      ),
  });

  await assert.rejects(
    () => connector.send("123", { text: "auth failure" }),
    (error) =>
      error instanceof Error &&
      /Telegram API error 401/.test(error.message) &&
      !error.message.includes("secret-token") &&
      !error.message.includes("api.telegram.org/bot"),
  );
});
