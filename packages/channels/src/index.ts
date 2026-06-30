import { resolveToken, type ResolveTokenOptions } from "./discord/auth.ts";
import { DiscordConnector, type DiscordConnectorOptions } from "./discord/index.ts";
import {
  resolveTelegramToken,
  type ResolveTelegramTokenOptions,
} from "./telegram/auth.ts";
import { TelegramConnector, type TelegramConnectorOptions } from "./telegram/index.ts";
import type { ChannelConnector } from "./types.ts";

export { resolveDiscordChannelIdByName } from "./discord/channels.ts";
export type { DiscordChannelLookupOptions } from "./discord/channels.ts";
export { DiscordConnector } from "./discord/index.ts";
export type { DiscordConnectorOptions } from "./discord/index.ts";
export { resolveTelegramToken } from "./telegram/auth.ts";
export type { ResolveTelegramTokenOptions } from "./telegram/auth.ts";
export { TELEGRAM_TEXT_LIMIT, toTelegramTexts } from "./telegram/format.ts";
export { TelegramConnector } from "./telegram/index.ts";
export type { TelegramConnectorOptions } from "./telegram/index.ts";
export { readOnePasswordReference, resolveOnePasswordReference } from "./onepassword.ts";
export type { OnePasswordRead, OnePasswordReadOptions } from "./onepassword.ts";
export type { ChannelConnector, ChannelEmbed, ChannelMap, ChannelMessage } from "./types.ts";

export type ConnectorKind = "discord" | "telegram";

export interface CreateConnectorOptions
  extends ResolveTokenOptions,
    ResolveTelegramTokenOptions,
    DiscordConnectorOptions,
    TelegramConnectorOptions {}

export async function createConnector(
  kind: ConnectorKind,
  options: CreateConnectorOptions = {},
): Promise<ChannelConnector> {
  switch (kind) {
    case "discord":
      return new DiscordConnector(await resolveToken(options), options);
    case "telegram":
      return new TelegramConnector(await resolveTelegramToken(options), options);
    default:
      throw new Error(`Unknown connector kind: ${kind}`);
  }
}
