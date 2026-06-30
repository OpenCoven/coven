import assert from "node:assert/strict";
import { test } from "node:test";

import { readOnePasswordReference, resolveOnePasswordReference } from "../src/onepassword.ts";

test("resolveOnePasswordReference reads a reference without resolving the secret", () => {
  assert.equal(
    resolveOnePasswordReference("COVEN_DISCORD_TOKEN_REF", {
      env: { COVEN_DISCORD_TOKEN_REF: "op://vault/item/token" },
    }),
    "op://vault/item/token",
  );
});

test("readOnePasswordReference trims the resolved secret", async () => {
  const secret = await readOnePasswordReference("op://vault/item/field", {
    readOnePassword: async () => " resolved-secret\n",
  });

  assert.equal(secret, "resolved-secret");
});

test("readOnePasswordReference supports 1Password item titles with a pipe", async () => {
  const calls: string[][] = [];
  const secret = await readOnePasswordReference("op://Vault/Bot | Private/credential", {
    runOnePasswordCli: async (args) => {
      calls.push(args);
      return " resolved-secret\n";
    },
  });

  assert.equal(secret, "resolved-secret");
  assert.deepEqual(calls, [
    ["item", "get", "Bot | Private", "--vault", "Vault", "--fields", "label=credential", "--reveal"],
  ]);
});

test("readOnePasswordReference does not expose references when CLI reads fail", async () => {
  await assert.rejects(
    () =>
      readOnePasswordReference("op://Vault/Bot | Private/credential", {
        runOnePasswordCli: async () => {
          throw new Error("invalid reference: op://Vault/Bot | Private/credential");
        },
      }),
    (error) => error instanceof Error && error.message === "1Password reference could not be read.",
  );
});

test("readOnePasswordReference rejects empty 1Password values", async () => {
  await assert.rejects(
    () =>
      readOnePasswordReference("op://vault/item/field", {
        readOnePassword: async () => "  ",
      }),
    /1Password reference resolved to an empty value\./,
  );
});
