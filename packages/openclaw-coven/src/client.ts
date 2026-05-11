import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { lstatIfExists, pathIsInside } from "./path-utils.js";

export type CovenSessionRecord = {
  id: string;
  projectRoot: string;
  harness: string;
  title: string;
  status: string;
  exitCode: number | null;
  createdAt: string;
  updatedAt: string;
};

export type CovenEventRecord = {
  seq: number;
  id: string;
  sessionId: string;
  kind: string;
  payloadJson: string;
  createdAt: string;
};

export type CovenHealthCapabilities = {
  sessions: boolean;
  events: boolean;
  eventCursor: string;
  structuredErrors: boolean;
};

export type CovenHealthResponse = {
  apiVersion: string;
  covenVersion: string;
  capabilities: CovenHealthCapabilities;
  ok: boolean;
  daemon?: {
    pid: number;
    startedAt: string;
    socket: string;
  } | null;
};

export type CovenEventsResponse = {
  events: CovenEventRecord[];
  nextCursor: { afterSeq: number } | null;
  hasMore: boolean;
};

export type LaunchCovenSessionInput = {
  projectRoot: string;
  cwd: string;
  harness: string;
  prompt: string;
  title: string;
};

export interface CovenClient {
  health(signal?: AbortSignal): Promise<CovenHealthResponse>;
  launchSession(input: LaunchCovenSessionInput, signal?: AbortSignal): Promise<CovenSessionRecord>;
  getSession(sessionId: string, signal?: AbortSignal): Promise<CovenSessionRecord>;
  listEvents(
    sessionId: string,
    options?: CovenListEventsOptions,
    signal?: AbortSignal,
  ): Promise<CovenEventRecord[]>;
  sendInput(sessionId: string, data: string, signal?: AbortSignal): Promise<void>;
  killSession(sessionId: string, signal?: AbortSignal): Promise<void>;
}

export type CovenListEventsOptions = {
  afterSeq?: number;
  afterEventId?: string;
  limit?: number;
};

const COVEN_API_URL_VERSION = "v1";
const COVEN_API_CONTRACT_VERSION = "coven.daemon.v1";
const COVEN_API_BASE_PATH = `/api/${COVEN_API_URL_VERSION}`;

type RequestOptions = {
  socketPath: string;
  socketRoot?: string;
  method: "GET" | "POST";
  path: string;
  body?: unknown;
  signal?: AbortSignal;
};

type HttpResponse = {
  status: number;
  body: string;
};

type JsonRecord = Record<string, unknown>;

type SocketFingerprint = {
  dev: number;
  ino: number;
  mode: number;
  uid: number;
  gid: number;
};

export class CovenApiError extends Error {
  readonly status: number;
  readonly body: string;

  constructor(status: number, body: string) {
    super(`Coven API returned HTTP ${status || "unknown"}`);
    this.name = "CovenApiError";
    this.status = status;
    this.body = body;
  }
}

const DEFAULT_REQUEST_TIMEOUT_MS = 10_000;
const MAX_REQUEST_BYTES = 1_000_000;
const MAX_RESPONSE_BYTES = 1_000_000;
const DEFAULT_SOCKET_FILENAME = "coven.sock";
const SAFE_QUERY_ID_REGEX = /^[A-Za-z0-9._:-]+$/;
const MAX_QUERY_ID_CHARS = 256;

function statExistingPath(filePath: string, label: string): fs.Stats {
  try {
    return fs.statSync(filePath);
  } catch {
    throw new Error(`${label} must exist`);
  }
}

function realpathExistingPath(filePath: string, label: string): string {
  try {
    return fs.realpathSync.native(filePath);
  } catch {
    throw new Error(`${label} must exist`);
  }
}

function fingerprintSocket(stat: fs.Stats): SocketFingerprint {
  return {
    dev: stat.dev,
    ino: stat.ino,
    mode: stat.mode,
    uid: stat.uid,
    gid: stat.gid,
  };
}

function socketFingerprintMatches(left: SocketFingerprint, right: SocketFingerprint): boolean {
  return (
    left.dev === right.dev &&
    left.ino === right.ino &&
    left.mode === right.mode &&
    left.uid === right.uid &&
    left.gid === right.gid
  );
}

