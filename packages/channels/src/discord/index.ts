import type { ChannelConnector, ChannelMap, ChannelMessage } from "../types.ts";
import { toDiscordPayload } from "./format.ts";

export interface DiscordConnectorOptions {
  apiBaseUrl?: string;
  channels?: ChannelMap;
  fetch?: typeof fetch;
  sleep?: (ms: number) => Promise<void>;
  userAgent?: string;
}

const DISCORD_API_BASE_URL = "https://discord.com/api/v10";
const DEFAULT_USER_AGENT = "OpenCovenChannels/1 (https://opencoven.ai)";

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function parseRetryAfter(response: Response): number {
  const retryAfterSeconds = Number(response.headers.get("retry-after"));
  return Number.isFinite(retryAfterSeconds) && retryAfterSeconds > 0
    ? Math.ceil(retryAfterSeconds * 1000)
    : 1000;
}

async function readResponseBody(response: Response): Promise<string> {
  return response.text().catch(() => "");
}

export class DiscordConnector implements ChannelConnector {
  readonly #apiBaseUrl: string;
  readonly #channels: ChannelMap;
  readonly #fetch: typeof fetch;
  readonly #sleep: (ms: number) => Promise<void>;
  readonly #token: string;
  readonly #userAgent: string;

  constructor(token: string, options: DiscordConnectorOptions = {}) {
    this.#token = token;
    this.#apiBaseUrl = options.apiBaseUrl ?? DISCORD_API_BASE_URL;
    this.#channels = options.channels ?? {};
    this.#fetch = options.fetch ?? fetch;
    this.#sleep = options.sleep ?? defaultSleep;
    this.#userAgent = options.userAgent ?? DEFAULT_USER_AGENT;
  }

  async send(channelIdOrName: string, message: ChannelMessage): Promise<void> {
    const channelId = this.#channels[channelIdOrName] ?? channelIdOrName;
    const payload = toDiscordPayload(message);
    const url = `${this.#apiBaseUrl}/channels/${encodeURIComponent(channelId)}/messages`;
    const init: RequestInit = {
      method: "POST",
      headers: {
        Authorization: `Bot ${this.#token}`,
        "Content-Type": "application/json",
        "User-Agent": this.#userAgent,
      },
      body: JSON.stringify(payload),
    };

    const first = await this.#fetch(url, init);

    if (first.ok) {
      return;
    }

    if (first.status === 429) {
      await this.#sleep(parseRetryAfter(first));
      const second = await this.#fetch(url, init);
      if (second.ok) {
        return;
      }

      const body = await readResponseBody(second);
      throw new Error(`Discord API error ${second.status}: ${body}`);
    }

    if (first.status >= 500) {
      await this.#sleep(2000);
      const second = await this.#fetch(url, init);
      if (second.ok) {
        return;
      }

      const body = await readResponseBody(second);
      throw new Error(`Discord API error ${second.status}: ${body}`);
    }

    const body = await readResponseBody(first);
    throw new Error(`Discord API error ${first.status}: ${body}`);
  }
}
