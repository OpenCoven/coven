/**
 * Compatibility tests for the @opencoven/coven OpenClaw bridge.
 *
 * These tests verify that the plugin client correctly handles representative
 * Coven daemon API responses. The fixture files under src/fixtures/v2026.4/
 * capture the JSON shapes produced by the Rust daemon at the v2026.4 API.
 *
 * Supported version pair:
 *   - Plugin:  @opencoven/coven 2026.4.28
 *   - Daemon:  Coven (coven-cli) built from the same repo at 2026.4.x
 *
 * When the Rust daemon changes a response shape for /health, /sessions, /events,
 * input, or kill behavior, update the matching fixture file and re-run these
 * tests to confirm the plugin handles the new shape correctly.
 */

import fs from "node:fs/promises";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CovenApiError, createCovenClient } from "./client.js";

const FIXTURES_DIR = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "fixtures",
  "v2026.4",
);

async function loadFixture(name: string): Promise<string> {
  return fs.readFile(path.join(FIXTURES_DIR, `${name}.json`), "utf8");
}

let tmpDir: string;

beforeEach(async () => {
  tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-coven-compat-"));
});

afterEach(async () => {
  await fs.rm(tmpDir, { recursive: true, force: true });
});

async function withServer(
  handler: http.RequestListener,
  fn: (socketPath: string) => Promise<void>,
): Promise<void> {
  const socketPath = path.join(tmpDir, "coven.sock");
  const server = http.createServer(handler);
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => resolve());
  });
  try {
    await fn(socketPath);
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
    });
  }
}