function validateSocketPathForUse(
  socketPath: string,
  socketRoot: string | undefined,
  platform: NodeJS.Platform = process.platform,
): SocketFingerprint | null {
  if (!socketRoot) {
    return null;
  }
  validateSocketPlatform(platform);
  const socketRootLstat = lstatIfExists(socketRoot);
  if (socketRootLstat?.isSymbolicLink()) {
    throw new Error("Coven covenHome must not be a symlink");
  }
  const socketRootStat = statExistingPath(socketRoot, "Coven covenHome");
  validateSocketOwnerAndMode(socketRootStat, "Coven covenHome", platform);
  validatePrivateDirectory(socketRootStat, "Coven covenHome", platform);
  const expectedSocketPath = path.resolve(socketRoot, DEFAULT_SOCKET_FILENAME);
  if (path.resolve(socketPath) !== expectedSocketPath) {
    throw new Error("Coven socketPath must be <covenHome>/coven.sock");
  }

  const socketStat = lstatIfExists(socketPath);
  if (socketStat?.isSymbolicLink()) {
    throw new Error("Coven socketPath must not be a symlink");
  }
  const resolvedSocketStat = statExistingPath(socketPath, "Coven socketPath");
  if (!resolvedSocketStat.isSocket()) {
    throw new Error("Coven socketPath must be a Unix socket");
  }
  validateSocketOwnerAndMode(resolvedSocketStat, "Coven socketPath", platform);

  const realSocketRoot = realpathExistingPath(socketRoot, "Coven covenHome");
  const realSocketDir = realpathExistingPath(
    path.dirname(socketPath),
    "Coven socketPath directory",
  );
  const socketDirStat = statExistingPath(path.dirname(socketPath), "Coven socketPath directory");
  validateSocketOwnerAndMode(socketDirStat, "Coven socketPath directory", platform);
  validatePrivateDirectory(socketDirStat, "Coven socketPath directory", platform);
  if (!pathIsInside(realSocketRoot, realSocketDir)) {
    throw new Error("Coven socketPath must stay inside covenHome");
  }
  const realSocketPath = realpathExistingPath(socketPath, "Coven socketPath");
  if (!pathIsInside(realSocketRoot, realSocketPath)) {
    throw new Error("Coven socketPath must stay inside covenHome");
  }
  return fingerprintSocket(resolvedSocketStat);
}

function validateSocketPlatform(platform: NodeJS.Platform): void {
  if (platform === "win32") {
    throw new Error("Coven Unix socket validation is not supported on Windows");
  }
}

