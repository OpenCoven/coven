import fs from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, expectTypeOf, it } from "vitest";
import {
  __testing,
  CovenApiError,
  createCovenClient,
  PSYCHE_EXECUTION_BINDING_V1,
  type CovenExecutionBinding,
} from "./client.js";

const DIGEST_A = `sha256:${"a".repeat(64)}`;
const DIGEST_B = `sha256:${"b".repeat(64)}`;
const DIGEST_C = `sha256:${"c".repeat(64)}`;
const DIGEST_D = `sha256:${"d".repeat(64)}`;

function validBinding(overrides: Partial<CovenExecutionBinding> = {}): CovenExecutionBinding {
  return {
    contract: PSYCHE_EXECUTION_BINDING_V1,
    principalRef: "principal:operator",
    familiarId: "sage",
    familiarSnapshotDigest: DIGEST_A,
    projectDigest: DIGEST_B,
    graphId: "graph-1",
    nodeId: "node-1",
    attemptId: "attempt-1",
    requestDigest: DIGEST_C,
    policyRevision: "policy:7",
    expiresAt: "2099-01-01T00:00:00Z",
    parent: null,
    delegationDigest: null,
    ...overrides,
  };
}

function delegatedBinding(): CovenExecutionBinding {
  return validBinding({
    parent: {
      sessionId: "parent-session-1",
      graphId: "graph-1",
      nodeId: "parent-node-1",
      attemptId: "parent-attempt-1",
    },
    delegationDigest: DIGEST_D,
  });
}

let tmpDir: string;

beforeEach(async () => {
  tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-coven-client-"));
  await fs.chmod(tmpDir, 0o700);
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
  await fs.chmod(socketPath, 0o600);
  try {
    await fn(socketPath);
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
    });
  }
}

