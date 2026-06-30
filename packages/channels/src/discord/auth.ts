import {
  readOnePasswordReference,
  resolveOnePasswordReference,
  type OnePasswordRead,
} from "../onepassword.ts";

export interface ResolveTokenOptions {
  env?: Record<string, string | undefined>;
  readOnePassword?: OnePasswordRead;
  tokenRef?: string;
}

const DISCORD_TOKEN_REF_ENV = "COVEN_DISCORD_TOKEN_REF";

export async function resolveToken(options: ResolveTokenOptions = {}): Promise<string> {
  const reference =
    options.tokenRef?.trim() ?? resolveOnePasswordReference(DISCORD_TOKEN_REF_ENV, options);

  if (!reference) {
    throw new Error(`Discord bot token reference not found. Set ${DISCORD_TOKEN_REF_ENV}.`);
  }

  return readOnePasswordReference(reference, options);
}
