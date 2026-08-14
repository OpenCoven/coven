import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { lstatIfExists, pathIsInside } from "./path-utils.js";

/**
 * Contract name for the Psyche opaque execution-binding tuple that Coven
 * validates for shape/syntax/expiry and exact-compares on bound mutations,
 * but never interprets. See `specs/psyche/O2_CONTRACT_DESIGN.md`.
 */
export const PSYCHE_EXECUTION_BINDING_V1 = "psyche.execution_binding.v1" as const;

export type CovenExecutionBindingParent = {
  sessionId: string;
  graphId: string;
  nodeId: string;
  attemptId: string;
};

export type CovenExecutionBinding = {
  contract: typeof PSYCHE_EXECUTION_BINDING_V1;
  principalRef: string;
  familiarId: string;
  familiarSnapshotDigest: string;
  projectDigest: string;
  graphId: string;
  nodeId: string;
  attemptId: string;
  requestDigest: string;
  policyRevision: string;
  expiresAt: string;
  parent: CovenExecutionBindingParent | null;
  delegationDigest: string | null;
};

export type CovenSessionRecord = {
  id: string;
  projectRoot: string;
  harness: string;
  title: string;
  status: string;
  exitCode: number | null;
  createdAt: string;
  updatedAt: string;
  executionBinding: CovenExecutionBinding | null;
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
  sessions?: unknown;
  events?: unknown;
  eventCursor?: unknown;
  structuredErrors?: unknown;
  executionBindingContracts?: unknown;
};

export type CovenHealthResponse = {
  apiVersion?: unknown;
  covenVersion?: unknown;
  capabilities?: CovenHealthCapabilities;
  ok?: unknown;
  daemon?: unknown;
};

export type CovenEventsResponse = {
  events: CovenEventRecord[];
  nextCursor: { afterSeq: number } | null;
  hasMore: boolean;
};

/**
 * Pre-O2 harness launch mode. Mirrors the daemon's `HarnessLaunchMode`
 * (`crates/coven-cli/src/harness.rs`) and the exact wire strings
 * `launch_mode_from_payload` (`crates/coven-cli/src/api.rs`) accepts.
 */
export type CovenLaunchMode = "interactive" | "nonInteractive" | "stream";

/**
 * Pre-O2 unattended-launch policy. Mirrors the daemon's `LaunchPolicyPayload`
 * (`crates/coven-cli/src/api.rs`): `approval`/`sandbox` are accepted only as
 * the exact literals below, and are enforced only for Codex nonInteractive
 * launches. `addDirs` is optional and defaults to no extra directories.
 */
export type CovenLaunchPolicy = {
  approval: "never";
  sandbox: "workspace-write";
  addDirs?: string[];
};

/**
 * Pre-O2 conversation continuation hint. Mirrors the daemon's
 * `ConversationHint` (`crates/coven-cli/src/harness.rs`): `init` starts a
 * new harness-native conversation claimed under `id`; `resume` continues an
 * existing one.
 */
export type CovenConversationHint = {
  mode: "init" | "resume";
  id: string;
};

export type LaunchCovenSessionInput = {
  projectRoot: string;
  cwd: string;
  harness: string;
  prompt: string;
  title: string;
  model?: string;
  launchMode?: CovenLaunchMode;
  launchPolicy?: CovenLaunchPolicy;
  conversation?: CovenConversationHint;
  conversationId?: string;
  familiarId?: string;
  callerFamiliarId?: string;
  executionBinding?: CovenExecutionBinding;
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
  sendBoundInput(
    sessionId: string,
    data: string,
    executionBinding: CovenExecutionBinding,
    signal?: AbortSignal,
  ): Promise<void>;
  killBoundSession(
    sessionId: string,
    executionBinding: CovenExecutionBinding,
    signal?: AbortSignal,
  ): Promise<void>;
}

export type CovenListEventsOptions = {
  afterSeq?: number;
  afterEventId?: string;
  limit?: number;
};

const COVEN_API_URL_VERSION = "v1";
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

function isJsonRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

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

