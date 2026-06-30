import type { ChannelConnector, ChannelMap, ChannelMessage } from "../types.ts";
import { toTelegramTexts } from "./format.ts";

export interface TelegramConnectorOptions {
  apiBaseUrl?: string;
  fetch?: typeof fetch;
  targets?: ChannelMap;
  userAgent?: string;
}

const TELEGRAM_API_BASE_URL = "https://api.telegram.org";
const DEFAULT_USER_AGENT = "OpenCovenChannels/1 (https://opencoven.ai)";

async function readResponseBody(response: Response): Promise<string> {
  return response.text().catch(() => "");
}

function redactTelegramDetails(value: string, token: string): string {
  return value
    .replaceAll(token, "[redacted]")
    .replace(/https:\/\/api\.telegram\.org\/bot[^\s"')]+/g, "[redacted Telegram API URL]");
}

export class TelegramConnector implements ChannelConnector {
  readonly #apiBaseUrl: string;
  readonly #fetch: typeof fetch;
  readonly #targets: ChannelMap;
  readonly #token: string;
  readonly #userAgent: string;

  constructor(token: string, options: TelegramConnectorOptions = {}) {
    this.#token = token;
    this.#apiBaseUrl = options.apiBaseUrl ?? TELEGRAM_API_BASE_URL;
    this.#fetch = options.fetch ?? fetch;
    this.#targets = options.targets ?? {};
    this.#userAgent = options.userAgent ?? DEFAULT_USER_AGENT;
  }

  async send(targetIdOrName: string, message: ChannelMessage): Promise<void> {
    const chatId = this.#targets[targetIdOrName] ?? targetIdOrName;
    const url = `${this.#apiBaseUrl}/bot${this.#token}/sendMessage`;

    for (const text of toTelegramTexts(message)) {
      const response = await this.#fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "User-Agent": this.#userAgent,
        },
        body: JSON.stringify({
          chat_id: chatId,
          text,
          disable_web_page_preview: true,
        }),
      }).catch(() => {
        throw new Error("Telegram API request failed.");
      });

      const body = await readResponseBody(response);
      if (!response.ok) {
        throw new Error(
          `Telegram API error ${response.status}: ${redactTelegramDetails(body, this.#token)}`,
        );
      }

      if (body) {
        const parsed = JSON.parse(body) as { ok?: unknown; description?: unknown };
        if (parsed.ok === false) {
          const description =
            typeof parsed.description === "string" ? parsed.description : "request failed";
          throw new Error(`Telegram API error: ${redactTelegramDetails(description, this.#token)}`);
        }
      }
    }
  }
}
