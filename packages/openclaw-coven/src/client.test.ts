import fs from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, expectTypeOf, it } from "vitest";
import * as clientModule from "./client.js";
import {
  __testing,
  CovenApiError,
  createCovenClient,
  PSYCHE_EXECUTION_BINDING_V1,
  type CovenExecutionBinding,
  type LaunchCovenSessionInput,
} from "./client.js";

const DIGEST_A = `sha256:${"a".repeat(64)}`;
const DIGEST_B = `sha256:${"b".repeat(64)}`;
const DIGEST_C = `sha256:${"c".repeat(64)}`;
const DIGEST_D = `sha256:${"d".repeat(64)}`;
const REQUEST_ADOPTION_CONTRACT = "psyche.request_adoption.v1" as const;

type TestRequestAdoption = {
  contract: typeof REQUEST_ADOPTION_CONTRACT;
  key: string;
  requestDigest: string;
};

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

function validAdoption(
  overrides: Partial<TestRequestAdoption> = {},
): TestRequestAdoption {
  return {
    contract: REQUEST_ADOPTION_CONTRACT,
    key: "psyche:graph-1/node-1/attempt-1/request-1",
    requestDigest: DIGEST_C,
    ...overrides,
  };
}

function o3Health(requestAdoptionContracts: unknown = [REQUEST_ADOPTION_CONTRACT]) {
  return {
    ok: true,
    apiVersion: "coven.daemon.v1",
    covenVersion: "0.0.0",
    capabilities: {
      sessions: true,
      events: true,
      eventCursor: "sequence",
      structuredErrors: true,
      requestAdoptionContracts,
    },
    daemon: null,
  };
}