describe("createCovenClient", () => {
  it("parses daemon JSON over a Unix socket", async () => {
    await withServer(
      (req, res) => {
        expect(req.url).toBe("/api/v1/health");
        res.setHeader("Content-Type", "application/json");
        res.end(
          JSON.stringify({
            ok: true,
            apiVersion: "coven.daemon.v1",
            covenVersion: "0.0.0",
            capabilities: {
              sessions: true,
              events: true,
              eventCursor: "sequence",
              structuredErrors: true,
            },
            daemon: null,
          }),
        );
      },
      async (socketPath) => {
        const health = await createCovenClient(socketPath).health();
        expect(health.ok).toBe(true);
        expect(health.apiVersion).toBe("coven.daemon.v1");
        expect(health.capabilities?.structuredErrors).toBe(true);
        expect(health.daemon).toBeNull();
      },
    );
  });

  it("preserves health contract fields as untrusted wire values", async () => {
    await withServer(
      (_req, res) => {
        res.setHeader("Content-Type", "application/json");
        res.end(
          JSON.stringify({
            apiVersion: "coven.daemon.v1",
            ok: true,
            capabilities: {
              sessions: true,
              events: false,
              eventCursor: "offset",
              structuredErrors: "yes",
            },
          }),
        );
      },
      async (socketPath) => {
        const health = await createCovenClient(socketPath).health();

        expect(health).toEqual({
          apiVersion: "coven.daemon.v1",
          covenVersion: undefined,
          capabilities: {
            sessions: true,
            events: false,
            eventCursor: "offset",
            structuredErrors: "yes",
          },
          ok: true,
          daemon: undefined,
        });
        expectTypeOf(health.apiVersion).toEqualTypeOf<unknown>();
        expectTypeOf(health.capabilities?.sessions).toEqualTypeOf<unknown>();
        expectTypeOf(health.capabilities?.events).toEqualTypeOf<unknown>();
        expectTypeOf(health.capabilities?.eventCursor).toEqualTypeOf<unknown>();
        expectTypeOf(health.capabilities?.structuredErrors).toEqualTypeOf<unknown>();
      },
    );
  });

  it.each([
    ["array", []],
    ["null", null],
    ["string", "sessions"],
  ])("does not treat a capabilities %s as a capability record", (_name, capabilities) => {
    expect(
      __testing.normalizeHealthResponse({
        apiVersion: "coven.daemon.v1",
        capabilities,
        ok: true,
      }).capabilities,
    ).toBeUndefined();
  });

  it.each([
    ["eventCursor", false, "event_cursor", "sequence", false],
    ["structuredErrors", false, "structured_errors", true, false],
    ["eventCursor", null, "event_cursor", "sequence", "sequence"],
    ["structuredErrors", null, "structured_errors", true, true],
  ])(
    "uses nullish camel-case precedence for health capability %s",
    (camelKey, camelValue, snakeKey, snakeValue, expected) => {
      const health = __testing.normalizeHealthResponse({
        capabilities: {
          [camelKey]: camelValue,
          [snakeKey]: snakeValue,
        },
      });

      expect(health.capabilities?.[camelKey as "eventCursor" | "structuredErrors"]).toBe(expected);
    },
  );

  it("preserves executionBindingContracts from the camelCase wire field as untrusted", () => {
    const health = __testing.normalizeHealthResponse({
      capabilities: { executionBindingContracts: [PSYCHE_EXECUTION_BINDING_V1] },
    });
    expect(health.capabilities?.executionBindingContracts).toEqual([PSYCHE_EXECUTION_BINDING_V1]);
    expectTypeOf(health.capabilities?.executionBindingContracts).toEqualTypeOf<unknown>();
  });

  it("preserves executionBindingContracts from the snake_case wire field as untrusted", () => {
    const health = __testing.normalizeHealthResponse({
      capabilities: { execution_binding_contracts: [PSYCHE_EXECUTION_BINDING_V1] },
    });
    expect(health.capabilities?.executionBindingContracts).toEqual([PSYCHE_EXECUTION_BINDING_V1]);
  });

  it("prefers the camelCase executionBindingContracts over snake_case when both are present", () => {
    const health = __testing.normalizeHealthResponse({
      capabilities: {
        executionBindingContracts: ["camel"],
        execution_binding_contracts: ["snake"],
      },
    });
    expect(health.capabilities?.executionBindingContracts).toEqual(["camel"]);
  });

  describe("execution binding", () => {
    describe("session record normalization", () => {
      it("normalizes a snake_case execution_binding into camelCase", () => {
        const binding = validBinding();
        const session = __testing.normalizeSessionRecord({
          id: "session-1",
          project_root: "/repo",
          harness: "codex",
          title: "Fix tests",
          status: "running",
          exit_code: null,
          created_at: "2026-04-27T10:00:00Z",
          updated_at: "2026-04-27T10:00:00Z",
          execution_binding: binding,
        });
        expect(session.executionBinding).toEqual(binding);
      });

      it("normalizes a camelCase executionBinding", () => {
        const binding = validBinding();
        const session = __testing.normalizeSessionRecord({
          id: "session-1",
          projectRoot: "/repo",
          harness: "codex",
          title: "Fix tests",
          status: "running",
          exitCode: null,
          createdAt: "2026-04-27T10:00:00Z",
          updatedAt: "2026-04-27T10:00:00Z",
          executionBinding: binding,
        });
        expect(session.executionBinding).toEqual(binding);
      });

      it("keeps an explicit null execution_binding as null", () => {
        const session = __testing.normalizeSessionRecord({
          id: "session-1",
          project_root: "/repo",
          harness: "codex",
          title: "Fix tests",
          status: "running",
          exit_code: null,
          created_at: "2026-04-27T10:00:00Z",
          updated_at: "2026-04-27T10:00:00Z",
          execution_binding: null,
        });
        expect(session.executionBinding).toBeNull();
      });

      it("normalizes an absent pre-O2 execution_binding field to null (rolling upgrade)", () => {
        const session = __testing.normalizeSessionRecord({
          id: "session-1",
          project_root: "/repo",
          harness: "codex",
          title: "Fix tests",
          status: "running",
          exit_code: null,
          created_at: "2026-04-27T10:00:00Z",
          updated_at: "2026-04-27T10:00:00Z",
          // execution_binding intentionally omitted
        });
        expect(session.executionBinding).toBeNull();
      });

      it("throws when a present execution_binding is invalid", () => {
        expect(() =>
          __testing.normalizeSessionRecord({
            id: "session-1",
            project_root: "/repo",
            harness: "codex",
            title: "Fix tests",
            status: "running",
            exit_code: null,
            created_at: "2026-04-27T10:00:00Z",
            updated_at: "2026-04-27T10:00:00Z",
            execution_binding: { ...validBinding(), contract: "bogus" },
          }),
        ).toThrow(/executionBinding\.contract is unsupported/);
      });
    });

    describe("normalizeExecutionBinding validator", () => {
      it("accepts a fully valid unbound binding", () => {
        expect(__testing.normalizeExecutionBinding(validBinding())).toEqual(validBinding());
      });

      it("accepts a fully valid delegated binding with a parent and delegationDigest", () => {
        const binding = delegatedBinding();
        expect(__testing.normalizeExecutionBinding(binding)).toEqual(binding);
      });

      it("throws when the root object is missing a required key", () => {
        const { policyRevision: _drop, ...incomplete } = validBinding();
        expect(() => __testing.normalizeExecutionBinding(incomplete)).toThrow(
          /executionBinding has missing or unknown fields/,
        );
      });

      it("throws when the root object has an unknown extra key", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), extra: "nope" }),
        ).toThrow(/executionBinding has missing or unknown fields/);
      });

      it("throws when the parent object is missing a required key", () => {
        const binding = delegatedBinding();
        const { sessionId: _drop, ...incompleteParent } = binding.parent!;
        expect(() =>
          __testing.normalizeExecutionBinding({ ...binding, parent: incompleteParent }),
        ).toThrow(/executionBinding\.parent has missing or unknown fields/);
      });

      it("throws when the parent object has an unknown extra key", () => {
        const binding = delegatedBinding();
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...binding,
            parent: { ...binding.parent, extra: "nope" },
          }),
        ).toThrow(/executionBinding\.parent has missing or unknown fields/);
      });

      it("rejects an unsupported contract value", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), contract: "psyche.other.v1" }),
        ).toThrow(/executionBinding\.contract is unsupported/);
      });

      it.each([
        ["principalRef"],
        ["familiarId"],
        ["graphId"],
        ["nodeId"],
        ["attemptId"],
        ["policyRevision"],
      ] as const)("rejects an empty opaque value for %s", (key) => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), [key]: "" }),
        ).toThrow(new RegExp(`executionBinding\\.${key} is invalid`));
      });

      it("rejects an opaque value over the 255-byte boundary", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            principalRef: "a".repeat(256),
          }),
        ).toThrow(/executionBinding\.principalRef is invalid/);
      });

      it("accepts an opaque value at the exact 255-byte boundary", () => {
        expect(
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            principalRef: "a".repeat(255),
          }).principalRef,
        ).toHaveLength(255);
      });

      it("rejects an opaque value containing a disallowed character", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), graphId: "graph 1" }),
        ).toThrow(/executionBinding\.graphId is invalid/);
      });

      it("rejects an opaque value containing non-ASCII bytes", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), nodeId: "nöde-1" }),
        ).toThrow(/executionBinding\.nodeId is invalid/);
      });

      it.each([
        ["familiarSnapshotDigest"],
        ["projectDigest"],
        ["requestDigest"],
      ] as const)("rejects an uppercase-hex digest for %s", (key) => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            [key]: `sha256:${"A".repeat(64)}`,
          }),
        ).toThrow(new RegExp(`executionBinding\\.${key} is invalid`));
      });

      it("rejects a digest with the wrong hex length", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            requestDigest: `sha256:${"c".repeat(63)}`,
          }),
        ).toThrow(/executionBinding\.requestDigest is invalid/);
      });

      it("rejects a digest missing the sha256: prefix", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            requestDigest: "c".repeat(64),
          }),
        ).toThrow(/executionBinding\.requestDigest is invalid/);
      });

      it("rejects a delegationDigest that is not a valid digest", () => {
        const binding = delegatedBinding();
        expect(() =>
          __testing.normalizeExecutionBinding({ ...binding, delegationDigest: "not-a-digest" }),
        ).toThrow(/executionBinding\.delegationDigest is invalid/);
      });

      it("rejects a fractional-second expiresAt", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            expiresAt: "2099-01-01T00:00:00.000Z",
          }),
        ).toThrow(/executionBinding\.expiresAt is invalid/);
      });

      it("rejects a non-UTC offset expiresAt", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            expiresAt: "2099-01-01T00:00:00+00:00",
          }),
        ).toThrow(/executionBinding\.expiresAt is invalid/);
      });

      it("rejects a calendar-invalid expiresAt that fails the canonical round-trip", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            expiresAt: "2099-02-30T00:00:00Z",
          }),
        ).toThrow(/executionBinding\.expiresAt is invalid/);
      });

      it("rejects a parent present with a null delegationDigest (parity violation)", () => {
        const binding = delegatedBinding();
        expect(() =>
          __testing.normalizeExecutionBinding({ ...binding, delegationDigest: null }),
        ).toThrow(/executionBinding parent\/delegationDigest relationship is invalid/);
      });

      it("rejects a null parent with a non-null delegationDigest (parity violation)", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), delegationDigest: DIGEST_D }),
        ).toThrow(/executionBinding parent\/delegationDigest relationship is invalid/);
      });

      it("does not trim or normalize opaque values", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({ ...validBinding(), familiarId: " sage " }),
        ).toThrow(/executionBinding\.familiarId is invalid/);
      });
    });

    describe("bound client requests", () => {
      it("sends the exact executionBinding object on a bound session launch", async () => {
        const binding = validBinding();
        let capturedBody = "";
        await withServer(
          (req, res) => {
            let body = "";
            req.on("data", (chunk: string) => {
              body += chunk;
            });
            req.on("end", () => {
              capturedBody = body;
              res.statusCode = 201;
              res.setHeader("Content-Type", "application/json");
              res.end(
                JSON.stringify({
                  id: "session-1",
                  project_root: "/repo",
                  harness: "codex",
                  title: "Fix tests",
                  status: "running",
                  exit_code: null,
                  created_at: "2026-04-27T10:00:00Z",
                  updated_at: "2026-04-27T10:00:00Z",
                  execution_binding: binding,
                }),
              );
            });
          },
          async (socketPath) => {
            const session = await createCovenClient(socketPath).launchSession({
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              executionBinding: binding,
            });
            expect(JSON.parse(capturedBody).executionBinding).toEqual(binding);
            expect(session.executionBinding).toEqual(binding);
          },
        );
      });

      it("sendBoundInput sends exactly { data, executionBinding }", async () => {
        const binding = validBinding();
        let capturedBody = "";
        await withServer(
          (req, res) => {
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
            await createCovenClient(socketPath).sendBoundInput(
              "session-1",
              "hello\n",
              binding,
            );
            expect(JSON.parse(capturedBody)).toEqual({ data: "hello\n", executionBinding: binding });
          },
        );
      });

      it("killBoundSession sends exactly { executionBinding }", async () => {
        const binding = validBinding();
        let capturedBody = "";
        await withServer(
          (req, res) => {
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
            await createCovenClient(socketPath).killBoundSession("session-1", binding);
            expect(JSON.parse(capturedBody)).toEqual({ executionBinding: binding });
          },
        );
      });

      it("rejects an invalid executionBinding on launchSession before any HTTP request", async () => {
        const missingSocket = path.join(tmpDir, "definitely-missing.sock");
        await expect(
          createCovenClient(missingSocket).launchSession({
            projectRoot: "/repo",
            cwd: "/repo",
            harness: "codex",
            prompt: "Fix tests",
            title: "Fix tests",
            executionBinding: { ...validBinding(), extra: "nope" } as CovenExecutionBinding,
          }),
        ).rejects.toThrow(/executionBinding has missing or unknown fields/);
      });

      it("rejects an unknown root key on sendBoundInput before any HTTP request", async () => {
        const missingSocket = path.join(tmpDir, "definitely-missing.sock");
        await expect(
          createCovenClient(missingSocket).sendBoundInput(
            "session-1",
            "hello\n",
            { ...validBinding(), extra: "nope" } as CovenExecutionBinding,
          ),
        ).rejects.toThrow(/executionBinding has missing or unknown fields/);
      });

      it("rejects an unknown parent key on killBoundSession before any HTTP request", async () => {
        const missingSocket = path.join(tmpDir, "definitely-missing.sock");
        const binding = delegatedBinding();
        await expect(
          createCovenClient(missingSocket).killBoundSession("session-1", {
            ...binding,
            parent: { ...binding.parent, extra: "nope" },
          } as CovenExecutionBinding),
        ).rejects.toThrow(/executionBinding\.parent has missing or unknown fields/);
      });

      it("legacy sendInput body remains { data } only when no binding is supplied", async () => {
        let capturedBody = "";
        await withServer(
          (req, res) => {
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
            await createCovenClient(socketPath).sendInput("session-1", "hello\n");
            expect(JSON.parse(capturedBody)).toEqual({ data: "hello\n" });
          },
        );
      });

      it("legacy killSession body remains empty when no binding is supplied", async () => {
        let capturedBody = "";
        await withServer(
          (req, res) => {
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
            await createCovenClient(socketPath).killSession("session-1");
            expect(capturedBody).toBe("");
          },
        );
      });
    });
  });

  it("validates a real socket inside the configured socket root", async () => {
    await withServer(
      (_req, res) => {
        res.setHeader("Content-Type", "application/json");
        res.end(
          JSON.stringify({
            ok: true,
            apiVersion: "coven.daemon.v1",
            covenVersion: "0.0.0",
            capabilities: {
              sessions: true,
              events: true,
              eventCursor: "sequence",
              structuredErrors: true,
            },
            daemon: null,
          }),
        );
      },
      async (socketPath) => {
        const health = await createCovenClient(socketPath, { socketRoot: tmpDir }).health();
        expect(health.ok).toBe(true);
        expect(health.apiVersion).toBe("coven.daemon.v1");
      },
    );
  });

  it("sends the event cursor through the versioned API path when listing events", async () => {
    await withServer(
      (req, res) => {
        expect(req.url).toBe("/api/v1/events?sessionId=session-1&afterEventId=event-1");
        res.setHeader("Content-Type", "application/json");
        res.end(JSON.stringify({ events: [], nextCursor: null, hasMore: false }));
      },
      async (socketPath) => {
        await expect(
          createCovenClient(socketPath).listEvents("session-1", { afterEventId: "event-1" }),
        ).resolves.toEqual([]);
      },
    );
  });

  it("normalizes snake_case session records returned by the Rust daemon", async () => {
    await withServer(
      (_req, res) => {
        res.setHeader("Content-Type", "application/json");
        res.end(
          JSON.stringify({
            id: "session-1",
            project_root: tmpDir,
            harness: "codex",
            title: "Smoke",
            status: "completed",
            exit_code: 0,
            created_at: "2026-04-27T10:00:00Z",
            updated_at: "2026-04-27T10:00:01Z",
          }),
        );
      },
      async (socketPath) => {
        await expect(createCovenClient(socketPath).getSession("session-1")).resolves.toEqual({
          id: "session-1",
          projectRoot: tmpDir,
          harness: "codex",
          title: "Smoke",
          status: "completed",
          exitCode: 0,
          createdAt: "2026-04-27T10:00:00Z",
          updatedAt: "2026-04-27T10:00:01Z",
          executionBinding: null,
        });
      },
    );
  });

  it("normalizes snake_case event records returned by the Rust daemon", async () => {
    await withServer(
      (_req, res) => {
        res.setHeader("Content-Type", "application/json");
        res.end(
          JSON.stringify({
            events: [
              {
                seq: 1,
                id: "event-1",
                session_id: "session-1",
                kind: "output",
                payload_json: JSON.stringify({ data: "hello" }),
                created_at: "2026-04-27T10:00:00Z",
              },
            ],
            nextCursor: { afterSeq: 1 },
            hasMore: false,
          }),
        );
      },
      async (socketPath) => {
        await expect(createCovenClient(socketPath).listEvents("session-1")).resolves.toEqual([
          {
            seq: 1,
            id: "event-1",
            sessionId: "session-1",
            kind: "output",
            payloadJson: JSON.stringify({ data: "hello" }),
            createdAt: "2026-04-27T10:00:00Z",
          },
        ]);
      },
    );
  });

  it("rejects oversized event cursors before building the events URL", () => {
    expect(() =>
      createCovenClient("/tmp/coven.sock").listEvents("session-1", {
        afterEventId: "e".repeat(257),
      }),
    ).toThrow(/event id is invalid/);
  });

  it("wraps invalid daemon JSON in a typed API error", async () => {
    await withServer(
      (_req, res) => {
        res.end("{not json");
      },
      async (socketPath) => {
        await expect(createCovenClient(socketPath).health()).rejects.toBeInstanceOf(CovenApiError);
      },
    );
  });

  it("rejects daemon responses above the response size limit", async () => {
    await withServer(
      (_req, res) => {
        res.end("x".repeat(1_000_001));
      },
      async (socketPath) => {
        await expect(createCovenClient(socketPath).health()).rejects.toThrow(/size limit/);
      },
    );
  });

  it("rejects request bodies above the request size limit", async () => {
    await withServer(
      (_req, res) => {
        res.end("{}");
      },
      async (socketPath) => {
        await expect(
          createCovenClient(socketPath).launchSession({
            projectRoot: "/repo",
            cwd: "/repo",
            harness: "codex",
            prompt: "x".repeat(1_000_001),
            title: "Large prompt",
          }),
        ).rejects.toThrow(/request exceeded size limit/);
      },
    );
  });

  it("revalidates socket paths before connecting", async () => {
    const covenHome = path.join(tmpDir, ".coven");
    await fs.mkdir(covenHome);
    await fs.chmod(covenHome, 0o700);
    const socketPath = path.join(covenHome, "coven.sock");
    await fs.symlink("/var/run/docker.sock", socketPath);

    await expect(createCovenClient(socketPath, { socketRoot: covenHome }).health()).rejects.toThrow(
      /must not be a symlink/,
    );
  });

  it("rejects a socket root that resolves through a symlink", async () => {
    const realHome = path.join(tmpDir, "real-coven");
    const symlinkHome = path.join(tmpDir, "symlink-coven");
    await fs.mkdir(realHome);
    await fs.chmod(realHome, 0o700);
    await fs.symlink(realHome, symlinkHome);

    await expect(
      createCovenClient(path.join(symlinkHome, "coven.sock"), { socketRoot: symlinkHome }).health(),
    ).rejects.toThrow(/covenHome must not be a symlink/);
  });

  it("rejects missing socket roots with a validation error", async () => {
    const covenHome = path.join(tmpDir, "missing-coven");

    await expect(
      createCovenClient(path.join(covenHome, "coven.sock"), { socketRoot: covenHome }).health(),
    ).rejects.toThrow(/covenHome must exist/);
  });

  it("rejects a group or world writable socket root", async () => {
    if (process.platform === "win32") {
      return;
    }
    const covenHome = path.join(tmpDir, ".coven");
    await fs.mkdir(covenHome);
    await fs.chmod(covenHome, 0o777);

    await expect(
      createCovenClient(path.join(covenHome, "coven.sock"), { socketRoot: covenHome }).health(),
    ).rejects.toThrow(/covenHome must not be group or world writable/);
  });

  it("rejects socket paths that are not Unix sockets", async () => {
    const covenHome = path.join(tmpDir, ".coven");
    await fs.mkdir(covenHome);
    await fs.chmod(covenHome, 0o700);
    const socketPath = path.join(covenHome, "coven.sock");
    await fs.writeFile(socketPath, "");

    await expect(createCovenClient(socketPath, { socketRoot: covenHome }).health()).rejects.toThrow(
      /must be a Unix socket/,
    );
  });

  it("rejects socket path overrides even when they are inside covenHome", async () => {
    const covenHome = path.join(tmpDir, ".coven");
    await fs.mkdir(covenHome);
    await fs.chmod(covenHome, 0o700);
    const socketPath = path.join(covenHome, "other.sock");
    const server = http.createServer((_req, res) => {
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify({ ok: true, daemon: null }));
    });
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(socketPath, () => resolve());
    });
    try {
      await expect(
        createCovenClient(socketPath, { socketRoot: covenHome }).health(),
      ).rejects.toThrow(/socketPath must be <covenHome>\/coven\.sock/);
    } finally {
      await new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
    }
  });

  it("fails closed instead of bypassing socket validation on Windows", () => {
    expect(() =>
      __testing.validateSocketPathForUse(
        path.join(tmpDir, ".coven", "coven.sock"),
        path.join(tmpDir, ".coven"),
        "win32",
      ),
    ).toThrow(/not supported on Windows/);
  });
});