function requireSafeQueryId(input: string, label: string): string {
  const value = input.trim();
  if (!value || value.length > MAX_QUERY_ID_CHARS || !SAFE_QUERY_ID_REGEX.test(value)) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function validateSocketOwnerAndMode(
  stat: fs.Stats,
  label: string,
  platform: NodeJS.Platform,
): void {
  validateSocketPlatform(platform);
  const currentUid = typeof process.getuid === "function" ? process.getuid() : null;
  if (currentUid != null && stat.uid !== currentUid) {
    throw new Error(`${label} must be owned by the current user`);
  }
  if ((stat.mode & 0o022) !== 0) {
    throw new Error(`${label} must not be group or world writable`);
  }
}

function validatePrivateDirectory(stat: fs.Stats, label: string, platform: NodeJS.Platform): void {
  validateSocketPlatform(platform);
  if (!stat.isDirectory()) {
    throw new Error(`${label} must be a directory`);
  }
  if ((stat.mode & 0o077) !== 0) {
    throw new Error(`${label} must not be group or world accessible`);
  }
}

function serializeRequestBody(body: unknown): { text: string; byteLength: number } {
  if (body === undefined) {
    return { text: "", byteLength: 0 };
  }
  const text = JSON.stringify(body) ?? "";
  const byteLength = Buffer.byteLength(text, "utf8");
  if (byteLength > MAX_REQUEST_BYTES) {
    throw new Error("Coven API request exceeded size limit");
  }
  return { text, byteLength };
}

function errorToError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function socketThatFailsWith(error: unknown): net.Socket {
  const socket = new net.Socket();
  queueMicrotask(() => socket.destroy(errorToError(error)));
  return socket;
}

function requestOverSocket(options: RequestOptions): Promise<HttpResponse> {
  return new Promise((resolve, reject) => {
    if (options.signal?.aborted) {
      reject(options.signal.reason ?? new Error("request aborted"));
      return;
    }
    let requestBody = "";
    let requestBodyBytes = 0;
    let socketFingerprint: SocketFingerprint | null = null;
    try {
      socketFingerprint = validateSocketPathForUse(options.socketPath, options.socketRoot);
      const serialized = serializeRequestBody(options.body);
      requestBody = serialized.text;
      requestBodyBytes = serialized.byteLength;
    } catch (error) {
      reject(error);
      return;
    }

    let settled = false;
    let body = "";
    let totalBytes = 0;

    const settle = (fn: () => void, req?: http.ClientRequest) => {
      if (settled) {
        return;
      }
      settled = true;
      req?.destroy();
      fn();
    };

    const req = http.request(
      {
        createConnection: () => {
          try {
            const beforeConnect = validateSocketPathForUse(options.socketPath, options.socketRoot);
            const socket = net.createConnection({ path: options.socketPath });
            socket.once("connect", () => {
              try {
                const afterConnect = validateSocketPathForUse(
                  options.socketPath,
                  options.socketRoot,
                );
                const expected = beforeConnect ?? socketFingerprint;
                if (expected && afterConnect && !socketFingerprintMatches(expected, afterConnect)) {
                  socket.destroy(new Error("Coven socketPath changed during connection"));
                }
              } catch (error) {
                socket.destroy(errorToError(error));
              }
            });
            return socket;
          } catch (error) {
            return socketThatFailsWith(error);
          }
        },
        method: options.method,
        path: options.path,
        headers: {
          Host: "coven",
          Connection: "close",
          ...(requestBody
            ? {
                "Content-Type": "application/json",
                "Content-Length": requestBodyBytes,
              }
            : {}),
        },
        signal: options.signal,
      },
      (res) => {
        res.setEncoding("utf8");
        res.on("data", (chunk: string) => {
          if (settled) {
            return;
          }
          totalBytes += Buffer.byteLength(chunk);
          if (totalBytes > MAX_RESPONSE_BYTES) {
            settle(() => reject(new Error("Coven API response exceeded size limit")), req);
            return;
          }
          body += chunk;
        });
        res.on("end", () => {
          settle(() =>
            resolve({
              status: res.statusCode ?? 0,
              body,
            }),
          );
        });
        res.on("error", (error) => settle(() => reject(error), req));
      },
    );
    req.setTimeout(DEFAULT_REQUEST_TIMEOUT_MS, () => {
      settle(() => reject(new Error("Coven API request timed out")), req);
    });
    req.on("error", (error) => {
      if (settled) {
        return;
      }
      settle(() => reject(error));
    });
    req.end(requestBody);
  });
}

async function requestJson<T>(options: RequestOptions): Promise<T> {
  const response = await requestOverSocket(options);
  if (response.status < 200 || response.status >= 300) {
    throw new CovenApiError(response.status, response.body);
  }
  try {
    return JSON.parse(response.body || "null") as T;
  } catch (error) {
    throw new CovenApiError(response.status, `Invalid JSON response: ${String(error)}`);
  }
}

function requireRecord(value: unknown, label: string): JsonRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} response must be an object`);
  }
  return value as JsonRecord;
}

function requireStringField(record: JsonRecord, camelKey: string, snakeKey: string): string {
  const value = record[camelKey] ?? record[snakeKey];
  if (typeof value !== "string") {
    throw new Error(`Coven response field ${camelKey} is invalid`);
  }
  return value;
}

function requireNullableNumberField(
  record: JsonRecord,
  camelKey: string,
  snakeKey: string,
): number | null {
  const value = record[camelKey] ?? record[snakeKey];
  if (value === null || value === undefined) {
    return null;
  }
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`Coven response field ${camelKey} is invalid`);
  }
  return value;
}

function normalizeHealthResponse(value: unknown): CovenHealthResponse {
  const record = requireRecord(value, "Coven health");
  if (record.apiVersion !== COVEN_API_CONTRACT_VERSION) {
    throw new Error(`Coven API version is unsupported: ${String(record.apiVersion)}`);
  }
  if (typeof record.ok !== "boolean") {
    throw new Error("Coven response field ok is invalid");
  }
  return record as CovenHealthResponse;
}

function normalizeSessionRecord(value: unknown): CovenSessionRecord {
  const record = requireRecord(value, "Coven session");
  return {
    id: requireStringField(record, "id", "id"),
    projectRoot: requireStringField(record, "projectRoot", "project_root"),
    harness: requireStringField(record, "harness", "harness"),
    title: requireStringField(record, "title", "title"),
    status: requireStringField(record, "status", "status"),
    exitCode: requireNullableNumberField(record, "exitCode", "exit_code"),
    createdAt: requireStringField(record, "createdAt", "created_at"),
    updatedAt: requireStringField(record, "updatedAt", "updated_at"),
  };
}

function normalizeEventRecord(value: unknown): CovenEventRecord {
  const record = requireRecord(value, "Coven event");
  return {
    // seq is 0 for records received from daemons that pre-date coven.daemon.v1;
    // production responses from a coven.daemon.v1 daemon always include seq > 0.
    seq: (record.seq as number) ?? 0,
    id: requireStringField(record, "id", "id"),
    sessionId: requireStringField(record, "sessionId", "session_id"),
    kind: requireStringField(record, "kind", "kind"),
    payloadJson: requireStringField(record, "payloadJson", "payload_json"),
    createdAt: requireStringField(record, "createdAt", "created_at"),
  };
}

function normalizeEventRecords(value: unknown): CovenEventRecord[] {
  // Accept either the paginated envelope { events, nextCursor, hasMore } or a
  // plain array (legacy compatibility shim during the migration window).
  if (Array.isArray(value)) {
    return value.map(normalizeEventRecord);
  }
  const envelope = requireRecord(value, "Coven events response");
  if (!Array.isArray(envelope.events)) {
    throw new Error("Coven events response must contain an events array");
  }
  return envelope.events.map(normalizeEventRecord);
}

export function createCovenClient(
  socketPath: string,
  clientOptions: { socketRoot?: string } = {},
): CovenClient {
  return {
    health(signal) {
      return requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "GET",
        path: `${COVEN_API_BASE_PATH}/health`,
        signal,
      }).then(normalizeHealthResponse);
    },
    launchSession(input, signal) {
      return requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "POST",
        path: `${COVEN_API_BASE_PATH}/sessions`,
        body: input,
        signal,
      }).then(normalizeSessionRecord);
    },
    getSession(sessionId, signal) {
      return requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "GET",
        path: `${COVEN_API_BASE_PATH}/sessions/${encodeURIComponent(sessionId)}`,
        signal,
      }).then(normalizeSessionRecord);
    },
    listEvents(sessionId, options, signal) {
      const params = new URLSearchParams({
        sessionId: requireSafeQueryId(sessionId, "Coven session id"),
      });
      const afterSeq = options?.afterSeq;
      if (typeof afterSeq === "number") {
        params.set("afterSeq", String(afterSeq));
      }
      const afterEventId = options?.afterEventId?.trim();
      if (afterEventId) {
        params.set("afterEventId", requireSafeQueryId(afterEventId, "Coven event id"));
      }
      const limit = options?.limit;
      if (typeof limit === "number") {
        params.set("limit", String(Math.max(1, Math.floor(limit))));
      }
      return requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "GET",
        path: `${COVEN_API_BASE_PATH}/events?${params.toString()}`,
        signal,
      }).then(normalizeEventRecords);
    },
    async sendInput(sessionId, data, signal) {
      await requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "POST",
        path: `${COVEN_API_BASE_PATH}/sessions/${encodeURIComponent(sessionId)}/input`,
        body: { data },
        signal,
      });
    },
    async killSession(sessionId, signal) {
      await requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "POST",
        path: `${COVEN_API_BASE_PATH}/sessions/${encodeURIComponent(sessionId)}/kill`,
        signal,
      });
    },
  };
}

export const __testing = {
  validateSocketPathForUse,
  normalizeEventRecord,
  normalizeHealthResponse,
  normalizeSessionRecord,
};
