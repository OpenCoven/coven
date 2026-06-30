import {
  readOnePasswordReference,
  resolveOnePasswordReference,
  type OnePasswordRead,
} from "../onepassword.ts";

export interface ResolveTelegramTokenOptions {
  env?: Record<string, string | undefined>;
  readOnePassword?: OnePasswordRead;
  tokenRef?: string;
}

const TELEGRAM_TOKEN_REF_ENV = "COVEN_TELEGRAM_TOKEN_REF";

export async function resolveTelegramToken(
  options: ResolveTelegramTokenOptions = {},
): Promise<string> {
  const reference =
    options.tokenRef?.trim() ?? resolveOnePasswordReference(TELEGRAM_TOKEN_REF_ENV, options);

  if (!reference) {
    throw new Error(`Telegram bot token reference not found. Set ${TELEGRAM_TOKEN_REF_ENV}.`);
  }

  return readOnePasswordReference(reference, options);
}
