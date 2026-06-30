export interface DiscordChannelLookupOptions {
  apiBaseUrl?: string;
  fetch?: typeof fetch;
  userAgent?: string;
}

interface DiscordGuildSummary {
  id?: unknown;
}

interface DiscordChannelSummary {
  id?: unknown;
  name?: unknown;
}

const DISCORD_API_BASE_URL = "https://discord.com/api/v10";
const DEFAULT_USER_AGENT = "OpenCovenChannels/1 (https://opencoven.ai)";

function normalizeChannelName(channelName: string): string {
  return channelName.trim().replace(/^#+/, "");
}

async function readJson(response: Response, context: string): Promise<unknown> {
  const body = await response.text().catch(() => "");

  if (!response.ok) {
    throw new Error(`Discord API error ${response.status} while ${context}: ${body}`);
  }

  return body ? JSON.parse(body) : null;
}

function requireArray(value: unknown, context: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new Error(`Discord API returned an unexpected ${context} response.`);
  }

  return value;
}

export async function resolveDiscordChannelIdByName(
  token: string,
  channelName: string,
  options: DiscordChannelLookupOptions = {},
): Promise<string> {
  const targetName = normalizeChannelName(channelName);

  if (!targetName) {
    throw new Error("Discord channel name is required.");
  }

  const apiBaseUrl = options.apiBaseUrl ?? DISCORD_API_BASE_URL;
  const request = options.fetch ?? fetch;
  const headers = {
    Authorization: `Bot ${token}`,
    "User-Agent": options.userAgent ?? DEFAULT_USER_AGENT,
  };

  const guildsResponse = await request(`${apiBaseUrl}/users/@me/guilds`, { headers });
  const guilds = requireArray(await readJson(guildsResponse, "listing bot guilds"), "guild list");
  const matches: string[] = [];

  for (const guild of guilds as DiscordGuildSummary[]) {
    if (typeof guild.id !== "string") {
      continue;
    }

    const channelsResponse = await request(`${apiBaseUrl}/guilds/${encodeURIComponent(guild.id)}/channels`, {
      headers,
    });
    const channels = requireArray(
      await readJson(channelsResponse, "listing guild channels"),
      "channel list",
    );

    for (const channel of channels as DiscordChannelSummary[]) {
      if (channel.name === targetName && typeof channel.id === "string") {
        matches.push(channel.id);
      }
    }
  }

  if (matches.length === 1) {
    return matches[0]!;
  }

  if (matches.length > 1) {
    throw new Error(`Discord channel name "${targetName}" matched ${matches.length} accessible channels.`);
  }

  throw new Error(`Discord channel name "${targetName}" was not found in accessible guild channels.`);
}