const EXECUTION_BINDING_KEYS = [
  "attemptId",
  "contract",
  "delegationDigest",
  "expiresAt",
  "familiarId",
  "familiarSnapshotDigest",
  "graphId",
  "nodeId",
  "parent",
  "policyRevision",
  "principalRef",
  "projectDigest",
  "requestDigest",
] as const;

const EXECUTION_BINDING_PARENT_KEYS = ["attemptId", "graphId", "nodeId", "sessionId"] as const;

// Opaque, Psyche-defined fields: 1-255 bytes drawn only from
// [A-Za-z0-9._:/-]. Values are never trimmed or case-folded — a rejected
// value is invalid as-is.
const OPAQUE_VALUE_REGEX = /^[A-Za-z0-9._:/-]+$/;
const DIGEST_REGEX = /^sha256:[0-9a-f]{64}$/;
const CANONICAL_EXPIRY_REGEX = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/;

/**
 * Closed-object membership check shared by `executionBinding` and its
 * `parent`: every expected key must be present and no unknown key may be
 * present. This runs before any per-field validation so unknown/missing
 * members are rejected as a single, uniform error class.
 */
function requireExactKeys(record: JsonRecord, expected: readonly string[], label: string): void {
  const actual = Object.keys(record).sort();
  const sortedExpected = [...expected].sort();
  if (
    actual.length !== sortedExpected.length ||
    actual.some((key, index) => key !== sortedExpected[index])
  ) {
    throw new Error(`${label} has missing or unknown fields`);
  }
}

function requireBindingString(record: JsonRecord, key: string, label: string): string {
  const value = record[key];
  if (typeof value !== "string") {
    throw new Error(`${label}.${key} must be a string`);
  }
  return value;
}

function validOpaque(value: string): boolean {
  return (
    Buffer.byteLength(value, "ascii") === value.length &&
    value.length >= 1 &&
    value.length <= 255 &&
    OPAQUE_VALUE_REGEX.test(value)
  );
}

function validDigest(value: string): boolean {
  return DIGEST_REGEX.test(value);
}

const DAYS_IN_MONTH = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

function isGregorianLeapYear(year: number): boolean {
  return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
}

function daysInMonth(year: number, month: number): number {
  return month === 2 && isGregorianLeapYear(year) ? 29 : DAYS_IN_MONTH[month - 1];
}

/**
 * Canonical RFC3339 UTC whole-second timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
 * Calendar and time-of-day fields are validated directly, field by field,
 * rather than via `Date`: `Date` silently overflows out-of-range components
 * (e.g. rolls `2016-12-31T24:00:00Z` into the next day) and always rejects
 * the `:60` leap-second value, which the Rust contract's Chrono-backed
 * `parse_expiry` accepts on every minute boundary (not only real UTC leap
 * seconds). Matching that byte-for-byte, without normalizing or
 * reformatting the input, keeps this validator's accepted set identical to
 * Rust's.
 */
function validCanonicalExpiry(value: string): boolean {
  if (!CANONICAL_EXPIRY_REGEX.test(value)) {
    return false;
  }
  const year = Number(value.slice(0, 4));
  const month = Number(value.slice(5, 7));
  const day = Number(value.slice(8, 10));
  const hour = Number(value.slice(11, 13));
  const minute = Number(value.slice(14, 16));
  const second = Number(value.slice(17, 19));
  if (month < 1 || month > 12) {
    return false;
  }
  if (day < 1 || day > daysInMonth(year, month)) {
    return false;
  }
  // Hour/minute follow the usual 0-23 / 0-59 ranges; seconds allow the
  // leap-second value 60 in addition to the usual 0-59, with no further
  // restriction on which minute it falls in (Chrono parity, verified above).
  return hour <= 23 && minute <= 59 && second <= 60;
}

