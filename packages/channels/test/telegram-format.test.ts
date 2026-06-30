import assert from "node:assert/strict";
import { test } from "node:test";

import { TELEGRAM_TEXT_LIMIT, toTelegramTexts } from "../src/telegram/format.ts";

test("toTelegramTexts translates text and embeds into readable Telegram text", () => {
  const texts = toTelegramTexts({
    text: "Release shipped.",
    embed: {
      title: "Coven release",
      description: "New build is available.",
      author: {
        name: "Charm",
        icon_url: "https://opencoven.ai/avatars/charm.png",
      },
      fields: [{ name: "Version", value: "0.1.0", inline: true }],
      footer: { text: "OpenCoven" },
      timestamp: "2026-06-28T00:00:00.000Z",
    },
  });

  assert.deepEqual(texts, [
    [
      "Release shipped.",
      "",
      "Coven release",
      "New build is available.",
      "",
      "Version: 0.1.0",
      "",
      "OpenCoven",
      "Charm",
      "2026-06-28T00:00:00.000Z",
    ].join("\n"),
  ]);
});

test("toTelegramTexts chunks long messages below Telegram text limits", () => {
  const longText = "a".repeat(TELEGRAM_TEXT_LIMIT * 2 + 25);
  const texts = toTelegramTexts({ text: longText });

  assert.equal(texts.length, 3);
  assert.ok(texts.every((text) => text.length <= TELEGRAM_TEXT_LIMIT));
  assert.equal(texts.join(""), longText);
});

test("toTelegramTexts rejects an empty message", () => {
  assert.throws(() => toTelegramTexts({}), /ChannelMessage must include text or embed/);
});
