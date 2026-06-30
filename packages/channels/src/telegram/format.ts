import type { ChannelEmbed, ChannelMessage } from "../types.ts";

export const TELEGRAM_TEXT_LIMIT = 3900;

function clean(value: string | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

function embedLines(embed: ChannelEmbed): string[] {
  const lines: string[] = [];
  const title = clean(embed.title);
  const description = clean(embed.description);
  const footer = clean(embed.footer?.text);
  const author = clean(embed.author?.name);
  const timestamp = clean(embed.timestamp);

  if (title) lines.push(title);
  if (description) lines.push(description);

  if (embed.fields?.length) {
    if (lines.length) lines.push("");
    for (const field of embed.fields) {
      const name = clean(field.name);
      const value = clean(field.value);
      if (name && value) lines.push(`${name}: ${value}`);
    }
  }

  const meta = [footer, author, timestamp].filter((value): value is string => Boolean(value));
  if (meta.length) {
    if (lines.length) lines.push("");
    lines.push(...meta);
  }

  return lines;
}

function chunkText(text: string, limit: number): string[] {
  const chunks: string[] = [];
  for (let index = 0; index < text.length; index += limit) {
    chunks.push(text.slice(index, index + limit));
  }
  return chunks;
}

export function toTelegramTexts(
  message: ChannelMessage,
  limit = TELEGRAM_TEXT_LIMIT,
): string[] {
  const sections: string[] = [];
  const text = clean(message.text);

  if (text) {
    sections.push(text);
  }

  if (message.embed) {
    const renderedEmbed = embedLines(message.embed).join("\n").trim();
    if (renderedEmbed) sections.push(renderedEmbed);
  }

  const body = sections.join("\n\n").trim();
  if (!body) {
    throw new Error("ChannelMessage must include text or embed.");
  }

  return chunkText(body, limit);
}
