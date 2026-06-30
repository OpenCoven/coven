export interface ChannelEmbed {
  title?: string;
  description?: string;
  color?: number;
  author?: {
    name: string;
    icon_url?: string;
  };
  fields?: Array<{ name: string; value: string; inline?: boolean }>;
  footer?: { text: string };
  timestamp?: string;
}

export interface ChannelMessage {
  text?: string;
  embed?: ChannelEmbed;
}

export interface ChannelConnector {
  send(channelIdOrName: string, message: ChannelMessage): Promise<void>;
}

export type ChannelMap = Record<string, string>;
