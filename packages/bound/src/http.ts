/** Tiny shared HTTP helpers. Kept deliberately small; no framework needed. */

import { createServer, type IncomingMessage, type ServerResponse, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';

export type Handler = (
  req: IncomingMessage,
  res: ServerResponse,
  url: URL,
) => Promise<void> | void;

export function json(res: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'content-length': Buffer.byteLength(payload),
    'cache-control': 'no-store',
  });
  res.end(payload);
}

export function html(res: ServerResponse, status: number, body: string): void {
  res.writeHead(status, {
    'content-type': 'text/html; charset=utf-8',
    'content-length': Buffer.byteLength(body),
    'cache-control': 'no-store',
  });
  res.end(body);
}

export async function readJson<T>(req: IncomingMessage, limitBytes = 1_000_000): Promise<T | null> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of req) {
    total += (chunk as Buffer).length;
    if (total > limitBytes) return null;
    chunks.push(chunk as Buffer);
  }
  if (total === 0) return null;
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8')) as T;
  } catch {
    return null;
  }
}

export interface RunningService {
  server: Server;
  port: number;
  origin: string;
  close: () => Promise<void>;
}

export function listen(handler: Handler, port: number, name: string): Promise<RunningService> {
  const server = createServer((req, res) => {
    const url = new URL(req.url ?? '/', `http://127.0.0.1`);
    Promise.resolve(handler(req, res, url)).catch((error: unknown) => {
      if (!res.headersSent) {
        json(res, 500, { ok: false, reason: 'internal_error', detail: String(error), service: name });
      } else {
        res.end();
      }
    });
  });

  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => {
      const actual = (server.address() as AddressInfo).port;
      resolve({
        server,
        port: actual,
        origin: `http://127.0.0.1:${actual}`,
        close: () => new Promise<void>((done) => server.close(() => done())),
      });
    });
  });
}

export async function postJson<T>(
  url: string,
  body: unknown,
  timeoutMs = 5000,
): Promise<{ ok: boolean; status: number; body: T | null; error?: string }> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const parsed = (await response.json().catch(() => null)) as T | null;
    return { ok: response.ok, status: response.status, body: parsed };
  } catch (error) {
    return { ok: false, status: 0, body: null, error: (error as Error).message };
  } finally {
    clearTimeout(timer);
  }
}
