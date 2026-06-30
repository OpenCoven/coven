import {
  DiscordConnector,
  readOnePasswordReference,
  resolveDiscordChannelIdByName,
  resolveOnePasswordReference,
} from "../src/index.ts";

const tokenReference = resolveOnePasswordReference("COVEN_DISCORD_TOKEN_REF");
const testChannelReference = resolveOnePasswordReference("COVEN_DISCORD_TEST_CHANNEL_REF");
const testChannelName = process.env.COVEN_DISCORD_TEST_CHANNEL_NAME?.trim() || null;

if (!tokenReference || (!testChannelReference && !testChannelName)) {
  console.log("1Password Discord smoke references not set; skipping Discord smoke test.");
  process.exit(0);
}

const token = await readOnePasswordReference(tokenReference);
const connector = new DiscordConnector(token);
const channelId = testChannelReference
  ? await readOnePasswordReference(testChannelReference)
  : await resolveDiscordChannelIdByName(token, testChannelName!);

await connector.send(channelId, {
  embed: {
    title: "Coven Channels smoke test",
    description: "If you see this, the harness-agnostic Discord connector works.",
    color: 0x8e3dff,
    author: { name: "OpenCoven" },
    timestamp: new Date().toISOString(),
  },
});

console.log("Discord smoke test sent.");