function normalizeExecutionBindingParent(value: unknown): CovenExecutionBindingParent {
  const record = requireRecord(value, "executionBinding.parent");
  requireExactKeys(record, EXECUTION_BINDING_PARENT_KEYS, "executionBinding.parent");
  const parent = {
    sessionId: requireBindingString(record, "sessionId", "executionBinding.parent"),
    graphId: requireBindingString(record, "graphId", "executionBinding.parent"),
    nodeId: requireBindingString(record, "nodeId", "executionBinding.parent"),
    attemptId: requireBindingString(record, "attemptId", "executionBinding.parent"),
  };
  for (const [key, field] of Object.entries(parent)) {
    if (!validOpaque(field)) {
      throw new Error(`executionBinding.parent.${key} is invalid`);
    }
  }
  return parent;
}

/**
 * Defense-in-depth client-side validator/normalizer for the closed
 * `psyche.execution_binding.v1` wire object. Rust remains authoritative:
 * this exists to fail fast and byte-exact on malformed values before a
 * request is ever sent or a response is trusted, not to enforce business
 * rules (e.g. expiry-at-use) that belong solely to the daemon.
 */
function normalizeExecutionBinding(value: unknown): CovenExecutionBinding {
  const record = requireRecord(value, "executionBinding");
  requireExactKeys(record, EXECUTION_BINDING_KEYS, "executionBinding");
  const contract = requireBindingString(record, "contract", "executionBinding");
  if (contract !== PSYCHE_EXECUTION_BINDING_V1) {
    throw new Error("executionBinding.contract is unsupported");
  }

  const binding: CovenExecutionBinding = {
    contract,
    principalRef: requireBindingString(record, "principalRef", "executionBinding"),
    familiarId: requireBindingString(record, "familiarId", "executionBinding"),
    familiarSnapshotDigest: requireBindingString(
      record,
      "familiarSnapshotDigest",
      "executionBinding",
    ),
    projectDigest: requireBindingString(record, "projectDigest", "executionBinding"),
    graphId: requireBindingString(record, "graphId", "executionBinding"),
    nodeId: requireBindingString(record, "nodeId", "executionBinding"),
    attemptId: requireBindingString(record, "attemptId", "executionBinding"),
    requestDigest: requireBindingString(record, "requestDigest", "executionBinding"),
    policyRevision: requireBindingString(record, "policyRevision", "executionBinding"),
    expiresAt: requireBindingString(record, "expiresAt", "executionBinding"),
    parent: record.parent === null ? null : normalizeExecutionBindingParent(record.parent),
    delegationDigest:
      record.delegationDigest === null
        ? null
        : requireBindingString(record, "delegationDigest", "executionBinding"),
  };

  for (const [key, field] of [
    ["principalRef", binding.principalRef],
    ["familiarId", binding.familiarId],
    ["graphId", binding.graphId],
    ["nodeId", binding.nodeId],
    ["attemptId", binding.attemptId],
    ["policyRevision", binding.policyRevision],
  ] as const) {
    if (!validOpaque(field)) {
      throw new Error(`executionBinding.${key} is invalid`);
    }
  }
  for (const [key, field] of [
    ["familiarSnapshotDigest", binding.familiarSnapshotDigest],
    ["projectDigest", binding.projectDigest],
    ["requestDigest", binding.requestDigest],
  ] as const) {
    if (!validDigest(field)) {
      throw new Error(`executionBinding.${key} is invalid`);
    }
  }
  if (!validCanonicalExpiry(binding.expiresAt)) {
    throw new Error("executionBinding.expiresAt is invalid");
  }
  if (binding.delegationDigest !== null && !validDigest(binding.delegationDigest)) {
    throw new Error("executionBinding.delegationDigest is invalid");
  }
  if ((binding.parent === null) !== (binding.delegationDigest === null)) {
    throw new Error("executionBinding parent/delegationDigest relationship is invalid");
  }
  return binding;
}

