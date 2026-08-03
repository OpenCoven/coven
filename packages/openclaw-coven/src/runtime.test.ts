import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import {
  AcpRuntimeError,
  registerAcpRuntimeBackend,
  unregisterAcpRuntimeBackend,
  type AcpRuntime,
  type AcpRuntimeEvent,
  type AcpRuntimeHandle,
} from "openclaw/plugin-sdk/acp-runtime";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CovenClient,
  CovenEventRecord,
  CovenHealthResponse,
  CovenSessionRecord,
} from "./client.js";
import type { ResolvedCovenPluginConfig } from "./config.js";
import { __testing, CovenAcpRuntime } from "./runtime.js";

const baseConfig: ResolvedCovenPluginConfig = {
  covenHome: "",
  socketPath: "",
  workspaceDir: "",
  allowFallback: false,
  fallbackBackend: "acpx",
  pollIntervalMs: 25,
  harnesses: {},
};

let workspaceDir: string;
let config: ResolvedCovenPluginConfig;

beforeEach(async () => {
  workspaceDir = await fs.realpath(
    await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-coven-workspace-")),
  );
  const covenHome = path.join(workspaceDir, ".coven");
  await fs.mkdir(covenHome);
  config = {
    ...baseConfig,
    covenHome,
    socketPath: path.join(covenHome, "coven.sock"),
    workspaceDir,
  };
});

function session(overrides: Partial<CovenSessionRecord> = {}): CovenSessionRecord {
  return {
    id: "session-1",
    projectRoot: workspaceDir,
    harness: "codex",
    title: "Fix tests",
    status: "running",
    exitCode: null,
    createdAt: "2026-04-27T10:00:00Z",
    updatedAt: "2026-04-27T10:00:00Z",
    ...overrides,
  };
}

function event(overrides: Partial<CovenEventRecord>): CovenEventRecord {
  return {
    seq: 1,
    id: "event-1",
    sessionId: "session-1",
    kind: "output",
    payloadJson: JSON.stringify({ data: "hello\n" }),
    createdAt: "2026-04-27T10:00:00Z",
    ...overrides,
  };
}

function compatibleHealth(
  overrides: Partial<CovenHealthResponse> = {},
): CovenHealthResponse {
  return {
    apiVersion: "coven.daemon.v1",
    covenVersion: "0.0.0",
    capabilities: {
      sessions: true,
      events: true,
      eventCursor: "sequence",
      structuredErrors: true,
    },
    ok: true,
    daemon: null,
    ...overrides,
  };
}

function fakeClient(overrides: Partial<CovenClient> = {}): CovenClient {
  return {
    health: vi.fn(async () => compatibleHealth()),
    launchSession: vi.fn(async () => session()),
    getSession: vi.fn(async () => session({ status: "completed", exitCode: 0 })),
    listEvents: vi.fn(async () => [
      event({ id: "event-1", kind: "output", payloadJson: JSON.stringify({ data: "hello\n" }) }),
      event({
        id: "event-2",
        kind: "exit",
        payloadJson: JSON.stringify({ status: "completed", exitCode: 0 }),
      }),
    ]),
    sendInput: vi.fn(async () => undefined),
    killSession: vi.fn(async () => undefined),
    ...overrides,
  };
}

async function collect(iterable: AsyncIterable<AcpRuntimeEvent>): Promise<AcpRuntimeEvent[]> {
  const events: AcpRuntimeEvent[] = [];
  for await (const item of iterable) {
    events.push(item);
  }
  return events;
}

function fallbackRuntime(): AcpRuntime {
  const handle: AcpRuntimeHandle = {
    sessionKey: "agent:codex:test",
    backend: "acpx",
    runtimeSessionName: "fallback-session",
    cwd: workspaceDir,
  };
  return {
    ensureSession: vi.fn(async () => handle),
    async *runTurn() {
      yield { type: "text_delta", text: "direct fallback\n", stream: "output" };
      yield { type: "done", stopReason: "complete" };
    },
    getStatus: vi.fn(async () => ({ summary: "fallback active" })),
    cancel: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
  };
}

afterEach(() => {
  vi.useRealTimers();
  unregisterAcpRuntimeBackend("acpx");
  return fs.rm(workspaceDir, { recursive: true, force: true });
});

