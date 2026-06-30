import {
  createConnector,
  readOnePasswordReference,
  resolveOnePasswordReference,
} from "../src/index.ts";

const tokenReference = resolveOnePasswordReference("COVEN_TELEGRAM_TOKEN_REF");
const testTargetReference = resolveOnePasswordReference("COVEN_TELEGRAM_TEST_TARGET_REF");

if (!tokenReference || !testTargetReference) {
  console.log("1Password Telegram smoke references not set; skipping Telegram smoke test.");
  process.exit(0);
}

const targetId = await readOnePasswordReference(testTargetReference);
const connector = await createConnector("telegram", { tokenRef: tokenReference });

await connector.send(targetId, {
  text: "Coven Channels Telegram smoke test",
});

console.log("Telegram smoke test sent.");