function normalizeHealthResponse(value: unknown): CovenHealthResponse {
  const record = requireRecord(value, "Coven health");
  const capabilities = isJsonRecord(record.capabilities) ? record.capabilities : undefined;
  return {
    apiVersion: record.apiVersion,
    covenVersion: record.covenVersion ?? record.coven_version,
    capabilities: capabilities
      ? {
          sessions: capabilities.sessions,
          events: capabilities.events,
          eventCursor: capabilities.eventCursor ?? capabilities.event_cursor,
          structuredErrors: capabilities.structuredErrors ?? capabilities.structured_errors,
          executionBindingContracts:
            capabilities.executionBindingContracts ?? capabilities.execution_binding_contracts,
        }
      : undefined,
    ok: record.ok,
    daemon: record.daemon,
  };
}

function normalizeSessionRecord(value: unknown): CovenSessionRecord {
  const record = requireRecord(value, "Coven session");
  // Rolling-upgrade compatibility: a pre-O2 daemon omits the field entirely,
  // which normalizes to unbound (null) rather than throwing. A present
  // non-null value is fully validated.
  const rawExecutionBinding = record.executionBinding ?? record.execution_binding ?? null;
  return {
    id: requireStringField(record, "id", "id"),
    projectRoot: requireStringField(record, "projectRoot", "project_root"),
    harness: requireStringField(record, "harness", "harness"),
    title: requireStringField(record, "title", "title"),
    status: requireStringField(record, "status", "status"),
    exitCode: requireNullableNumberField(record, "exitCode", "exit_code"),
    createdAt: requireStringField(record, "createdAt", "created_at"),
    updatedAt: requireStringField(record, "updatedAt", "updated_at"),
    executionBinding:
      rawExecutionBinding === null ? null : normalizeExecutionBinding(rawExecutionBinding),
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
    async launchSession(input, signal) {
      // Snapshot `executionBinding` exactly once: it may be a getter, and
      // re-reading it later (e.g. implicitly during JSON.stringify) could
      // observe a different, unvalidated value than the one checked/
      // normalized here. The wire body below is a single fresh plain object
      // built from this one snapshot and every other supported launch field
      // read directly off `input`, never from `input` itself (no spread, no
      // reused reference), so a custom `toJSON`/prototype/getter on `input`
      // (or a stale/dropped field from an earlier, narrower body shape) can
      // never reach the request. `input` itself is never mutated. A field
      // `input` doesn't set reads as `undefined` here, and `JSON.stringify`
      // always omits an object key whose value is `undefined`, so every
      // absent optional field is naturally dropped from the wire body
      // without any conditional key construction.
      const executionBindingSnapshot = input.executionBinding;
      const body: LaunchCovenSessionInput = {
        projectRoot: input.projectRoot,
        cwd: input.cwd,
        harness: input.harness,
        prompt: input.prompt,
        title: input.title,
        model: input.model,
        launchMode: input.launchMode,
        launchPolicy: input.launchPolicy,
        conversation: input.conversation,
        conversationId: input.conversationId,
        familiarId: input.familiarId,
        callerFamiliarId: input.callerFamiliarId,
        // Validate before any request leaves the process; Rust remains
        // authoritative, this only fails fast on malformed client input.
        // Replaces the snapshot with its validated plain normalized object
        // so no getter/toJSON on it can be re-read at serialization time.
        executionBinding:
          executionBindingSnapshot === undefined
            ? undefined
            : normalizeExecutionBinding(executionBindingSnapshot),
      };
      return requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "POST",
        path: `${COVEN_API_BASE_PATH}/sessions`,
        body,
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
    async sendBoundInput(sessionId, data, executionBinding, signal) {
      const binding = normalizeExecutionBinding(executionBinding);
      await requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "POST",
        path: `${COVEN_API_BASE_PATH}/sessions/${encodeURIComponent(sessionId)}/input`,
        body: { data, executionBinding: binding },
        signal,
      });
    },
    async killBoundSession(sessionId, executionBinding, signal) {
      const binding = normalizeExecutionBinding(executionBinding);
      await requestJson<unknown>({
        socketPath,
        socketRoot: clientOptions.socketRoot,
        method: "POST",
        path: `${COVEN_API_BASE_PATH}/sessions/${encodeURIComponent(sessionId)}/kill`,
        body: { executionBinding: binding },
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
  normalizeExecutionBinding,
};