function sessionWire(executionBinding: CovenExecutionBinding = validBinding()) {
  return {
    id: "session-1",
    project_root: "/repo",
    harness: "codex",
    title: "Fix tests",
    status: "running",
    exit_code: null,
    created_at: "2026-04-27T10:00:00Z",
    updated_at: "2026-04-27T10:00:01Z",
    execution_binding: executionBinding,
  };
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

  it("exports the exact O3 request-adoption contract literal", () => {
    expect(
      (clientModule as unknown as Record<string, unknown>).PSYCHE_REQUEST_ADOPTION_V1,
    ).toBe(REQUEST_ADOPTION_CONTRACT);
  });

  it("preserves requestAdoptionContracts from the camelCase wire field as untrusted", () => {
    const additiveValue = [REQUEST_ADOPTION_CONTRACT, { future: true }];
    const health = __testing.normalizeHealthResponse({
      capabilities: { requestAdoptionContracts: additiveValue },
    });
    expect(health.capabilities?.requestAdoptionContracts).toEqual(additiveValue);
    expectTypeOf(
      health.capabilities?.requestAdoptionContracts,
    ).toEqualTypeOf<unknown>();
  });

  it("normalizes snake_case request_adoption_contracts without narrowing it", () => {
    const additiveValue = [REQUEST_ADOPTION_CONTRACT, 7];
    const health = __testing.normalizeHealthResponse({
      capabilities: { request_adoption_contracts: additiveValue },
    });
    expect(health.capabilities?.requestAdoptionContracts).toEqual(additiveValue);
  });

  it("prefers camelCase requestAdoptionContracts when both health fields are present", () => {
    const health = __testing.normalizeHealthResponse({
      capabilities: {
        requestAdoptionContracts: ["camel"],
        request_adoption_contracts: ["snake"],
      },
    });
    expect(health.capabilities?.requestAdoptionContracts).toEqual(["camel"]);
  });

  it.each([
    ["null", null],
    ["object", { contract: REQUEST_ADOPTION_CONTRACT }],
    ["string", REQUEST_ADOPTION_CONTRACT],
    ["undefined", undefined],
  ])(
    "does not fall back from a present %s canonical request-adoption capability",
    (_name, canonicalValue) => {
      const capabilities = {
        requestAdoptionContracts: canonicalValue,
        request_adoption_contracts: [REQUEST_ADOPTION_CONTRACT],
      };
      const health = __testing.normalizeHealthResponse({ capabilities });

      expect(Object.hasOwn(capabilities, "requestAdoptionContracts")).toBe(true);
      expect(health.capabilities?.requestAdoptionContracts).toBe(canonicalValue);
    },
  );

  it("reads the canonical request-adoption capability getter exactly once", () => {
    let reads = 0;
    const capabilities: Record<string, unknown> = {
      request_adoption_contracts: [REQUEST_ADOPTION_CONTRACT],
    };
    Object.defineProperty(capabilities, "requestAdoptionContracts", {
      enumerable: true,
      get() {
        reads += 1;
        return undefined;
      },
    });

    const health = __testing.normalizeHealthResponse({ capabilities });

    expect(health.capabilities?.requestAdoptionContracts).toBeUndefined();
    expect(reads).toBe(1);
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

      it("rejects a calendar-invalid expiresAt (February 30th)", () => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            expiresAt: "2099-02-30T00:00:00Z",
          }),
        ).toThrow(/executionBinding\.expiresAt is invalid/);
      });

      // Parity with the Rust contract's Chrono-backed `parse_expiry`: Chrono
      // accepts the whole-second leap-second value (`:60`) on every minute
      // boundary, not only real UTC leap seconds, and this validator must
      // accept exactly the same canonical forms without normalizing the
      // stored string. See crates/coven-cli/src/execution_binding.rs.
      it.each([
        "2016-12-31T23:59:60Z",
        "2020-03-15T08:30:60Z",
        "2016-04-30T23:59:60Z",
        "0000-02-29T23:59:60Z",
        "2000-02-29T23:59:60Z",
      ])("accepts the leap-second value :60 at %s (Chrono parity)", (expiresAt) => {
        const binding = __testing.normalizeExecutionBinding({
          ...validBinding(),
          expiresAt,
        });
        // Not normalized: the stored string is returned byte-exact.
        expect(binding.expiresAt).toBe(expiresAt);
      });

      it.each([
        ["2020-03-15T08:30:61Z", "seconds above 60 are always invalid"],
        ["2016-12-31T23:60:00Z", "minute 60 is invalid even alongside a valid second"],
        ["2016-12-31T24:00:60Z", "hour 24 is invalid even alongside a leap second"],
        ["2016-04-31T23:59:60Z", "April has no 31st, leap second or not"],
        ["2001-02-29T23:59:60Z", "February 29th in a non-leap year is invalid"],
        ["1900-02-29T23:59:60Z", "1900 is not a Gregorian leap year (divisible by 100, not 400)"],
      ] as const)("rejects %s (%s)", (expiresAt, _reason) => {
        expect(() =>
          __testing.normalizeExecutionBinding({
            ...validBinding(),
            expiresAt,
          }),
        ).toThrow(/executionBinding\.expiresAt is invalid/);
      });

      it("accepts February 29th in a Gregorian leap year (divisible by 400)", () => {
        const binding = __testing.normalizeExecutionBinding({
          ...validBinding(),
          expiresAt: "2000-02-29T00:00:00Z",
        });
        expect(binding.expiresAt).toBe("2000-02-29T00:00:00Z");
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

      it("sends the validated normalized executionBinding, not a poisoned toJSON, on launchSession", async () => {
        const binding = validBinding();
        // A closed-object membership check on Object.keys() cannot see a
        // non-enumerable `toJSON`, but JSON.stringify still calls it via a
        // plain property [[Get]], so this is a realistic bypass of the
        // exact-keys check that only a fresh-body rebuild defeats.
        const poisoned = { ...binding };
        Object.defineProperty(poisoned, "toJSON", {
          enumerable: false,
          configurable: true,
          value: () => ({ contract: "evil", tampered: true }),
        });
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
            const input = {
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              executionBinding: poisoned as unknown as CovenExecutionBinding,
            };
            await createCovenClient(socketPath).launchSession(input);
            expect(JSON.parse(capturedBody).executionBinding).toEqual(binding);
            // The original input is untouched: same executionBinding reference.
            expect(input.executionBinding).toBe(poisoned);
          },
        );
      });

      it("sends the value validated on first read, not a later getter re-read, on launchSession", async () => {
        const binding = validBinding();
        let principalRefReads = 0;
        // A getter is a time-of-check/time-of-use trap: it can answer
        // validation's single read with a valid value, then answer a later
        // re-serialization read with something else entirely.
        const trapped = { ...binding };
        Object.defineProperty(trapped, "principalRef", {
          enumerable: true,
          configurable: true,
          get() {
            principalRefReads += 1;
            return principalRefReads === 1 ? binding.principalRef : "principal:tampered-reread";
          },
        });
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
            const input = {
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              executionBinding: trapped as unknown as CovenExecutionBinding,
            };
            await createCovenClient(socketPath).launchSession(input);
            const sentBinding = JSON.parse(capturedBody).executionBinding;
            expect(sentBinding).toEqual(binding);
            expect(sentBinding.principalRef).toBe(binding.principalRef);
            expect(principalRefReads).toBe(1);
            // The original input object is never mutated.
            expect(input.executionBinding).toBe(trapped);
            expect(input.projectRoot).toBe("/repo");
            expect(Object.keys(input).sort()).toEqual(
              ["cwd", "executionBinding", "harness", "projectRoot", "prompt", "title"].sort(),
            );
          },
        );
      });

      it("does not send executionBinding when a getter answers undefined on the single validation read, even if it would answer a binding later", async () => {
        const binding = validBinding();
        let executionBindingReads = 0;
        // A time-of-check/time-of-use trap in the other direction: the
        // getter answers `undefined` (looking unbound) on the one read
        // `launchSession` performs, but would answer a malicious binding on
        // any later read. If the implementation ever re-reads
        // `executionBinding` (e.g. by serializing `input` itself), the
        // wire body would gain an executionBinding that was never
        // validated. It must not: the body must omit executionBinding
        // entirely, and the getter must be invoked exactly once.
        const input = {
          projectRoot: "/repo",
          cwd: "/repo",
          harness: "codex",
          prompt: "Fix tests",
          title: "Fix tests",
        };
        Object.defineProperty(input, "executionBinding", {
          enumerable: true,
          configurable: true,
          get() {
            executionBindingReads += 1;
            return executionBindingReads === 1 ? undefined : (binding as unknown);
          },
        });
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
                  execution_binding: null,
                }),
              );
            });
          },
          async (socketPath) => {
            await createCovenClient(socketPath).launchSession(
              input as unknown as Parameters<
                ReturnType<typeof createCovenClient>["launchSession"]
              >[0],
            );
            const sentBody = JSON.parse(capturedBody);
            expect(sentBody).not.toHaveProperty("executionBinding");
            expect(sentBody).toEqual({
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
            });
            expect(executionBindingReads).toBe(1);
          },
        );
      });

      it("preserves the other launch fields exactly while replacing executionBinding", async () => {
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
            await createCovenClient(socketPath).launchSession({
              projectRoot: "/repo",
              cwd: "/other-cwd",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              model: "openai/gpt-5.6-sol",
              launchMode: "nonInteractive",
              launchPolicy: {
                approval: "never",
                sandbox: "workspace-write",
                addDirs: ["/extra-dir"],
              },
              conversation: { mode: "resume", id: "conversation-1" },
              conversationId: "native-conversation-1",
              familiarId: "sage",
              callerFamiliarId: "caller-familiar",
              executionBinding: binding,
            });
            expect(JSON.parse(capturedBody)).toEqual({
              projectRoot: "/repo",
              cwd: "/other-cwd",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              model: "openai/gpt-5.6-sol",
              launchMode: "nonInteractive",
              launchPolicy: {
                approval: "never",
                sandbox: "workspace-write",
                addDirs: ["/extra-dir"],
              },
              conversation: { mode: "resume", id: "conversation-1" },
              conversationId: "native-conversation-1",
              familiarId: "sage",
              callerFamiliarId: "caller-familiar",
              executionBinding: binding,
            });
          },
        );
      });

      it("legacy launchSession body remains exactly the five required fields when no optional launch fields are supplied", async () => {
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
                  execution_binding: null,
                }),
              );
            });
          },
          async (socketPath) => {
            await createCovenClient(socketPath).launchSession({
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
            });
            const sentBody = JSON.parse(capturedBody);
            expect(sentBody).toEqual({
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
            });
            expect(sentBody).not.toHaveProperty("model");
            expect(sentBody).not.toHaveProperty("launchMode");
            expect(sentBody).not.toHaveProperty("launchPolicy");
            expect(sentBody).not.toHaveProperty("conversation");
            expect(sentBody).not.toHaveProperty("conversationId");
            expect(sentBody).not.toHaveProperty("familiarId");
            expect(sentBody).not.toHaveProperty("callerFamiliarId");
            expect(sentBody).not.toHaveProperty("executionBinding");
          },
        );
      });

      it("does not serialize the original input object (top-level toJSON/prototype trick) on launchSession", async () => {
        let capturedBody = "";
        const input: LaunchCovenSessionInput = {
          projectRoot: "/repo",
          cwd: "/repo",
          harness: "codex",
          prompt: "Fix tests",
          title: "Fix tests",
          model: "openai/gpt-5.6-sol",
          conversationId: "native-conversation-1",
        };
        // A non-enumerable `toJSON` on `input` itself is invisible to any
        // membership/shape check but is still what `JSON.stringify` would
        // call if the wire body were ever `input` (or a shallow copy that
        // preserves accessors/`toJSON`) instead of a fresh object literal.
        Object.defineProperty(input, "toJSON", {
          enumerable: false,
          configurable: true,
          value: () => ({
            projectRoot: "/evil",
            cwd: "/evil",
            harness: "evil-harness",
            prompt: "evil prompt",
            title: "evil title",
            model: "evil-model",
            conversationId: "evil-conversation",
          }),
        });
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
                  execution_binding: null,
                }),
              );
            });
          },
          async (socketPath) => {
            await createCovenClient(socketPath).launchSession(input);
            expect(JSON.parse(capturedBody)).toEqual({
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              model: "openai/gpt-5.6-sol",
              conversationId: "native-conversation-1",
            });
          },
        );
      });

      it("reads an optional launch field exactly once even when it is a getter, on launchSession", async () => {
        let modelReads = 0;
        const input = {
          projectRoot: "/repo",
          cwd: "/repo",
          harness: "codex",
          prompt: "Fix tests",
          title: "Fix tests",
        };
        // Same time-of-check/time-of-use trap as the executionBinding getter
        // regression above, but for a plain (non-executionBinding) optional
        // field: proves every field is read exactly once into the fresh
        // body object, not re-read later from `input`.
        Object.defineProperty(input, "model", {
          enumerable: true,
          configurable: true,
          get() {
            modelReads += 1;
            return modelReads === 1 ? "openai/gpt-5.6-sol" : "tampered-reread";
          },
        });
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
                  execution_binding: null,
                }),
              );
            });
          },
          async (socketPath) => {
            await createCovenClient(socketPath).launchSession(
              input as unknown as Parameters<
                ReturnType<typeof createCovenClient>["launchSession"]
              >[0],
            );
            const sentBody = JSON.parse(capturedBody);
            expect(sentBody.model).toBe("openai/gpt-5.6-sol");
            expect(modelReads).toBe(1);
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

  describe("request adoption", () => {
    it("requires familiarId for adopted launches without changing legacy launch input", () => {
      type AdoptedLaunchInput = Parameters<
        ReturnType<typeof createCovenClient>["launchAdoptedSession"]
      >[0];

      expectTypeOf<AdoptedLaunchInput["familiarId"]>().toEqualTypeOf<string>();
      expectTypeOf<LaunchCovenSessionInput["familiarId"]>().toEqualTypeOf<
        string | undefined
      >();
    });

    describe("normalizeRequestAdoption validator", () => {
      it("returns a fresh exact plain snapshot for a valid request adoption", () => {
        const adoption = validAdoption();
        const normalized = __testing.normalizeRequestAdoption(adoption);

        expect(normalized).toEqual(adoption);
        expect(normalized).not.toBe(adoption);
        expect(Object.getPrototypeOf(normalized)).toBe(Object.prototype);
        expect(Reflect.ownKeys(normalized).sort()).toEqual(
          ["contract", "key", "requestDigest"].sort(),
        );
      });

      it.each([
        ["null", null],
        ["array", []],
        ["date", new Date(0)],
        [
          "custom prototype",
          Object.assign(Object.create({ inherited: "nope" }), validAdoption()),
        ],
      ])("rejects a non-plain %s root", (_name, value) => {
        expect(() => __testing.normalizeRequestAdoption(value)).toThrow(
          /requestAdoption must be a plain object/,
        );
      });

      it("accepts a null-prototype object but returns an ordinary plain snapshot", () => {
        const adoption = Object.assign(Object.create(null), validAdoption());
        const normalized = __testing.normalizeRequestAdoption(adoption);
        expect(normalized).toEqual(validAdoption());
        expect(Object.getPrototypeOf(normalized)).toBe(Object.prototype);
      });

      it("rejects missing, extra, symbol, and non-enumerable members", () => {
        const { key: _drop, ...missing } = validAdoption();
        const extra = { ...validAdoption(), extra: true };
        const symbol = { ...validAdoption() };
        Object.defineProperty(symbol, Symbol("extra"), {
          enumerable: true,
          value: true,
        });
        const hidden = { ...validAdoption() };
        Object.defineProperty(hidden, "hidden", {
          enumerable: false,
          value: true,
        });

        for (const value of [missing, extra, symbol, hidden]) {
          expect(() => __testing.normalizeRequestAdoption(value)).toThrow(
            /requestAdoption has missing or unknown fields/,
          );
        }
      });

      it("rejects accessors without invoking them", () => {
        let reads = 0;
        const adoption = { ...validAdoption() };
        Object.defineProperty(adoption, "key", {
          enumerable: true,
          configurable: true,
          get() {
            reads += 1;
            return "getter-must-not-run";
          },
        });

        expect(() => __testing.normalizeRequestAdoption(adoption)).toThrow(
          /requestAdoption must contain only own enumerable data properties/,
        );
        expect(reads).toBe(0);
      });

      it("rejects the wrong contract without echoing caller data", () => {
        const callerContractMarker = "caller-contract-marker";
        expect(() =>
          __testing.normalizeRequestAdoption({
            ...validAdoption(),
            contract: callerContractMarker,
          }),
        ).toThrow(/requestAdoption\.contract is unsupported/);
        try {
          __testing.normalizeRequestAdoption({
            ...validAdoption(),
            contract: callerContractMarker,
          });
        } catch (error) {
          expect(String(error)).not.toContain(callerContractMarker);
        }
      });

      it.each([
        ["empty", ""],
        ["over 255 bytes", "a".repeat(256)],
        ["space", "key with space"],
        ["non-ASCII", "kéy"],
        ["unsupported punctuation", "key?query"],
      ])("rejects an invalid %s key without normalization", (_name, key) => {
        expect(() =>
          __testing.normalizeRequestAdoption({ ...validAdoption(), key }),
        ).toThrow(/requestAdoption\.key is invalid/);
      });

      it.each(["a", "a".repeat(255), "Az09._:/-"])(
        "accepts the exact ASCII key boundary/value %s",
        (key) => {
          expect(
            __testing.normalizeRequestAdoption({ ...validAdoption(), key }).key,
          ).toBe(key);
        },
      );

      it.each([
        `sha256:${"A".repeat(64)}`,
        `sha256:${"a".repeat(63)}`,
        `sha256:${"a".repeat(65)}`,
        "a".repeat(64),
        `sha512:${"a".repeat(64)}`,
      ])("rejects a non-canonical request digest", (requestDigest) => {
        expect(() =>
          __testing.normalizeRequestAdoption({
            ...validAdoption(),
            requestDigest,
          }),
        ).toThrow(/requestAdoption\.requestDigest is invalid/);
      });
    });

    it.each([201, 200])(
      "launchAdoptedSession posts the exact dedicated body and normalizes a %s session",
      async (status) => {
        const binding = validBinding();
        const adoption = validAdoption();
        const order: string[] = [];
        let healthCompleted = false;
        let capturedBody: unknown;
        await withServer(
          (req, res) => {
            if (req.method === "GET") {
              order.push(`GET ${req.url}`);
              res.setHeader("Content-Type", "application/json");
              setImmediate(() => {
                healthCompleted = true;
                res.end(JSON.stringify(o3Health()));
              });
              return;
            }
            expect(healthCompleted).toBe(true);
            order.push(`POST ${req.url}`);
            let body = "";
            req.on("data", (chunk: string) => {
              body += chunk;
            });
            req.on("end", () => {
              capturedBody = JSON.parse(body);
              res.statusCode = status;
              res.setHeader("Content-Type", "application/json");
              res.end(JSON.stringify(sessionWire(binding)));
            });
          },
          async (socketPath) => {
            const session = await createCovenClient(socketPath).launchAdoptedSession({
              projectRoot: "/repo",
              cwd: "/repo/subdir",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              model: "openai/gpt-5.6-sol",
              launchMode: "nonInteractive",
              launchPolicy: {
                approval: "never",
                sandbox: "workspace-write",
                addDirs: ["/extra"],
              },
              conversation: { mode: "resume", id: "conversation-1" },
              conversationId: "native-conversation-1",
              familiarId: "sage",
              executionBinding: binding,
              requestAdoption: adoption,
            });
            expect(session).toEqual({
              id: "session-1",
              projectRoot: "/repo",
              harness: "codex",
              title: "Fix tests",
              status: "running",
              exitCode: null,
              createdAt: "2026-04-27T10:00:00Z",
              updatedAt: "2026-04-27T10:00:01Z",
              executionBinding: binding,
            });
          },
        );

        expect(order).toEqual([
          "GET /api/v1/health",
          "POST /api/v1/adopted-sessions",
        ]);
        expect(capturedBody).toEqual({
          projectRoot: "/repo",
          cwd: "/repo/subdir",
          harness: "codex",
          prompt: "Fix tests",
          title: "Fix tests",
          model: "openai/gpt-5.6-sol",
          launchMode: "nonInteractive",
          launchPolicy: {
            approval: "never",
            sandbox: "workspace-write",
            addDirs: ["/extra"],
          },
          conversation: { mode: "resume", id: "conversation-1" },
          conversationId: "native-conversation-1",
          familiarId: "sage",
          executionBinding: binding,
          requestAdoption: adoption,
        });
      },
    );

    it("launchAdoptedSession sends a daemon-valid child binding and matching caller", async () => {
      const binding = delegatedBinding();
      const adoption = validAdoption();
      let capturedBody: unknown;
      await withServer(
        (req, res) => {
          if (req.method === "GET") {
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(o3Health()));
            return;
          }
          let body = "";
          req.on("data", (chunk: string) => {
            body += chunk;
          });
          req.on("end", () => {
            capturedBody = JSON.parse(body);
            res.statusCode = 201;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(sessionWire(binding)));
          });
        },
        async (socketPath) => {
          await createCovenClient(socketPath).launchAdoptedSession({
            projectRoot: "/repo",
            cwd: "/repo",
            harness: "codex",
            prompt: "Delegate tests",
            title: "Delegate tests",
            familiarId: "sage",
            callerFamiliarId: "cody",
            executionBinding: binding,
            requestAdoption: adoption,
          });
        },
      );

      expect(capturedBody).toEqual({
        projectRoot: "/repo",
        cwd: "/repo",
        harness: "codex",
        prompt: "Delegate tests",
        title: "Delegate tests",
        familiarId: "sage",
        callerFamiliarId: "cody",
        executionBinding: binding,
        requestAdoption: adoption,
      });
    });

    it.each([202, 204])(
      "launchAdoptedSession rejects protocol-invalid HTTP %s without identity data",
      async (status) => {
        const binding = validBinding();
        const adoption = validAdoption();
        let error: unknown;
        await withServer(
          (req, res) => {
            if (req.method === "GET") {
              res.setHeader("Content-Type", "application/json");
              res.end(JSON.stringify(o3Health()));
              return;
            }
            res.statusCode = status;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(sessionWire(binding)));
          },
          async (socketPath) => {
            try {
              await createCovenClient(socketPath).launchAdoptedSession({
                projectRoot: "/repo",
                cwd: "/repo",
                harness: "codex",
                prompt: "private-launch-prompt",
                title: "Private launch",
                familiarId: "sage",
                executionBinding: binding,
                requestAdoption: adoption,
              });
            } catch (caught) {
              error = caught;
            }
          },
        );

        expect(error).toBeInstanceOf(Error);
        expect(error).not.toBeInstanceOf(CovenApiError);
        expect((error as Error).message).toBe(
          "Coven adopted launch response status is invalid",
        );
        expect(String(error)).not.toContain(adoption.key);
        expect(String(error)).not.toContain(binding.principalRef);
        expect(String(error)).not.toContain("private-launch-prompt");
      },
    );

    it.each([
      [202, false],
      [200, true],
    ])(
      "sendAdoptedInput normalizes the exact %s adopted result",
      async (status, replayed) => {
        const binding = validBinding();
        const adoption = validAdoption({ requestDigest: DIGEST_D });
        let capturedPath = "";
        let capturedBody: unknown;
        await withServer(
          (req, res) => {
            if (req.method === "GET") {
              res.setHeader("Content-Type", "application/json");
              res.end(JSON.stringify(o3Health()));
              return;
            }
            capturedPath = req.url ?? "";
            let body = "";
            req.on("data", (chunk: string) => {
              body += chunk;
            });
            req.on("end", () => {
              capturedBody = JSON.parse(body);
              res.statusCode = status;
              res.setHeader("Content-Type", "application/json");
              res.end(
                JSON.stringify({
                  adopted: true,
                  replayed,
                  delivery: "not_asserted",
                }),
              );
            });
          },
          async (socketPath) => {
            await expect(
              createCovenClient(socketPath).sendAdoptedInput(
                "session /1",
                "hello\n",
                binding,
                adoption,
              ),
            ).resolves.toEqual({
              adopted: true,
              replayed,
              delivery: "not_asserted",
            });
          },
        );
        expect(capturedPath).toBe(
          "/api/v1/sessions/session%20%2F1/adopted-input",
        );
        expect(capturedBody).toEqual({
          data: "hello\n",
          executionBinding: binding,
          requestAdoption: adoption,
        });
      },
    );

    it.each([
      ["HTTP 200 with a first-adoption body", 200, false],
      ["HTTP 202 with a replay body", 202, true],
      ["HTTP 201 with a first-adoption body", 201, false],
      ["HTTP 206 with a replay body", 206, true],
      ["HTTP 204 with no usable body", 204, false],
    ])("sendAdoptedInput rejects %s", async (_name, status, replayed) => {
      const sessionId = "private-session-id";
      const data = "private-input-data";
      const binding = validBinding();
      const adoption = validAdoption({ requestDigest: DIGEST_D });
      let error: unknown;
      await withServer(
        (req, res) => {
          if (req.method === "GET") {
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(o3Health()));
            return;
          }
          res.statusCode = status;
          res.setHeader("Content-Type", "application/json");
          res.end(
            JSON.stringify({
              adopted: true,
              replayed,
              delivery: "not_asserted",
            }),
          );
        },
        async (socketPath) => {
          try {
            await createCovenClient(socketPath).sendAdoptedInput(
              sessionId,
              data,
              binding,
              adoption,
            );
          } catch (caught) {
            error = caught;
          }
        },
      );

      expect(error).toBeInstanceOf(Error);
      expect(error).not.toBeInstanceOf(CovenApiError);
      expect((error as Error).message).toBe("Coven adoption result is invalid");
      const errorText = String(error);
      expect(errorText).not.toContain(sessionId);
      expect(errorText).not.toContain(data);
      expect(errorText).not.toContain(adoption.key);
      expect(errorText).not.toContain(binding.principalRef);
    });

    it.each([
      ["adopted literal", { adopted: false, replayed: false, delivery: "not_asserted" }],
      ["replayed type", { adopted: true, replayed: "false", delivery: "not_asserted" }],
      ["delivery literal", { adopted: true, replayed: false, delivery: "asserted" }],
      [
        "unknown member",
        { adopted: true, replayed: false, delivery: "not_asserted", extra: true },
      ],
      ["missing member", { adopted: true, replayed: false }],
    ])("rejects an adopted result with an invalid %s", (_name, responseBody) => {
      expect(() => __testing.normalizeAdoptionResult(responseBody)).toThrow(
        /Coven adoption result is invalid/,
      );
    });

    it("rejects a symbol member in an adopted result with the static shape error", () => {
      const response = {
        adopted: true,
        replayed: false,
        delivery: "not_asserted",
      };
      Object.defineProperty(response, Symbol("extra"), {
        enumerable: true,
        value: true,
      });
      expect(() => __testing.normalizeAdoptionResult(response)).toThrow(
        /Coven adoption result is invalid/,
      );
    });

    const ADOPTED_METHODS = ["launchAdoptedSession", "sendAdoptedInput"] as const;

    // Invokes the given adopted method with valid, normalized O2/O3 fixtures
    // so any rejection in the fail-closed matrix below stems from health/
    // capability negotiation, never from local input validation.
    async function invokeAdoptedMethod(
      client: ReturnType<typeof createCovenClient>,
      method: (typeof ADOPTED_METHODS)[number],
      sessionId: string,
      data: string,
      binding: CovenExecutionBinding,
      adoption: TestRequestAdoption,
    ): Promise<unknown> {
      if (method === "launchAdoptedSession") {
        return client.launchAdoptedSession({
          projectRoot: "/repo",
          cwd: "/repo",
          harness: "codex",
          prompt: "Fix tests",
          title: "Fix tests",
          familiarId: "sage",
          executionBinding: binding,
          requestAdoption: adoption,
        });
      }
      return client.sendAdoptedInput(sessionId, data, binding, adoption);
    }

    describe.each(ADOPTED_METHODS)("fail-closed negotiation for %s", (method) => {
      it.each([
        [
          "health transport/API failure",
          503,
          {
            error: {
              code: "unavailable",
              message: "offline",
            },
          },
          "Coven API returned HTTP 503",
        ],
        [
          "absent capability",
          200,
          { ...o3Health(), capabilities: {} },
          "Coven daemon does not support request adoption",
        ],
        [
          "null capability value",
          200,
          o3Health(null),
          "Coven daemon does not support request adoption",
        ],
        [
          "non-array capability (bare string)",
          200,
          o3Health(REQUEST_ADOPTION_CONTRACT),
          "Coven daemon does not support request adoption",
        ],
        [
          "non-array capability (object)",
          200,
          o3Health({ contract: REQUEST_ADOPTION_CONTRACT }),
          "Coven daemon does not support request adoption",
        ],
        [
          "unsupported array (older version)",
          200,
          o3Health(["psyche.request_adoption.v0"]),
          "Coven daemon does not support request adoption",
        ],
        [
          "unsupported array (case mismatch)",
          200,
          o3Health(["PSYCHE.REQUEST_ADOPTION.V1"]),
          "Coven daemon does not support request adoption",
        ],
        [
          "malformed array (non-string entries)",
          200,
          o3Health([{ contract: REQUEST_ADOPTION_CONTRACT }, 7]),
          "Coven daemon does not support request adoption",
        ],
      ])("sends zero POST requests for %s", async (_name, status, healthBody, message) => {
        const sessionId = "session-fail-closed";
        const data = "leak-guard-input\n";
        const binding = validBinding();
        const adoption = validAdoption({
          key: "psyche:graph-1/node-1/attempt-1/leak-guard",
          requestDigest: DIGEST_D,
        });
        const requests: string[] = [];
        let error: unknown;
        await withServer(
          (req, res) => {
            requests.push(`${req.method} ${req.url}`);
            res.statusCode = status;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(healthBody));
          },
          async (socketPath) => {
            const client = createCovenClient(socketPath);
            try {
              await invokeAdoptedMethod(client, method, sessionId, data, binding, adoption);
            } catch (caught) {
              error = caught;
            }
          },
        );
        expect(error).toBeInstanceOf(Error);
        // The thrown message must be the static, local negotiation-failure
        // string (or a status-only transport error) — never an echo of the
        // caller-supplied key, digest, session id, or input data.
        expect((error as Error).message).toBe(message);
        const errorText = String(error);
        expect(errorText).not.toContain(adoption.key);
        expect(errorText).not.toContain(adoption.requestDigest);
        expect(errorText).not.toContain(binding.principalRef);
        expect(errorText).not.toContain(sessionId);
        expect(errorText).not.toContain(data);
        expect(requests.filter((entry) => entry.startsWith("POST"))).toEqual([]);
        expect(requests[0]).toBe("GET /api/v1/health");
      });

      // The daemon must advertise the exact `coven.daemon.v1` contract
      // before either adopted method is authorized to negotiate the
      // request-adoption capability at all. Every case here has a fully
      // valid `requestAdoptionContracts` advertisement, so a failure can
      // only stem from the apiVersion gate.
      it.each([
        ["missing", undefined, false],
        ["null", null, true],
        ["non-string (number)", 7, true],
        ["non-string (object)", { name: "coven.daemon.v1" }, true],
        ["non-string (array)", ["coven.daemon.v1"], true],
        ["wrong case", "Coven.Daemon.V1", true],
        ["near-match (trailing space)", "coven.daemon.v1 ", true],
        ["near-match (leading space)", " coven.daemon.v1", true],
        ["near-match (extra suffix)", "coven.daemon.v1beta", true],
        ["near-match (version bump)", "coven.daemon.v2", true],
        ["unsupported legacy literal", "v1", true],
        ["empty string", "", true],
      ])(
        "sends zero POST requests for %s apiVersion",
        async (_name, apiVersion, includeProperty) => {
          const sessionId = "session-version-fail-closed";
          const data = "version-leak-guard-input\n";
          const binding = validBinding();
          const adoption = validAdoption({
            key: "psyche:graph-1/node-1/attempt-1/version-leak-guard",
            requestDigest: DIGEST_D,
          });
          const healthBody: Record<string, unknown> = { ...o3Health() };
          if (includeProperty) {
            healthBody.apiVersion = apiVersion;
          } else {
            delete healthBody.apiVersion;
          }
          const requests: string[] = [];
          let error: unknown;
          await withServer(
            (req, res) => {
              requests.push(`${req.method} ${req.url}`);
              res.setHeader("Content-Type", "application/json");
              res.end(JSON.stringify(healthBody));
            },
            async (socketPath) => {
              const client = createCovenClient(socketPath);
              try {
                await invokeAdoptedMethod(client, method, sessionId, data, binding, adoption);
              } catch (caught) {
                error = caught;
              }
            },
          );
          expect(error).toBeInstanceOf(Error);
          // Static, local failure — never the capability-negotiation
          // message, and never an echo of the received version value or
          // any caller-supplied secret.
          expect((error as Error).message).toBe(
            "Coven daemon API version is not supported",
          );
          const errorText = String(error);
          expect(errorText).not.toContain(adoption.key);
          expect(errorText).not.toContain(adoption.requestDigest);
          expect(errorText).not.toContain(binding.principalRef);
          expect(errorText).not.toContain(sessionId);
          expect(errorText).not.toContain(data);
          if (typeof apiVersion === "string" && apiVersion.length > 0) {
            expect(errorText).not.toContain(apiVersion);
          }
          expect(requests.filter((entry) => entry.startsWith("POST"))).toEqual([]);
          expect(requests).toEqual(["GET /api/v1/health"]);
        },
      );

      it("fails on apiVersion before capability when both are invalid", async () => {
        const binding = validBinding();
        const adoption = validAdoption();
        const healthBody = { ...o3Health(null), apiVersion: "coven.daemon.v2" };
        const requests: string[] = [];
        let error: unknown;
        await withServer(
          (req, res) => {
            requests.push(`${req.method} ${req.url}`);
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(healthBody));
          },
          async (socketPath) => {
            try {
              await invokeAdoptedMethod(
                createCovenClient(socketPath),
                method,
                "session-version-precedence",
                "version-precedence-input\n",
                binding,
                adoption,
              );
            } catch (caught) {
              error = caught;
            }
          },
        );
        expect(error).toBeInstanceOf(Error);
        expect((error as Error).message).toBe(
          "Coven daemon API version is not supported",
        );
        expect(requests).toEqual(["GET /api/v1/health"]);
      });

      it("still fails on capability when apiVersion is valid and capability is invalid", async () => {
        const binding = validBinding();
        const adoption = validAdoption();
        const healthBody = o3Health(null);
        const requests: string[] = [];
        let error: unknown;
        await withServer(
          (req, res) => {
            requests.push(`${req.method} ${req.url}`);
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(healthBody));
          },
          async (socketPath) => {
            try {
              await invokeAdoptedMethod(
                createCovenClient(socketPath),
                method,
                "session-capability-precedence",
                "capability-precedence-input\n",
                binding,
                adoption,
              );
            } catch (caught) {
              error = caught;
            }
          },
        );
        expect(error).toBeInstanceOf(Error);
        expect((error as Error).message).toBe(
          "Coven daemon does not support request adoption",
        );
        expect(requests).toEqual(["GET /api/v1/health"]);
      });

      it.each([
        ["null", null],
        ["object", { contract: REQUEST_ADOPTION_CONTRACT }],
        ["string", REQUEST_ADOPTION_CONTRACT],
      ])(
        "does not fall back from a present invalid canonical %s capability",
        async (_name, canonicalValue) => {
          const sessionId = "session-canonical-precedence";
          const data = "canonical-precedence-input\n";
          const binding = validBinding();
          const adoption = validAdoption();
          const healthBody = o3Health(canonicalValue);
          (healthBody.capabilities as Record<string, unknown>).request_adoption_contracts = [
            REQUEST_ADOPTION_CONTRACT,
          ];
          const requests: string[] = [];
          let error: unknown;
          await withServer(
            (req, res) => {
              requests.push(`${req.method} ${req.url}`);
              res.setHeader("Content-Type", "application/json");
              res.end(JSON.stringify(healthBody));
            },
            async (socketPath) => {
              try {
                await invokeAdoptedMethod(
                  createCovenClient(socketPath),
                  method,
                  sessionId,
                  data,
                  binding,
                  adoption,
                );
              } catch (caught) {
                error = caught;
              }
            },
          );

          expect(error).toBeInstanceOf(Error);
          expect((error as Error).message).toBe(
            "Coven daemon does not support request adoption",
          );
          expect(requests).toEqual(["GET /api/v1/health"]);
        },
      );

      it("uses supported snake_case capability only when the canonical property is absent", async () => {
        const binding = validBinding();
        const adoption = validAdoption();
        const healthBody = o3Health();
        const capabilities = healthBody.capabilities as Record<string, unknown>;
        delete capabilities.requestAdoptionContracts;
        capabilities.request_adoption_contracts = [REQUEST_ADOPTION_CONTRACT];
        const requests: string[] = [];
        await withServer(
          (req, res) => {
            requests.push(`${req.method} ${req.url}`);
            res.setHeader("Content-Type", "application/json");
            if (req.method === "GET") {
              res.end(JSON.stringify(healthBody));
              return;
            }
            if (method === "launchAdoptedSession") {
              res.statusCode = 201;
              res.end(JSON.stringify(sessionWire(binding)));
              return;
            }
            res.statusCode = 202;
            res.end(
              JSON.stringify({
                adopted: true,
                replayed: false,
                delivery: "not_asserted",
              }),
            );
          },
          async (socketPath) => {
            await invokeAdoptedMethod(
              createCovenClient(socketPath),
              method,
              "session-snake-fallback",
              "snake-fallback-input\n",
              binding,
              adoption,
            );
          },
        );

        expect(requests).toHaveLength(2);
        expect(requests[0]).toBe("GET /api/v1/health");
        expect(requests[1]).toMatch(/^POST /);
      });

      it("sends zero POST requests when the health transport connection fails", async () => {
        const sessionId = "session-transport-failure";
        const data = "leak-guard-transport\n";
        const binding = validBinding();
        const adoption = validAdoption({
          key: "psyche:graph-1/node-1/attempt-1/transport",
          requestDigest: DIGEST_D,
        });
        const requests: string[] = [];
        let error: unknown;
        await withServer(
          (req) => {
            requests.push(`${req.method} ${req.url}`);
            // Simulate a genuine transport-level failure (connection reset)
            // rather than a well-formed HTTP error response.
            req.socket.destroy();
          },
          async (socketPath) => {
            const client = createCovenClient(socketPath);
            try {
              await invokeAdoptedMethod(client, method, sessionId, data, binding, adoption);
            } catch (caught) {
              error = caught;
            }
          },
        );
        expect(error).toBeInstanceOf(Error);
        const errorText = String(error);
        expect(errorText).not.toContain(adoption.key);
        expect(errorText).not.toContain(adoption.requestDigest);
        expect(errorText).not.toContain(binding.principalRef);
        expect(errorText).not.toContain(sessionId);
        expect(errorText).not.toContain(data);
        expect(requests.filter((entry) => entry.startsWith("POST"))).toEqual([]);
        expect(requests[0]).toBe("GET /api/v1/health");
      });
    });

    it.each([
      ["missing", undefined, false],
      ["undefined", undefined, true],
      ["null", null, true],
      ["non-string", 7, true],
      ["empty", "", true],
      ["whitespace-padded", " sage", true],
      ["invalid opaque syntax", "sage?", true],
    ])(
      "rejects a %s adopted familiarId before health negotiation",
      async (_name, familiarId, includeProperty) => {
        let requestCount = 0;
        const input: Record<string, unknown> = {
          projectRoot: "/repo",
          cwd: "/repo",
          harness: "codex",
          prompt: "Fix tests",
          title: "Fix tests",
          executionBinding: validBinding(),
          requestAdoption: validAdoption(),
        };
        if (includeProperty) {
          input.familiarId = familiarId;
        }

        await withServer(
          (_req, res) => {
            requestCount += 1;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(o3Health()));
          },
          async (socketPath) => {
            await expect(
              createCovenClient(socketPath).launchAdoptedSession(
                input as Parameters<
                  ReturnType<typeof createCovenClient>["launchAdoptedSession"]
                >[0],
              ),
            ).rejects.toThrow("familiarId is invalid");
          },
        );
        expect(requestCount).toBe(0);
      },
    );

    it("validates adopted caller metadata before sending health", async () => {
      let requestCount = 0;
      await withServer(
        (_req, res) => {
          requestCount += 1;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify(o3Health()));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).launchAdoptedSession({
              projectRoot: "/repo",
              cwd: "/repo",
              harness: "codex",
              prompt: "Fix tests",
              title: "Fix tests",
              familiarId: "sage",
              executionBinding: validBinding(),
              requestAdoption: {
                ...validAdoption(),
                key: "invalid key",
              },
            }),
          ).rejects.toThrow(/requestAdoption\.key is invalid/);
        },
      );
      expect(requestCount).toBe(0);
    });

    it("reads launch fields once and snapshots nested caller state before health completes", async () => {
      const binding = validBinding();
      const adoption = validAdoption();
      const addDirs = ["/original"];
      const launchPolicy = {
        approval: "never" as const,
        sandbox: "workspace-write" as const,
        addDirs,
      };
      const conversation = { mode: "resume" as const, id: "conversation-1" };
      let requestAdoptionReads = 0;
      let promptReads = 0;
      let capturedBody: Record<string, unknown> | undefined;
      const input = {
        projectRoot: "/repo",
        cwd: "/repo",
        harness: "codex",
        title: "Fix tests",
        familiarId: "sage",
        launchPolicy,
        conversation,
        executionBinding: binding,
      };
      Object.defineProperty(input, "prompt", {
        enumerable: true,
        get() {
          promptReads += 1;
          return promptReads === 1 ? "original prompt" : "tampered prompt";
        },
      });
      Object.defineProperty(input, "requestAdoption", {
        enumerable: true,
        get() {
          requestAdoptionReads += 1;
          return requestAdoptionReads === 1
            ? adoption
            : validAdoption({ key: "tampered-reread" });
        },
      });

      await withServer(
        (req, res) => {
          if (req.method === "GET") {
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(o3Health()));
            return;
          }
          let body = "";
          req.on("data", (chunk: string) => {
            body += chunk;
          });
          req.on("end", () => {
            capturedBody = JSON.parse(body) as Record<string, unknown>;
            res.statusCode = 201;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(sessionWire()));
          });
        },
        async (socketPath) => {
          const pending = createCovenClient(socketPath).launchAdoptedSession(
            input as Parameters<
              ReturnType<typeof createCovenClient>["launchAdoptedSession"]
            >[0],
          );
          binding.principalRef = "principal:mutated";
          adoption.key = "mutated-key";
          addDirs[0] = "/mutated";
          conversation.id = "mutated-conversation";
          await pending;
        },
      );

      expect(promptReads).toBe(1);
      expect(requestAdoptionReads).toBe(1);
      expect(capturedBody?.prompt).toBe("original prompt");
      expect(
        (capturedBody?.executionBinding as CovenExecutionBinding).principalRef,
      ).toBe("principal:operator");
      expect(
        (capturedBody?.requestAdoption as TestRequestAdoption).key,
      ).toBe("psyche:graph-1/node-1/attempt-1/request-1");
      expect(capturedBody?.launchPolicy).toEqual({
        approval: "never",
        sandbox: "workspace-write",
        addDirs: ["/original"],
      });
      expect(capturedBody?.conversation).toEqual({
        mode: "resume",
        id: "conversation-1",
      });
    });

    it("never falls back to a legacy mutation after a dedicated POST error", async () => {
      const requests: string[] = [];
      await withServer(
        (req, res) => {
          requests.push(`${req.method} ${req.url}`);
          if (req.method === "GET") {
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify(o3Health()));
            return;
          }
          res.statusCode = 404;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: { code: "invalid_request" } }));
        },
        async (socketPath) => {
          await expect(
            createCovenClient(socketPath).sendAdoptedInput(
              "session-1",
              "hello",
              validBinding(),
              validAdoption({ requestDigest: DIGEST_D }),
            ),
          ).rejects.toBeInstanceOf(CovenApiError);
        },
      );
      expect(requests).toEqual([
        "GET /api/v1/health",
        "POST /api/v1/sessions/session-1/adopted-input",
      ]);
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