describe("Coven daemon API compatibility — v2026.4", () => {
  // ---------------------------------------------------------------------------
  // Available daemon state
  // ---------------------------------------------------------------------------

  describe("available daemon state", () => {
    it("parses a healthy daemon health response (daemon status present)", async () => {
      const fixture = await loadFixture("health-available");
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const health = await createCovenClient(socketPath).health();
          expect(health.ok).toBe(true);
          expect(health.apiVersion).toBe("coven.daemon.v1");
          expect(health.capabilities).toMatchObject({
            sessions: true,
            events: true,
            eventCursor: "sequence",
            structuredErrors: true,
          });
          expect(health.daemon).toMatchObject({
            pid: expect.any(Number),
            startedAt: expect.any(String),
            socket: expect.any(String),
          });
        },
      );
    });

    it("parses a health response when daemon metadata is null", async () => {
      const fixture = await loadFixture("health-daemon-null");
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const health = await createCovenClient(socketPath).health();
          expect(health.ok).toBe(true);
          expect(health.apiVersion).toBe("coven.daemon.v1");
          expect(health.daemon).toBeNull();
        },
      );
    });

    it("normalizes a v2026.4 running session record (snake_case → camelCase)", async () => {
      const fixture = await loadFixture("session-running");
      await withServer(
        (_req, res) => {
          res.statusCode = 201;
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const session = await createCovenClient(socketPath).launchSession({
            projectRoot: "/home/user/myproject",
            cwd: "/home/user/myproject",
            harness: "codex",
            prompt: "Fix failing tests",
            title: "Fix failing tests",
          });
          expect(session.id).toBe("550e8400-e29b-41d4-a716-446655440001");
          expect(session.projectRoot).toBe("/home/user/myproject");
          expect(session.harness).toBe("codex");
          expect(session.title).toBe("Fix failing tests");
          expect(session.status).toBe("running");
          expect(session.exitCode).toBeNull();
          expect(session.createdAt).toBe("2026-04-28T09:10:00.000Z");
          expect(session.updatedAt).toBe("2026-04-28T09:12:00.000Z");
        },
      );
    });

    it("normalizes a v2026.4 completed session record", async () => {
      const fixture = await loadFixture("session-completed");
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const session = await createCovenClient(socketPath).getSession(
            "550e8400-e29b-41d4-a716-446655440000",
          );
          expect(session.id).toBe("550e8400-e29b-41d4-a716-446655440000");
          expect(session.projectRoot).toBe("/home/user/myproject");
          expect(session.harness).toBe("codex");
          expect(session.title).toBe("Implement authentication");
          expect(session.status).toBe("completed");
          expect(session.exitCode).toBe(0);
        },
      );
    });

    it("normalizes v2026.4 event records: output events followed by an exit event", async () => {
      const fixture = await loadFixture("events-output-exit");
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const events = await createCovenClient(socketPath).listEvents(
            "550e8400-e29b-41d4-a716-446655440001",
          );
          expect(events).toHaveLength(3);
          expect(events[0]).toMatchObject({
            seq: 1,
            id: "event-0001",
            sessionId: "550e8400-e29b-41d4-a716-446655440001",
            kind: "output",
            payloadJson: expect.stringContaining("Analyzing codebase"),
            createdAt: "2026-04-28T09:10:01.000Z",
          });
          expect(events[1]).toMatchObject({
            seq: 2,
            id: "event-0002",
            kind: "output",
            payloadJson: expect.stringContaining("Done"),
          });
          expect(events[2]).toMatchObject({
            seq: 3,
            id: "event-0003",
            kind: "exit",
            payloadJson: expect.stringContaining("completed"),
            createdAt: "2026-04-28T09:10:03.000Z",
          });
        },
      );
    });

    it("sends the afterEventId cursor when listing incremental events", async () => {
      const fixture = await loadFixture("events-output-exit");
      await withServer(
        (req, res) => {
          expect(req.url).toContain("afterEventId=event-0001");
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const events = await createCovenClient(socketPath).listEvents(
            "550e8400-e29b-41d4-a716-446655440001",
            { afterEventId: "event-0001" },
          );
          expect(events).toHaveLength(3);
        },
      );
    });

    it("sends the afterSeq cursor when listing incremental events", async () => {
      const fixture = await loadFixture("events-output-exit");
      await withServer(
        (req, res) => {
          expect(req.url).toContain("afterSeq=2");
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const events = await createCovenClient(socketPath).listEvents(
            "550e8400-e29b-41d4-a716-446655440001",
            { afterSeq: 2 },
          );
          expect(events).toHaveLength(3);
        },
      );
    });

    it("sends the limit parameter when listing events with a page size", async () => {
      const fixture = await loadFixture("events-output-exit");
      await withServer(
        (req, res) => {
          expect(req.url).toContain("limit=10");
          res.setHeader("Content-Type", "application/json");
          res.end(fixture);
        },
        async (socketPath) => {
          const events = await createCovenClient(socketPath).listEvents(
            "550e8400-e29b-41d4-a716-446655440001",
            { limit: 10 },
          );
          expect(events).toHaveLength(3);
        },
      );
    });

    it("v2026.4 sessions-list fixture uses daemon snake_case field names", async () => {
      const fixture = await loadFixture("sessions-list");
      const sessions = JSON.parse(fixture) as unknown[];
      expect(Array.isArray(sessions)).toBe(true);
      expect(sessions).toHaveLength(2);
      // The Rust daemon serializes SessionRecord without a rename attribute, so
      // all multi-word fields remain in snake_case.
      expect(sessions[0]).toMatchObject({
        id: expect.any(String),
        project_root: expect.any(String),
        harness: "codex",
        status: "completed",
        exit_code: 0,
        created_at: expect.any(String),
        updated_at: expect.any(String),
      });
      expect(sessions[1]).toMatchObject({
        harness: "claude",
        status: "running",
        exit_code: null,
      });
    });

    it("normalizes sessions from the sessions-list fixture via getSession", async () => {
      const rawSessions = JSON.parse(await loadFixture("sessions-list")) as unknown[];
      const rawFirst = rawSessions[0];
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify(rawFirst));
        },
        async (socketPath) => {
          const session = await createCovenClient(socketPath).getSession(
            "550e8400-e29b-41d4-a716-446655440000",
          );
          expect(session.projectRoot).toBe("/home/user/myproject");
          expect(session.exitCode).toBe(0);
          expect(session.status).toBe("completed");
        },
      );
    });

    it("accepts a 202 response from sendInput and forwards the data field", async () => {
      let capturedBody = "";
      await withServer(
        (req, res) => {
          // Verify the client posts to the correct input path
          expect(req.url).toBe(
            "/api/v1/sessions/550e8400-e29b-41d4-a716-446655440001/input",
          );
          let body = "";
          req.on("data", (chunk: string) => {
            body += chunk;
          });
          req.on("end", () => {
            capturedBody = body;
            res.statusCode = 202;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify({ ok: true, accepted: true }));
          });
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).sendInput(
              "550e8400-e29b-41d4-a716-446655440001",
              "fix the test\n",
            ),
          ).resolves.toBeUndefined();
          expect(JSON.parse(capturedBody)).toEqual({ data: "fix the test\n" });
        },
      );
    });

    it("accepts a 202 response from killSession", async () => {
      await withServer(
        (req, res) => {
          // Verify the client posts to the correct kill path
          expect(req.url).toBe(
            "/api/v1/sessions/550e8400-e29b-41d4-a716-446655440001/kill",
          );
          res.statusCode = 202;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ ok: true, accepted: true }));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).killSession(
              "550e8400-e29b-41d4-a716-446655440001",
            ),
          ).resolves.toBeUndefined();
        },
      );
    });
  });

  // ---------------------------------------------------------------------------
  // Unavailable daemon state
  // ---------------------------------------------------------------------------

  describe("unavailable daemon state", () => {
    it("throws when the daemon socket does not exist", async () => {
      const missingSocket = path.join(tmpDir, "coven.sock");
      await expect(createCovenClient(missingSocket).health()).rejects.toThrow();
    });

    it("throws when the daemon closes the connection immediately", async () => {
      const socketPath = path.join(tmpDir, "coven.sock");
      // Simulate a daemon that is in the process of shutting down: it accepts
      // the TCP-level connection but immediately closes it without sending HTTP.
      const server = net.createServer((socket) => {
        socket.destroy();
      });
      await new Promise<void>((resolve, reject) => {
        server.once("error", reject);
        server.listen(socketPath, () => resolve());
      });
      try {
        await expect(createCovenClient(socketPath).health()).rejects.toThrow();
      } finally {
        await new Promise<void>((resolve, reject) => {
          server.close((error) => (error ? reject(error) : resolve()));
        });
      }
    });
  });

  // ---------------------------------------------------------------------------
  // Incompatible daemon state
  // ---------------------------------------------------------------------------

  describe("incompatible daemon state", () => {
    it("throws a CovenApiError when the daemon returns a non-2xx status on session launch", async () => {
      await withServer(
        (_req, res) => {
          res.statusCode = 503;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "daemon overloaded" }));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).launchSession({
              projectRoot: "/home/user/myproject",
              cwd: "/home/user/myproject",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
            }),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });

    it("throws a CovenApiError when the daemon returns a non-2xx status on event listing", async () => {
      await withServer(
        (_req, res) => {
          res.statusCode = 404;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "session not found" }));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).listEvents("no-such-session"),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });

    it("throws when a session record is missing a required field (incompatible schema)", async () => {
      // A daemon that drops the 'status' field would be incompatible with this
      // plugin version. The client validates required fields and throws on
      // missing ones, so OpenClaw receives a clear error rather than silently
      // using a malformed object.
      const incompatible = JSON.stringify({
        id: "some-session",
        project_root: "/home/user/myproject",
        harness: "codex",
        title: "Missing status",
        // status intentionally omitted
        exit_code: null,
        created_at: "2026-04-28T09:00:00.000Z",
        updated_at: "2026-04-28T09:00:00.000Z",
      });
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(incompatible);
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).getSession("some-session"),
          ).rejects.toThrow(/status.*invalid/i);
        },
      );
    });

    it("throws when an event record is missing a required field (incompatible schema)", async () => {
      // A daemon that renames 'payload_json' (e.g., to 'payload') would break
      // event parsing. The client validates the field is present and throws.
      const incompatible = JSON.stringify({
        events: [
          {
            seq: 1,
            id: "event-1",
            session_id: "some-session",
            kind: "output",
            // payload_json intentionally omitted
            created_at: "2026-04-28T09:10:01.000Z",
          },
        ],
        nextCursor: null,
        hasMore: false,
      });
      await withServer(
        (_req, res) => {
          res.setHeader("Content-Type", "application/json");
          res.end(incompatible);
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).listEvents("some-session"),
          ).rejects.toThrow(/payloadJson.*invalid/i);
        },
      );
    });

    it("throws a CovenApiError when the daemon returns invalid JSON", async () => {
      await withServer(
        (_req, res) => {
          res.end("{not valid json");
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).health(),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });

    it("throws a CovenApiError when sendInput receives a 404 (session not found)", async () => {
      // The daemon returns 404 when the session id does not exist.
      await withServer(
        (_req, res) => {
          res.statusCode = 404;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "session not found" }));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).sendInput("no-such-session", "hello\n"),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });

    it("throws a CovenApiError when sendInput receives a 409 (session not live)", async () => {
      // The daemon returns 409 when the session exists but is no longer running
      // (completed, killed, or failed). The plugin must not swallow this error.
      await withServer(
        (_req, res) => {
          res.statusCode = 409;
          res.setHeader("Content-Type", "application/json");
          res.end(
            JSON.stringify({
              error: "session not live",
              sessionId: "550e8400-e29b-41d4-a716-446655440000",
            }),
          );
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).sendInput(
              "550e8400-e29b-41d4-a716-446655440000",
              "too late\n",
            ),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });

    it("throws a CovenApiError when killSession receives a 404 (session not found)", async () => {
      await withServer(
        (_req, res) => {
          res.statusCode = 404;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "session not found" }));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).killSession("no-such-session"),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });

    it("throws a CovenApiError when killSession receives a 409 (session not live)", async () => {
      // The daemon returns 409 when the session is already completed/killed.
      await withServer(
        (_req, res) => {
          res.statusCode = 409;
          res.setHeader("Content-Type", "application/json");
          res.end(
            JSON.stringify({
              error: "session not live",
              sessionId: "550e8400-e29b-41d4-a716-446655440000",
            }),
          );
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).killSession(
              "550e8400-e29b-41d4-a716-446655440000",
            ),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
    });
  });
});
