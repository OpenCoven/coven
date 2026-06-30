import type { ChannelMessage } from "../types.ts";

export interface DiscordEmbed {
  title?: string;
  description?: string;
  color?: number;
  author?: { name: string; icon_url?: string };
  fields?: Array<{ name: string; value: string; inline?: boolean }>;
  footer?: { text: string };
  timestamp?: string;
}

export interface DiscordPayload {
  content?: string;
  embeds?: DiscordEmbed[];
}

export function toDiscordPayload(message: ChannelMessage): DiscordPayload {
  const payload: DiscordPayload = {};
  const text = message.text?.trim();

  if (text) {
    payload.content = text;
  }

  if (message.embed) {
    payload.embeds = [message.embed];
  }

  if (!payload.content && !payload.embeds?.length) {
    throw new Error("ChannelMessage must include text or embed.");
  }

  return payload;
}
