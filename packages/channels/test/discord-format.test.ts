import assert from "node:assert/strict";
import { test } from "node:test";

import { toDiscordPayload } from "../src/discord/format.ts";

test("toDiscordPayload translates text and embeds", () => {
  const payload = toDiscordPayload({
    text: "Release shipped.",
    embed: {
      title: "Coven release",
      description: "New build is available.",
      color: 0x8e3dff,
      author: {
        name: "Charm",
        icon_url: "https://opencoven.ai/avatars/charm.png",
      },
      fields: [{ name: "Version", value: "0.1.0", inline: true }],
      footer: { text: "OpenCoven" },
      timestamp: "2026-06-28T00:00:00.000Z",
    },
  });

  assert.deepEqual(payload, {
    content: "Release shipped.",
    embeds: [
      {
        title: "Coven release",
        description: "New build is available.",
        color: 0x8e3dff,
        author: {
          name: "Charm",
          icon_url: "https://opencoven.ai/avatars/charm.png",
        },
        fields: [{ name: "Version", value: "0.1.0", inline: true }],
        footer: { text: "OpenCoven" },
        timestamp: "2026-06-28T00:00:00.000Z",
      },
    ],
  });
});

test("toDiscordPayload rejects an empty message", () => {
  assert.throws(() => toDiscordPayload({}), /ChannelMessage must include text or embed/);
});