describe("CovenAcpRuntime", () => {
  it("fails closed by default when Coven is unavailable", async () => {
    const transportError = new Error("\u001b[31moffline\r\nnow");
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        health: vi.fn(async () => {
          throw transportError;
        }),
      }),
    });

    const error = await runtime
      .ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: workspaceDir,
      })
      .catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(AcpRuntimeError);
    expect(error).toMatchObject({
      code: "ACP_BACKEND_UNAVAILABLE",
      message: "Coven health check failed: Error: offline now",
      cause: transportError,
    });
  });

  it("falls back to the direct ACP backend when Coven is unavailable and fallback is enabled", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client: fakeClient({
        health: vi.fn(async () => {
          throw new Error("offline");
        }),
      }),
    });

    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    expect(handle.backend).toBe("acpx");
    expect(fallback.ensureSession).toHaveBeenCalledOnce();
  });

  it("logs sanitized compatibility failures before falling back", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const logger = { warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() };
    const launchSession = vi.fn(async () => session());
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client: fakeClient({
        health: vi.fn(async () =>
          compatibleHealth({ apiVersion: "\u001b[31mcoven.daemon.v2\r" }),
        ),
        launchSession,
      }),
      logger,
    });

    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    expect(handle.backend).toBe("acpx");
    expect(logger.warn).toHaveBeenCalledWith(
      "coven compatibility check failed; falling back to acpx: AcpRuntimeError: Coven compatibility check failed: expected apiVersion coven.daemon.v1, got invalid; upgrade Coven to a compatible version",
    );
    expect(launchSession).not.toHaveBeenCalled();
  });
  it("rejects unavailable-Coven fallback sessions whose cwd is outside the workspace", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client: fakeClient({
        health: vi.fn(async () => {
          throw new Error("offline");
        }),
      }),
    });
    const outsideWorkspaceDir = await fs.mkdtemp(
      path.join(os.tmpdir(), "openclaw-coven-outside-workspace-"),
    );

    try {
      await expect(
        runtime.ensureSession({
          sessionKey: "agent:codex:test",
          agent: "codex",
          mode: "oneshot",
          cwd: outsideWorkspaceDir,
        }),
      ).rejects.toThrow(/outside workspace/);
      expect(fallback.ensureSession).not.toHaveBeenCalled();
    } finally {
      await fs.rm(outsideWorkspaceDir, { recursive: true, force: true });
    }
  });
  it("falls back when Coven health checks do not settle before the deadline", async () => {
    vi.useFakeTimers();
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const client = fakeClient({
      health: vi.fn(
        async (signal?: AbortSignal) =>
          await new Promise<never>((_resolve, reject) => {
            signal?.addEventListener("abort", () => reject(signal.reason ?? new Error("aborted")), {
              once: true,
            });
          }),
      ),
    });
    const runtime = new CovenAcpRuntime({ config: { ...config, allowFallback: true }, client });

    const pending = runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });
    await vi.advanceTimersByTimeAsync(5_000);
    const handle = await pending;

    expect(handle.backend).toBe("acpx");
  });

  it("launches a Coven session and streams output events to ACP", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
      }),
    );

    expect(client.launchSession).toHaveBeenCalledWith(
      expect.objectContaining({
        projectRoot: workspaceDir,
        cwd: workspaceDir,
        harness: "codex",
        prompt: "Fix tests",
      }),
      undefined,
    );
    expect(handle.backendSessionId).toBe("session-1");
    expect(events).toEqual([
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
      expect.objectContaining({ type: "text_delta", text: "hello\n" }),
      expect.objectContaining({ type: "status", text: "coven session completed exitCode=0" }),
      expect.objectContaining({ type: "done", stopReason: "completed" }),
    ]);
  });

  it("rejects unknown ACP agent ids instead of forwarding them as Coven harness names", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });

    await expect(
      runtime.ensureSession({
        sessionKey: "agent:attacker:test",
        agent: "attacker-harness",
        mode: "oneshot",
        cwd: workspaceDir,
      }),
    ).rejects.toThrow(/Unknown or unauthorized ACP agent/);
    expect(client.health).not.toHaveBeenCalled();
  });

  it("keeps future harness ids out of the default OpenClaw mapping", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });

    await expect(
      runtime.ensureSession({
        sessionKey: "agent:gemini:test",
        agent: "gemini",
        mode: "oneshot",
        cwd: workspaceDir,
      }),
    ).rejects.toThrow(/Unknown or unauthorized ACP agent/);
    expect(client.health).not.toHaveBeenCalled();
  });

  it("allows explicit configured agent-to-harness mappings", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({
      config: { ...config, harnesses: { assistant: "codex" } },
      client,
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:assistant:test",
      agent: "assistant",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
      }),
    );

    expect(client.launchSession).toHaveBeenCalledWith(
      expect.objectContaining({ harness: "codex" }),
      undefined,
    );
  });

  it("sanitizes daemon-controlled harness fields in start status", async () => {
    const client = fakeClient({
      launchSession: vi.fn(async () =>
        session({
          harness: "\u001b[31mcodex\u001b[0m",
        }),
      ),
    });
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
      }),
    );

    expect(events).toContainEqual(
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
    );
  });

  it("rejects unsafe daemon-controlled session ids before exposing handle fields", async () => {
    const client = fakeClient({
      launchSession: vi.fn(async () =>
        session({
          id: "\u001b]0;spoof\u0007session-1\r",
        }),
      ),
    });
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await expect(
      collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: "req-1",
        }),
      ),
    ).rejects.toThrow(/session id is invalid/);
    expect(handle.backendSessionId).toBeUndefined();
    expect(handle.agentSessionId).toBeUndefined();
    expect(client.killSession).toHaveBeenCalledWith("\u001b]0;spoof\u0007session-1\r", undefined);
  });

  it("kills an already-launched Coven session before falling back on unsafe session ids", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const client = fakeClient({
      launchSession: vi.fn(async () => session({ id: "bad\nsession" })),
      killSession: vi.fn(async () => undefined),
    });
    const runtime = new CovenAcpRuntime({ config: { ...config, allowFallback: true }, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
      }),
    );

    expect(client.killSession).toHaveBeenCalledWith("bad\nsession", undefined);
    expect(handle.backend).toBe("acpx");
    expect(events).toEqual([
      expect.objectContaining({ type: "text_delta", text: "direct fallback\n" }),
      expect.objectContaining({ type: "done", stopReason: "complete" }),
    ]);
  });

  it("fails closed without launching Coven when prompts exceed the Coven request limit", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await expect(
      collect(
        runtime.runTurn({
          handle,
          text: "x".repeat(500_001),
          mode: "prompt",
          requestId: "req-1",
        }),
      ),
    ).rejects.toThrow(/fallback is disabled/);

    expect(client.launchSession).not.toHaveBeenCalled();
  });

  it("falls back on oversized prompts when fallback is explicitly enabled", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config: { ...config, allowFallback: true }, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "x".repeat(500_001),
        mode: "prompt",
        requestId: "req-1",
      }),
    );

    expect(client.launchSession).not.toHaveBeenCalled();
    expect(events).toEqual([
      expect.objectContaining({ type: "text_delta", text: "direct fallback\n" }),
      expect.objectContaining({ type: "done", stopReason: "complete" }),
    ]);
  });

  it("ignores cwd embedded in runtimeSessionName when launching Coven sessions", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });
    handle.runtimeSessionName = `coven:${Buffer.from(
      JSON.stringify({
        agent: "codex",
        mode: "prompt",
        cwd: "/tmp/attacker",
      }),
      "utf8",
    ).toString("base64url")}`;

    await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
      }),
    );

    expect(client.launchSession).toHaveBeenCalledWith(
      expect.objectContaining({
        projectRoot: workspaceDir,
        cwd: workspaceDir,
      }),
      undefined,
    );
  });

  it("rejects Coven handles whose cwd is outside the configured workspace", async () => {
    const runtime = new CovenAcpRuntime({ config, client: fakeClient() });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });
    handle.cwd = "/tmp/attacker";

    await expect(
      collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: "req-1",
        }),
      ),
    ).rejects.toThrow(/outside workspace/);
  });

  it("rejects Coven cwd symlinks that resolve outside the workspace", async () => {
    const workspaceDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-coven-workspace-"));
    const outsideDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-coven-outside-"));
    const symlinkPath = path.join(workspaceDir, "outside");
    await fs.symlink(outsideDir, symlinkPath);
    try {
      const runtime = new CovenAcpRuntime({
        config: { ...config, workspaceDir },
        client: fakeClient(),
      });
      const handle = await runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: symlinkPath,
      });

      await expect(
        collect(
          runtime.runTurn({
            handle,
            text: "Fix tests",
            mode: "prompt",
            requestId: "req-1",
          }),
        ),
      ).rejects.toThrow(/outside workspace/);
    } finally {
      await fs.rm(workspaceDir, { recursive: true, force: true });
      await fs.rm(outsideDir, { recursive: true, force: true });
    }
  });

  it("requests incremental events after the last processed Coven event", async () => {
    const client = fakeClient({
      listEvents: vi
        .fn()
        .mockResolvedValueOnce([
          event({
            id: "event-1",
            kind: "output",
            payloadJson: JSON.stringify({ data: "hello\n" }),
          }),
        ])
        .mockResolvedValueOnce([
          event({
            id: "event-2",
            kind: "exit",
            payloadJson: JSON.stringify({ status: "completed", exitCode: 0 }),
          }),
        ]),
      getSession: vi
        .fn()
        .mockResolvedValueOnce(session({ status: "running" }))
        .mockResolvedValueOnce(session({ status: "completed", exitCode: 0 })),
    });
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(client.listEvents).toHaveBeenNthCalledWith(
      2,
      "session-1",
      {
        afterEventId: "event-1",
      },
      undefined,
    );
    expect(events).not.toContainEqual(
      expect.objectContaining({ type: "status", text: "coven session polling failed" }),
    );
    expect(client.killSession).not.toHaveBeenCalled();
  });

  it("does not treat conversational idle as one-shot completion", async () => {
    const getSession = vi
      .fn()
      .mockResolvedValueOnce(session({ status: "idle" }))
      .mockResolvedValueOnce(session({ status: "completed", exitCode: 0 }));
    const runtime = new CovenAcpRuntime({
      config: { ...config, pollIntervalMs: 25 },
      client: fakeClient({
        listEvents: vi.fn(async () => []),
        getSession,
      }),
      sleep: vi.fn(async () => undefined),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-idle" }),
    );

    expect(getSession).toHaveBeenCalledTimes(2);
    expect(events.at(-1)).toEqual({ type: "done", stopReason: "completed" });
  });

  it.each(["created", "running"])(
    "continues polling for known nonterminal harness-session status %s",
    async (status) => {
      const getSession = vi
        .fn()
        .mockResolvedValueOnce(session({ status }))
        .mockResolvedValueOnce(session({ status: "completed", exitCode: 0 }));
      const runtime = new CovenAcpRuntime({
        config,
        client: fakeClient({
          listEvents: vi.fn(async () => []),
          getSession,
        }),
        sleep: vi.fn(async () => undefined),
      });
      const handle = await runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: workspaceDir,
      });

      const events = await collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: `req-${status}`,
        }),
      );

      expect(getSession).toHaveBeenCalledTimes(2);
      expect(events.at(-1)).toEqual({ type: "done", stopReason: "completed" });
    },
  );

  it.each([
    ["failed", "error"],
    ["killed", "cancelled"],
    ["orphaned", "error"],
  ])(
    "terminates polling for known terminal harness-session status %s",
    async (status, stopReason) => {
      const getSession = vi.fn(async () => session({ status }));
      const runtime = new CovenAcpRuntime({
        config,
        client: fakeClient({
          listEvents: vi.fn(async () => []),
          getSession,
        }),
      });
      const handle = await runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: workspaceDir,
      });

      const events = await collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: `req-${status}`,
        }),
      );

      expect(getSession).toHaveBeenCalledTimes(1);
      expect(events.at(-2)).toEqual(
        expect.objectContaining({ type: "status", text: `coven session ${status}` }),
      );
      expect(events.at(-1)).toEqual({ type: "done", stopReason });
    },
  );

  it("uses persisted killed state instead of a late completed exit event", async () => {
    const getSession = vi.fn(async () => session({ status: "killed" }));
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        listEvents: vi.fn(async () => [
          event({
            kind: "exit",
            payloadJson: JSON.stringify({ status: "completed", exitCode: 0 }),
          }),
        ]),
        getSession,
      }),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-late-exit",
      }),
    );

    expect(getSession).toHaveBeenCalledTimes(1);
    expect(events).toEqual([
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
      expect.objectContaining({ type: "status", text: "coven session killed" }),
      { type: "done", stopReason: "cancelled" },
    ]);
  });

  it("continues polling when an exit event arrives while persisted state is idle", async () => {
    const getSession = vi
      .fn()
      .mockResolvedValueOnce(session({ status: "idle" }))
      .mockResolvedValueOnce(session({ status: "completed", exitCode: 0 }));
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        listEvents: vi
          .fn()
          .mockResolvedValueOnce([
            event({
              kind: "exit",
              payloadJson: JSON.stringify({ status: "completed", exitCode: 0 }),
            }),
          ])
          .mockResolvedValueOnce([]),
        getSession,
      }),
      sleep: vi.fn(async () => undefined),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-idle-exit",
      }),
    );

    expect(getSession).toHaveBeenCalledTimes(2);
    expect(events).toEqual([
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
      expect.objectContaining({ type: "status", text: "coven session completed exitCode=0" }),
      { type: "done", stopReason: "completed" },
    ]);
  });

  it.each([
    ["exit", "completed", "failed", "error"],
    ["kill", "killed", "completed", "completed"],
  ])(
    "uses persisted ledger status instead of conflicting %s event evidence",
    async (kind, eventStatus, ledgerStatus, stopReason) => {
      const runtime = new CovenAcpRuntime({
        config,
        client: fakeClient({
          listEvents: vi.fn(async () => [
            event({
              kind,
              payloadJson: JSON.stringify({ status: eventStatus, exitCode: 0 }),
            }),
          ]),
          getSession: vi.fn(async () => session({ status: ledgerStatus })),
        }),
      });
      const handle = await runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: workspaceDir,
      });

      const events = await collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: `req-${kind}-disagreement`,
        }),
      );

      expect(events.at(-2)).toEqual(
        expect.objectContaining({ type: "status", text: `coven session ${ledgerStatus}` }),
      );
      expect(events.at(-1)).toEqual({ type: "done", stopReason });
    },
  );

  it("does not terminate on kill event evidence before persisted killed state", async () => {
    const getSession = vi
      .fn()
      .mockResolvedValueOnce(session({ status: "running" }))
      .mockResolvedValueOnce(session({ status: "killed" }));
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        listEvents: vi
          .fn()
          .mockResolvedValueOnce([event({ kind: "kill", payloadJson: "{}" })])
          .mockResolvedValueOnce([]),
        getSession,
      }),
      sleep: vi.fn(async () => undefined),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-kill-evidence",
      }),
    );

    expect(getSession).toHaveBeenCalledTimes(2);
    expect(events.at(-2)).toEqual(
      expect.objectContaining({ type: "status", text: "coven session killed" }),
    );
    expect(events.at(-1)).toEqual({ type: "done", stopReason: "cancelled" });
  });

  it.each(["future_state", "active"])(
    "fails closed on unsupported harness-session status %s",
    async (status) => {
      const runtime = new CovenAcpRuntime({
        config,
        client: fakeClient({
          listEvents: vi.fn(async () => []),
          getSession: vi.fn(async () => session({ status })),
        }),
      });
      const handle = await runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: workspaceDir,
      });

      const events = await collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: "req-unknown",
        }),
      );

      expect(events.at(-1)).toEqual({ type: "done", stopReason: "error" });
    },
  );

  it("sanitizes and bounds unsupported-status logs while attempting cleanup", async () => {
    const logger = { warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() };
    const killSession = vi.fn(async () => undefined);
    const unsafeStatus = `future\u001b]0;spoof\u0007_state\u202e${"x".repeat(500)}`;
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        listEvents: vi.fn(async () => []),
        getSession: vi.fn(async () => session({ status: unsafeStatus })),
        killSession,
      }),
      logger,
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-unsafe-status",
      }),
    );

    expect(killSession).toHaveBeenCalledWith("session-1", undefined);
    expect(events.at(-1)).toEqual({ type: "done", stopReason: "error" });
    const warning = vi.mocked(logger.warn).mock.calls[0]?.[0];
    expect(warning).toEqual(expect.any(String));
    expect(warning).not.toContain(unsafeStatus);
    expect(warning).not.toMatch(/[\u001b\u0007\u202e\r\n]/u);
    expect(warning?.length).toBeLessThanOrEqual(280);
  });

  it("fails and kills the Coven session when the daemon returns an unsafe event id", async () => {
    const client = fakeClient({
      listEvents: vi.fn(async () => [
        event({
          id: "e".repeat(257),
          kind: "output",
          payloadJson: JSON.stringify({ data: "hello\n" }),
        }),
      ]),
      killSession: vi.fn(async () => undefined),
    });
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(client.killSession).toHaveBeenCalledWith("session-1", undefined);
    expect(events).toEqual([
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
      expect.objectContaining({ type: "status", text: "coven session polling failed" }),
      expect.objectContaining({ type: "done", stopReason: "error" }),
    ]);
  });

  it("clamps malformed runtime poll intervals before sleeping", async () => {
    const sleep = vi.fn(async () => undefined);
    const client = fakeClient({
      listEvents: vi
        .fn()
        .mockResolvedValueOnce([])
        .mockResolvedValueOnce([
          event({
            id: "event-1",
            kind: "exit",
            payloadJson: JSON.stringify({ status: "completed", exitCode: 0 }),
          }),
        ]),
      getSession: vi.fn(async () => session({ status: "running" })),
    });
    const runtime = new CovenAcpRuntime({
      config: { ...config, pollIntervalMs: 0 },
      client,
      sleep,
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(sleep).toHaveBeenCalledWith(25, undefined);
  });

  it("fails the turn when the daemon returns too many events in one poll", async () => {
    const client = fakeClient({
      listEvents: vi.fn(async () =>
        Array.from({ length: 600 }, (_, index) =>
          event({
            id: `event-${index}`,
            kind: "output",
            payloadJson: JSON.stringify({ data: `line-${index}\n` }),
          }),
        ),
      ),
      killSession: vi.fn(async () => undefined),
    });
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(client.killSession).toHaveBeenCalledWith("session-1", undefined);
    expect(events).toEqual([
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
      expect.objectContaining({ type: "status", text: "coven session polling failed" }),
      expect.objectContaining({ type: "done", stopReason: "error" }),
    ]);
  });

  it("converts Coven polling failures into controlled terminal events", async () => {
    const client = fakeClient({
      listEvents: vi.fn(async () => {
        throw new Error("bad json");
      }),
      killSession: vi.fn(async () => undefined),
    });
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(client.killSession).toHaveBeenCalledWith("session-1", undefined);
    expect(events).toEqual([
      expect.objectContaining({ type: "status", text: "coven session session-1 started (codex)" }),
      expect.objectContaining({ type: "status", text: "coven session polling failed" }),
      expect.objectContaining({ type: "done", stopReason: "error" }),
    ]);
  });

  it("sanitizes Coven polling errors before logging", async () => {
    const logger = { warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() };
    const client = fakeClient({
      listEvents: vi.fn(async () => {
        throw new Error("\u001b]0;spoof\u0007bad\r\njson");
      }),
      killSession: vi.fn(async () => undefined),
    });
    const runtime = new CovenAcpRuntime({ config, client, logger });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(logger.warn).toHaveBeenCalledWith("coven polling failed: Error: bad json");
  });

  it("strips terminal escape and control characters from Coven output", () => {
    expect(
      __testing.sanitizeTerminalText(
        "\u001b]0;spoof\u0007hi\u001b[31m!\u001b[0m\u001b7\u001bc\u202e\r\n",
      ),
    ).toBe("hi!\n");
  });

  it("sanitizes prompt-derived session titles", () => {
    expect(__testing.titleFromPrompt("\u001b]0;spoof\u0007Fix\u001b[31m tests\r\nnow")).toBe(
      "Fix tests now",
    );
  });

  it("fails closed on untrusted persisted stop reasons and ignores terminal event outcomes", () => {
    expect(__testing.normalizeStopReason("completed")).toBe("completed");
    expect(__testing.normalizeStopReason("killed")).toBe("cancelled");
    expect(__testing.normalizeStopReason("orphaned")).toBe("error");
    expect(__testing.normalizeStopReason("refusal")).toBe("error");

    expect(
      __testing.eventToRuntimeEvents(
        event({
          kind: "exit",
          payloadJson: JSON.stringify({ status: "refusal", exitCode: 0 }),
        }),
      ),
    ).toEqual([]);
    expect(
      __testing.eventToRuntimeEvents(event({ kind: "kill", payloadJson: "{}" })),
    ).toEqual([]);
  });

  it("guards daemon exitCode types before rendering terminal status text", () => {
    expect(
      __testing.terminalStatusEvent(
        session({ status: "completed", exitCode: "\u001b[31m1" as unknown as number }),
      ),
    ).toEqual({
      type: "status",
      text: "coven session completed",
      tag: "session_info_update",
    });
  });

  it("drops oversized daemon event payloads before parsing", () => {
    expect(
      __testing.eventToRuntimeEvents(
        event({
          kind: "output",
          payloadJson: JSON.stringify({ data: "x".repeat(64_001) }),
        }),
      ),
    ).toEqual([]);
  });

  it("rejects oversized Coven runtime session metadata", () => {
    expect(__testing.decodeRuntimeSessionName(`coven:${"a".repeat(2_049)}`)).toBeNull();
  });

  it("bounds encoded Coven runtime session metadata before persistence", () => {
    const encoded = __testing.encodeRuntimeSessionName({
      agent: "A".repeat(5_000),
      mode: "prompt".repeat(1_000),
      sessionMode: "persistent".repeat(1_000),
    });

    expect(Buffer.byteLength(encoded, "utf8")).toBeLessThanOrEqual("coven:".length + 2_048);
    expect(__testing.decodeRuntimeSessionName(encoded)).toEqual({
      agent: "a".repeat(128),
      mode: "promptpromptpromptpromptpromptpr",
      sessionMode: "persistentpersistentpersistentpe",
    });
  });

  it("rejects missing Coven cwd paths before launching", async () => {
    const workspaceDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-coven-workspace-"));
    try {
      const runtime = new CovenAcpRuntime({
        config: { ...config, workspaceDir },
        client: fakeClient(),
      });
      const handle = await runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: path.join(workspaceDir, "missing"),
      });

      await expect(
        collect(
          runtime.runTurn({
            handle,
            text: "Fix tests",
            mode: "prompt",
            requestId: "req-1",
          }),
        ),
      ).rejects.toThrow(/outside workspace/);
    } finally {
      await fs.rm(workspaceDir, { recursive: true, force: true });
    }
  });

  it("rejects Coven cwd paths that are not directories", async () => {
    const filePath = path.join(workspaceDir, "not-a-directory");
    await fs.writeFile(filePath, "not a directory");
    const runtime = new CovenAcpRuntime({ config, client: fakeClient() });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: filePath,
    });

    await expect(
      collect(
        runtime.runTurn({
          handle,
          text: "Fix tests",
          mode: "prompt",
          requestId: "req-1",
        }),
      ),
    ).rejects.toThrow(/cwd must be a directory/);
  });

  it("does not trust persisted backendSessionId without an active tracked Coven session", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });
    const handle: AcpRuntimeHandle = {
      sessionKey: "agent:codex:test",
      backend: "coven",
      runtimeSessionName: __testing.encodeRuntimeSessionName({
        agent: "codex",
        mode: "prompt",
      }),
      cwd: workspaceDir,
      backendSessionId: "attacker-session",
    };

    await expect(runtime.getStatus({ handle })).resolves.toEqual({
      summary: "coven runtime ready",
    });
    await expect(runtime.cancel({ handle })).resolves.toBeUndefined();
    await expect(runtime.close({ handle, reason: "user" })).resolves.toBeUndefined();
    expect(client.getSession).not.toHaveBeenCalledWith("attacker-session", undefined);
    expect(client.killSession).not.toHaveBeenCalledWith("attacker-session", undefined);
  });

  it("rejects backendSessionId values that conflict with the active tracked Coven session", async () => {
    const client = fakeClient();
    const runtime = new CovenAcpRuntime({ config, client });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });
    const turn = runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" });
    const iterator = turn[Symbol.asyncIterator]();
    await iterator.next();
    handle.backendSessionId = "attacker-session";

    await expect(runtime.getStatus({ handle })).rejects.toThrow(/does not match/);
    await expect(runtime.cancel({ handle })).rejects.toThrow(/does not match/);
    await expect(runtime.close({ handle, reason: "user" })).rejects.toThrow(/does not match/);
    expect(client.getSession).not.toHaveBeenCalledWith("attacker-session", undefined);
    expect(client.killSession).not.toHaveBeenCalledWith("attacker-session", undefined);
    await iterator.return?.();
  });

  it("preserves direct fallback when Coven launch fails after detection", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client: fakeClient({
        launchSession: vi.fn(async () => {
          throw new Error("launch failed");
        }),
      }),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(handle.backend).toBe("acpx");
    expect(events).toEqual([
      expect.objectContaining({ type: "text_delta", text: "direct fallback\n" }),
      expect.objectContaining({ type: "done", stopReason: "complete" }),
    ]);
  });

  it("fails closed when Coven launch fails after detection and fallback is disabled", async () => {
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        launchSession: vi.fn(async () => {
          throw new Error("\u001b]0;spoof\u0007launch\r\nfailed");
        }),
      }),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await expect(
      collect(runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" })),
    ).rejects.toThrow(/Error: launch failed/);
  });

  it("reports unsupported named Coven API contracts in doctor", async () => {
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        health: vi.fn(async () => compatibleHealth({ apiVersion: "coven.daemon.v2" })),
      }),
    });

    await expect(runtime.doctor()).resolves.toMatchObject({
      ok: false,
      code: "COVEN_UNSUPPORTED_API_VERSION",
      message: "Coven daemon API version is not supported.",
      details: [
        "expected apiVersion coven.daemon.v1, got coven.daemon.v2; upgrade Coven to a compatible version",
      ],
    });
  });

  it.each([
    ["absent", undefined, "missing"],
    ["empty", "", "missing"],
    ["non-string", 2, "invalid"],
    ["ANSI", "\u001b[31mcoven.daemon.v2", "invalid"],
    ["newline", "coven.daemon.v2\nspoof", "invalid"],
    ["bidi", "coven.daemon.v2\u202espoof", "invalid"],
    ["oversized", `coven.daemon.${"v".repeat(1_024)}`, "invalid"],
  ])("bounds %s Coven API version diagnostics", async (_name, apiVersion, actual) => {
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        health: vi.fn(async () => compatibleHealth({ apiVersion })),
      }),
    });

    await expect(runtime.doctor()).resolves.toMatchObject({
      ok: false,
      code: "COVEN_UNSUPPORTED_API_VERSION",
      details: [
        `expected apiVersion coven.daemon.v1, got ${actual}; upgrade Coven to a compatible version`,
      ],
    });
  });

  it.each([
    [
      "sessions",
      { sessions: false },
      "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
    ],
    [
      "events",
      { events: false },
      "expected capabilities.events to be true; upgrade Coven to a compatible version",
    ],
    [
      "event cursor",
      { eventCursor: "offset" },
      "expected capabilities.eventCursor to be sequence; upgrade Coven to a compatible version",
    ],
    [
      "structured errors",
      { structuredErrors: false },
      "expected capabilities.structuredErrors to be true; upgrade Coven to a compatible version",
    ],
  ])(
    "fails closed when Coven reports an unsupported %s capability",
    async (_name, override, detail) => {
      const launchSession = vi.fn(async () => session());
      const health = compatibleHealth({
        capabilities: { ...compatibleHealth().capabilities, ...override },
      });
      const runtime = new CovenAcpRuntime({
        config,
        client: fakeClient({ health: vi.fn(async () => health), launchSession }),
      });

      await expect(runtime.doctor()).resolves.toMatchObject({
        ok: false,
        code: "COVEN_UNSUPPORTED_CAPABILITY",
        message: "Coven daemon capability is not supported.",
        details: [detail],
      });
      await expect(
        runtime.ensureSession({
          sessionKey: "agent:codex:test",
          agent: "codex",
          mode: "oneshot",
          cwd: workspaceDir,
        }),
      ).rejects.toThrow(detail);
      expect(launchSession).not.toHaveBeenCalled();
    },
  );

  it.each([
    [
      "missing capabilities",
      undefined,
      "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
    ],
    [
      "missing sessions",
      { events: true, eventCursor: "sequence", structuredErrors: true },
      "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
    ],
    [
      "missing events",
      { sessions: true, eventCursor: "sequence", structuredErrors: true },
      "expected capabilities.events to be true; upgrade Coven to a compatible version",
    ],
    [
      "malformed event cursor",
      { sessions: true, events: true, eventCursor: false, structuredErrors: true },
      "expected capabilities.eventCursor to be sequence; upgrade Coven to a compatible version",
    ],
    [
      "malformed structured errors",
      { sessions: true, events: true, eventCursor: "sequence", structuredErrors: "yes" },
      "expected capabilities.structuredErrors to be true; upgrade Coven to a compatible version",
    ],
  ])("fails closed when Coven reports %s", async (_name, capabilities, detail) => {
    const launchSession = vi.fn(async () => session());
    const health = compatibleHealth({ capabilities });
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({ health: vi.fn(async () => health), launchSession }),
    });

    await expect(runtime.doctor()).resolves.toMatchObject({
      ok: false,
      code: "COVEN_UNSUPPORTED_CAPABILITY",
      message: "Coven daemon capability is not supported.",
      details: [detail],
    });
    await expect(
      runtime.ensureSession({
        sessionKey: "agent:codex:test",
        agent: "codex",
        mode: "oneshot",
        cwd: workspaceDir,
      }),
    ).rejects.toThrow(detail);
    expect(launchSession).not.toHaveBeenCalled();
  });

  it("rechecks Coven compatibility immediately before launch", async () => {
    const launchSession = vi.fn(async () => session());
    const health = vi
      .fn<CovenClient["health"]>()
      .mockResolvedValueOnce(compatibleHealth())
      .mockResolvedValueOnce(
        compatibleHealth({
          capabilities: { ...compatibleHealth().capabilities, sessions: false },
        }),
      );
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({ health, launchSession }),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    await expect(
      collect(runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" })),
    ).rejects.toThrow(
      "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
    );
    expect(health).toHaveBeenCalledTimes(2);
    expect(launchSession).not.toHaveBeenCalled();
  });

  it("falls back without launching when capabilities drift before launch", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const launchSession = vi.fn(async () => session());
    const health = vi
      .fn<CovenClient["health"]>()
      .mockResolvedValueOnce(compatibleHealth())
      .mockResolvedValueOnce(
        compatibleHealth({
          capabilities: { ...compatibleHealth().capabilities, sessions: false },
        }),
      );
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client: fakeClient({ health, launchSession }),
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    const events = await collect(
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-1" }),
    );

    expect(launchSession).not.toHaveBeenCalled();
    expect(fallback.ensureSession).toHaveBeenCalledOnce();
    expect(handle.backend).toBe("acpx");
    expect(events).toEqual([
      expect.objectContaining({ type: "text_delta", text: "direct fallback\n" }),
      expect.objectContaining({ type: "done", stopReason: "complete" }),
    ]);
  });

  it("propagates caller cancellation during the pre-launch health check without fallback", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const launchSession = vi.fn(async () => session());
    let markSecondHealthStarted!: () => void;
    const secondHealthStarted = new Promise<void>((resolve) => {
      markSecondHealthStarted = resolve;
    });
    const health = vi
      .fn<CovenClient["health"]>()
      .mockResolvedValueOnce(compatibleHealth())
      .mockImplementationOnce(
        async (signal) =>
          await new Promise<never>((_resolve, reject) => {
            markSecondHealthStarted();
            signal?.addEventListener(
              "abort",
              () => reject(signal.reason ?? new Error("aborted")),
              { once: true },
            );
          }),
      );
    const client = fakeClient({ health, launchSession });
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client,
    });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });
    const originalHandle = { ...handle };
    const controller = new AbortController();
    const removeAbortListener = vi.spyOn(controller.signal, "removeEventListener");
    const cancellation = new Error("caller cancelled");
    const pending = collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
        signal: controller.signal,
      }),
    );
    await secondHealthStarted;

    controller.abort(cancellation);

    await expect(pending).rejects.toBe(cancellation);
    expect(launchSession).not.toHaveBeenCalled();
    expect(fallback.ensureSession).not.toHaveBeenCalled();
    expect(handle).toEqual(originalHandle);
    expect(removeAbortListener).toHaveBeenCalledWith("abort", expect.any(Function));
  });

  it("keeps the pre-launch health deadline when a caller signal is present", async () => {
    vi.useFakeTimers();
    const launchSession = vi.fn(async () => session());
    let observedSecondSignal: AbortSignal | undefined;
    let markSecondHealthStarted!: () => void;
    const secondHealthStarted = new Promise<void>((resolve) => {
      markSecondHealthStarted = resolve;
    });
    const health = vi.fn<CovenClient["health"]>(async (signal) => {
      if (health.mock.calls.length === 1) {
        return compatibleHealth();
      }
      observedSecondSignal = signal;
      markSecondHealthStarted();
      return await new Promise<never>((_resolve, reject) => {
        signal?.addEventListener(
          "abort",
          () => reject(signal.reason ?? new Error("aborted")),
          { once: true },
        );
      });
    });
    const runtime = new CovenAcpRuntime({ config, client: fakeClient({ health, launchSession }) });
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });
    const controller = new AbortController();
    const pending = collect(
      runtime.runTurn({
        handle,
        text: "Fix tests",
        mode: "prompt",
        requestId: "req-1",
        signal: controller.signal,
      }),
    );
    const settled = pending.catch((error: unknown) => error);
    await secondHealthStarted;

    await vi.advanceTimersByTimeAsync(5_000);
    const abortedAtDeadline = observedSecondSignal?.aborted ?? false;
    const reasonAtDeadline = observedSecondSignal?.reason;
    if (!abortedAtDeadline) {
      controller.abort(new Error("test cleanup"));
    }
    const deadlineError = await settled;

    expect(abortedAtDeadline).toBe(true);
    expect(reasonAtDeadline).toEqual(new Error("Coven health check timed out"));
    expect(controller.signal.aborted).toBe(false);
    expect(deadlineError).toMatchObject({ code: "ACP_TURN_FAILED" });
    expect(launchSession).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("falls back when Coven reports an unsupported API version and fallback is enabled", async () => {
    const fallback = fallbackRuntime();
    registerAcpRuntimeBackend({ id: "acpx", runtime: fallback });
    const launchSession = vi.fn(async () => session());
    const runtime = new CovenAcpRuntime({
      config: { ...config, allowFallback: true },
      client: fakeClient({
        health: vi.fn(async () => compatibleHealth({ apiVersion: "coven.daemon.v2" })),
        launchSession,
      }),
    });

    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    });

    expect(handle.backend).toBe("acpx");
    expect(launchSession).not.toHaveBeenCalled();
  });

  it("sanitizes Coven doctor error details", async () => {
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        health: vi.fn(async () => {
          throw new Error("\u001b[31moffline\r\nnow");
        }),
      }),
    });

    await expect(runtime.doctor()).resolves.toMatchObject({
      ok: false,
      details: ["Error: offline now"],
    });
  });
});
