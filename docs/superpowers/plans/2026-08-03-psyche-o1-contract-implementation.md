# Psyche O1 Coven Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Coven's named-contract handshake and harness-session lifecycle vocabulary internally consistent without changing the legacy `/api-version` wire response or implementing later Psyche adoption and cancellation contracts.

**Architecture:** Keep `/api/v1/health` as the canonical compatibility handshake for `coven.daemon.v1`, and retain `/api/v1/api-version` as a legacy route-token diagnostic. Migrate the first-party OpenClaw bridge to fail closed on the named contract and the exact session/event capabilities it consumes, interpret persisted lifecycle values explicitly, and add a stdlib documentation guardrail so public references cannot drift back to the contradictory vocabulary.

**Tech Stack:** Rust 2021, Serde/serde_json, TypeScript, Vitest, Python 3 stdlib, Markdown.

---

## File map

- `crates/coven-cli/src/api.rs` - distinguish the HTTP route-family token from the named daemon contract while preserving both wire responses.
- `packages/openclaw-coven/src/client.ts` - parse the health envelope while preserving capability values as untrusted input for runtime policy.
- `packages/openclaw-coven/src/client.test.ts` - prove health parsing does not promote untrusted capability values into typed authority.
- `packages/openclaw-coven/src/runtime.ts` - apply the OpenClaw bridge's fail-closed named-contract/capability policy and explicit lifecycle interpretation.
- `packages/openclaw-coven/src/runtime.test.ts` - prove mismatches stop before launch and `idle`/unknown states are not inferred as completed work.
- `packages/openclaw-coven/package.json` - provide reproducible test/typecheck scripts and exact test-time dependencies.
- `packages/openclaw-coven/package-lock.json` - keep the package's CI install reproducible.
- `.github/workflows/ci.yml` - run the OpenClaw bridge checks in CI.
- `docs/API-CONTRACT.md` - canonical single-page handshake, legacy route diagnostic, and `SessionRecord.status` contract.
- `docs/reference/api-contract.md` - condensed named-contract negotiation guidance.
- `docs/reference/api.md` - endpoint description that identifies `/api-version` as a route diagnostic.
- `docs/API.md` - public API overview using the same health handshake and route-token terminology.
- `docs/daemon/socket-api.md` - correct health capabilities and handshake guidance.
- `docs/daemon/capabilities-handshake.md` - state that capability availability does not grant authorization.
- `docs/ARCHITECTURE.md` - remove the obsolete health `supportedApiVersions` claim.
- `docs/SESSION-LIFECYCLE.md` - complete persisted harness-session state and archive semantics.
- `docs/sessions/lifecycle.md` - client-facing lifecycle vocabulary using persisted names.
- `scripts/check-api-contract-docs.py` - deterministic cross-document O1 contract guardrail.
- `scripts/check-api-contract-docs-test.py` - focused positive and negative tests for the guardrail.

### Task 0: Make the OpenClaw bridge testable

**Files:**
- Modify: `packages/openclaw-coven/package.json`
- Modify: `packages/openclaw-coven/package-lock.json`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Declare the package checks and exact test dependencies**

Preserve all package metadata and peer dependencies. Add the exact OpenClaw test
version required by `openclaw.build.openclawVersion`, TypeScript, Node types, and
Vitest. The package scripts are `typecheck` (`tsc --noEmit`) and `test`
(`vitest run`); do not refer to a nonexistent `build` script. Keep the npm
lockfile used by the package's CI job synchronized with `package.json`.

- [ ] **Step 2: Install reproducibly and prove the package checks**

Run:

```bash
npm --prefix packages/openclaw-coven ci --ignore-scripts
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test -- src/client.test.ts src/runtime.test.ts
```

Expected: the frozen `package-lock.json` install succeeds without lifecycle
scripts, TypeScript passes, and the focused Vitest suites execute.

- [ ] **Step 3: Keep CI synchronized with the package**

The `openclaw-bridge` job must use the repository's current checkout and Node
action versions, install from `packages/openclaw-coven/package-lock.json` with
`npm ci --ignore-scripts`, then run `npm run typecheck` and `npm test`. Do not
add a build invocation unless a `build` script is first added to `package.json`.
Task 5 adds the documentation guard to the existing Python policy job.

- [ ] **Step 4: Commit the package test workflow**

```bash
git add packages/openclaw-coven/package.json packages/openclaw-coven/package-lock.json .github/workflows/ci.yml
git commit -m "test(openclaw): add reproducible package checks"
```

### Task 1: Freeze route-token compatibility in Rust

**Files:**
- Modify: `crates/coven-cli/src/api.rs:27-31`
- Modify: `crates/coven-cli/src/api.rs:292-318`
- Test: `crates/coven-cli/src/api.rs` test module near `routes_versioned_health_request_to_named_api_contract`

- [ ] **Step 1: Add failing tests for both compatibility identities**

Add these tests beside the existing health route tests:

```rust
#[test]
fn legacy_api_version_route_remains_a_route_token_diagnostic() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let response = handle_request("GET", "/api/v1/api-version", temp_dir.path(), None)?;
    let body: serde_json::Value = serde_json::from_str(&response.body)?;

    assert_eq!(response.status, 200);
    assert_eq!(body["apiVersion"], COVEN_API_ROUTE_VERSION);
    assert_eq!(body["apiVersion"], "v1");
    assert_eq!(
        body["supportedApiVersions"],
        json!(SUPPORTED_API_ROUTE_VERSIONS)
    );
    assert_eq!(body["supportedApiVersions"], json!(["v1"]));
    Ok(())
}

#[test]
fn health_is_the_named_contract_handshake() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let response = handle_request("GET", "/api/v1/health", temp_dir.path(), None)?;
    let body: serde_json::Value = serde_json::from_str(&response.body)?;

    assert_eq!(response.status, 200);
    assert_eq!(body["apiVersion"], COVEN_API_NAMED_VERSION);
    assert_eq!(body["apiVersion"], "coven.daemon.v1");
    assert!(body.get("supportedApiVersions").is_none());
    assert_eq!(body["capabilities"]["sessions"], true);
    assert_eq!(body["capabilities"]["events"], true);
    assert_eq!(body["capabilities"]["eventCursor"], "sequence");
    assert_eq!(body["capabilities"]["structuredErrors"], true);
    Ok(())
}
```

- [ ] **Step 2: Run the focused Rust tests and confirm the new naming expectation fails**

Run:

```bash
cargo test -p coven-cli api::tests::legacy_api_version_route_remains_a_route_token_diagnostic
```

Expected: compilation fails because `COVEN_API_ROUTE_VERSION` and
`SUPPORTED_API_ROUTE_VERSIONS` do not exist yet.

- [ ] **Step 3: Rename route constants without changing serialized values**

Replace the version constants and update every route-normalization, unsupported-route error, and legacy response reference:

```rust
pub const COVEN_API_ROUTE_VERSION: &str = "v1";
pub const COVEN_API_NAMED_VERSION: &str = "coven.daemon.v1";
pub const COVEN_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SUPPORTED_API_ROUTE_VERSIONS: [&str; 1] = [COVEN_API_ROUTE_VERSION];
```

The legacy response must remain:

```rust
("GET", "/api-version") => json_response(
    200,
    &json!({
        "apiVersion": COVEN_API_ROUTE_VERSION,
        "supportedApiVersions": SUPPORTED_API_ROUTE_VERSIONS,
    }),
),
```

The health response must continue using:

```rust
api_version: COVEN_API_NAMED_VERSION.to_string(),
```

- [ ] **Step 4: Run the focused Rust tests**

Run:

```bash
cargo test -p coven-cli api::tests::legacy_api_version_route_remains_a_route_token_diagnostic
cargo test -p coven-cli api::tests::health_is_the_named_contract_handshake
cargo test -p coven-cli api::tests::rejects_unknown_api_version_prefixes
cargo test -p coven-cli api::tests::unknown_harness_capability_manifest_fails_closed_with_structured_error
```

Expected: all four tests pass; the legacy route still returns `v1`, health returns `coven.daemon.v1`, unknown route prefixes remain `404 invalid_request`, and unknown harness capability targets remain `404 harness_not_found`.

- [ ] **Step 5: Commit the Rust compatibility freeze**

```bash
git add crates/coven-cli/src/api.rs
git commit -m "refactor(api): distinguish route and contract versions"
```

### Task 2: Parse and negotiate health capabilities fail closed

**Files:**
- Modify: `packages/openclaw-coven/src/client.ts:27-44`
- Modify: `packages/openclaw-coven/src/client.ts:390-431`
- Test: `packages/openclaw-coven/src/client.test.ts`
- Modify: `packages/openclaw-coven/src/runtime.ts:24-45`
- Modify: `packages/openclaw-coven/src/runtime.ts:280-293`
- Modify: `packages/openclaw-coven/src/runtime.ts:514-577`
- Test: `packages/openclaw-coven/src/runtime.test.ts`

- [ ] **Step 1: Add a failing client test for untrusted capability preservation**

Add this test to `client.test.ts`:

```typescript
it("preserves health capabilities as untrusted wire values", async () => {
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
            events: false,
            eventCursor: "offset",
            structuredErrors: "yes",
          },
          daemon: null,
        }),
      );
    },
    async (socketPath) => {
      await expect(createCovenClient(socketPath).health()).resolves.toMatchObject({
        capabilities: {
          sessions: true,
          events: false,
          eventCursor: "offset",
          structuredErrors: "yes",
        },
      });
    },
  );
});
```

Change `CovenHealthCapabilities` fields to `unknown` and make
`CovenHealthResponse.capabilities` optional so callers cannot consume an
advertisement without validation:

```typescript
export type CovenHealthCapabilities = {
  sessions?: unknown;
  events?: unknown;
  eventCursor?: unknown;
  structuredErrors?: unknown;
};

export type CovenHealthResponse = {
  apiVersion?: unknown;
  covenVersion?: unknown;
  capabilities?: CovenHealthCapabilities;
  ok?: unknown;
  daemon?: unknown;
};
```

- [ ] **Step 2: Add failing runtime negotiation tests**

Change `fakeClient()` to return the real named health contract:

```typescript
health: vi.fn(async () => ({
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
})),
```

Replace the old `v1`/`supportedApiVersions` doctor tests with:

```typescript
it("reports an unsupported named Coven contract in doctor", async () => {
  const runtime = new CovenAcpRuntime({
    config,
    client: fakeClient({
      health: vi.fn(async () => ({
        ...(await fakeClient().health()),
        apiVersion: "coven.daemon.v2",
      })),
    }),
  });

  await expect(runtime.doctor()).resolves.toMatchObject({
    ok: false,
    code: "COVEN_UNSUPPORTED_API_VERSION",
    details: [
      "expected apiVersion coven.daemon.v1, got coven.daemon.v2; upgrade Coven to a compatible version",
    ],
  });
});

it.each([
  [undefined, "missing"],
  [1, "missing"],
  ["", "missing"],
  ["v1", "v1"],
] as const)(
  "treats missing or malformed Coven API version %j as unavailable before launch",
  async (apiVersion, actual) => {
    const launchSession = vi.fn(async () => session());
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        health: vi.fn(async () => ({
          ...(await fakeClient().health()),
          apiVersion,
        })),
        launchSession,
      }),
    });

    const detail =
      `expected apiVersion coven.daemon.v1, got ${actual}; ` +
      "upgrade Coven to a compatible version";
    await expect(runtime.doctor()).resolves.toMatchObject({
      ok: false,
      code: "COVEN_UNSUPPORTED_API_VERSION",
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
    "sessions",
    false,
    "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
  ],
  [
    "events",
    false,
    "expected capabilities.events to be true; upgrade Coven to a compatible version",
  ],
  [
    "eventCursor",
    "offset",
    "expected capabilities.eventCursor to be sequence; upgrade Coven to a compatible version",
  ],
  [
    "structuredErrors",
    false,
    "expected capabilities.structuredErrors to be true; upgrade Coven to a compatible version",
  ],
] as const)(
  "stops before launch when required health capability %s is unsupported",
  async (field, value, detail) => {
    const launchSession = vi.fn(async () => session());
    const health = await fakeClient().health();
    const runtime = new CovenAcpRuntime({
      config,
      client: fakeClient({
        health: vi.fn(async () => ({
          ...health,
          capabilities: { ...(health.capabilities ?? {}), [field]: value },
        })),
        launchSession,
      }),
    });

    await expect(runtime.doctor()).resolves.toMatchObject({
      ok: false,
      code: "COVEN_UNSUPPORTED_CAPABILITY",
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
    "capabilities",
    undefined,
    "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
  ],
  [
    "capabilities.sessions",
    { events: true, eventCursor: "sequence", structuredErrors: true },
    "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
  ],
  [
    "capabilities.events",
    { sessions: true, eventCursor: "sequence", structuredErrors: true },
    "expected capabilities.events to be true; upgrade Coven to a compatible version",
  ],
  [
    "capabilities.eventCursor",
    { sessions: true, events: true, eventCursor: false, structuredErrors: true },
    "expected capabilities.eventCursor to be sequence; upgrade Coven to a compatible version",
  ],
  [
    "capabilities.structuredErrors",
    { sessions: true, events: true, eventCursor: "sequence", structuredErrors: "yes" },
    "expected capabilities.structuredErrors to be true; upgrade Coven to a compatible version",
  ],
])(
  "reports malformed health field %s before launch",
  async (_field, capabilities, expectedDetail) => {
  const launchSession = vi.fn(async () => session());
  const runtime = new CovenAcpRuntime({
    config,
    client: fakeClient({
      health: vi.fn(async () => ({
        apiVersion: "coven.daemon.v1",
        covenVersion: "0.0.0",
        capabilities,
        ok: true,
        daemon: null,
      })),
      launchSession,
    }),
  });

  await expect(runtime.doctor()).resolves.toMatchObject({
    ok: false,
    code: "COVEN_UNSUPPORTED_CAPABILITY",
  });
  await expect(
    runtime.ensureSession({
      sessionKey: "agent:codex:test",
      agent: "codex",
      mode: "oneshot",
      cwd: workspaceDir,
    }),
  ).rejects.toThrow(expectedDetail);
  expect(launchSession).not.toHaveBeenCalled();
  },
);

it("rechecks compatibility immediately before dependent launch", async () => {
  const compatible = await fakeClient().health();
  const health = vi
    .fn()
    .mockResolvedValueOnce(compatible)
    .mockResolvedValueOnce({
      ...compatible,
      capabilities: { ...(compatible.capabilities ?? {}), sessions: false },
    });
  const launchSession = vi.fn(async () => session());
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
    collect(runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-drift" })),
  ).rejects.toThrow(
    /capabilities\.sessions.*upgrade Coven to a compatible version/,
  );
  expect(health).toHaveBeenCalledTimes(2);
  expect(launchSession).not.toHaveBeenCalled();
});
```

- [ ] **Step 3: Run the focused TypeScript tests and confirm failure**

Run:

```bash
npm --prefix packages/openclaw-coven test -- src/client.test.ts src/runtime.test.ts
```

Expected: failures show the current client casts capabilities to trusted types,
the runtime still expects route token `v1`, or unsupported capabilities are not
reported before session launch.

- [ ] **Step 4: Parse the health envelope without trusting compatibility fields**

Return an explicitly built object instead of `record as CovenHealthResponse`:

```typescript
function normalizeHealthResponse(value: unknown): CovenHealthResponse {
  const record = requireRecord(value, "Coven health");
  const rawCapabilities = isJsonRecord(record.capabilities)
    ? record.capabilities
    : undefined;
  return {
    apiVersion: record.apiVersion,
    covenVersion: record.covenVersion ?? record.coven_version,
    capabilities: rawCapabilities
      ? {
          sessions: rawCapabilities.sessions,
          events: rawCapabilities.events,
          eventCursor: rawCapabilities.eventCursor ?? rawCapabilities.event_cursor,
          structuredErrors:
            rawCapabilities.structuredErrors ?? rawCapabilities.structured_errors,
        }
      : undefined,
    ok: record.ok,
    daemon: record.daemon,
  };
}
```

Do not query or parse `supportedApiVersions`; it is not part of the current health shape.
Add this type guard beside `requireRecord`:

```typescript
function isJsonRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
```

Remove the now-unused `COVEN_API_CONTRACT_VERSION` constant from `client.ts`.
Compatibility policy belongs in the runtime; this normalization layer only
preserves bounded wire values for that policy to validate.

Update existing health assertions to use optional access, for example:

```typescript
expect(health.capabilities?.structuredErrors).toBe(true);
```

- [ ] **Step 5: Implement one fail-closed runtime compatibility check**

Replace `SUPPORTED_COVEN_API_VERSION` and `unsupportedApiVersionDetail` with:

```typescript
const SUPPORTED_COVEN_API_CONTRACT = "coven.daemon.v1";

type HealthCompatibilityError = {
  code: "COVEN_UNSUPPORTED_API_VERSION" | "COVEN_UNSUPPORTED_CAPABILITY";
  detail: string;
};

function healthCompatibilityError(
  health: CovenHealthResponse,
): HealthCompatibilityError | null {
  if (health.apiVersion !== SUPPORTED_COVEN_API_CONTRACT) {
    const actual =
      typeof health.apiVersion === "string" && health.apiVersion ? health.apiVersion : "missing";
    return {
      code: "COVEN_UNSUPPORTED_API_VERSION",
      detail:
        `expected apiVersion ${SUPPORTED_COVEN_API_CONTRACT}, got ${actual}; ` +
        "upgrade Coven to a compatible version",
    };
  }
  if (health.capabilities?.sessions !== true) {
    return {
      code: "COVEN_UNSUPPORTED_CAPABILITY",
      detail:
        "expected capabilities.sessions to be true; upgrade Coven to a compatible version",
    };
  }
  if (health.capabilities.events !== true) {
    return {
      code: "COVEN_UNSUPPORTED_CAPABILITY",
      detail:
        "expected capabilities.events to be true; upgrade Coven to a compatible version",
    };
  }
  if (health.capabilities.eventCursor !== "sequence") {
    return {
      code: "COVEN_UNSUPPORTED_CAPABILITY",
      detail:
        "expected capabilities.eventCursor to be sequence; upgrade Coven to a compatible version",
    };
  }
  if (health.capabilities.structuredErrors !== true) {
    return {
      code: "COVEN_UNSUPPORTED_CAPABILITY",
      detail:
        "expected capabilities.structuredErrors to be true; upgrade Coven to a compatible version",
    };
  }
  return null;
}
```

Use this helper in `doctor()`. In `doctor()`, return:

```typescript
const compatibilityError = healthCompatibilityError(health);
if (compatibilityError) {
  return {
    ok: false,
    code: compatibilityError.code,
    message:
      compatibilityError.code === "COVEN_UNSUPPORTED_API_VERSION"
        ? "Coven daemon API version is not supported."
        : "Coven daemon capability is not supported.",
    details: [compatibilityError.detail],
  };
}
```

Replace the boolean-only availability path with a method that preserves the
specific incompatibility:

```typescript
private async requireCovenCompatibility(signal?: AbortSignal): Promise<void> {
  const health = await this.client.health(signal);
  const compatibilityError = healthCompatibilityError(health);
  if (compatibilityError) {
    throw new AcpRuntimeError(
      "ACP_BACKEND_UNAVAILABLE",
      `Coven compatibility check failed: ${compatibilityError.detail}`,
    );
  }
  if (health.ok !== true) {
    throw new AcpRuntimeError(
      "ACP_BACKEND_UNAVAILABLE",
      "Coven daemon did not report healthy.",
    );
  }
}
```

Replace `ensureSession()`'s boolean availability branch with:

```typescript
try {
  await this.requireCovenCompatibility();
} catch (error) {
  if (!this.config.allowFallback) {
    throw error;
  }
  this.logger?.warn(
    `coven compatibility check failed; falling back to ${this.config.fallbackBackend}: ` +
      sanitizeErrorText(error),
  );
  return await this.ensureFallbackSession(input);
}
```

Call `await this.requireCovenCompatibility(input.signal)` again inside
`runTurn()`'s launch `try` block, immediately before bounding the prompt and
calling `client.launchSession(...)`. This closes the time-of-check/time-of-use
gap and preserves the exact capability/remediation detail in the existing
sanitized launch-failure path. Remove `isCovenAvailable()` entirely. The
healthy branch in `doctor()` must require `health.ok === true`.

- [ ] **Step 6: Run the focused TypeScript tests**

Run:

```bash
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test -- src/client.test.ts src/runtime.test.ts
```

Expected: all client and runtime tests pass, including named-contract mismatch and every required-capability mismatch before launch.

- [ ] **Step 7: Commit the client handshake migration**

```bash
git add packages/openclaw-coven/src/client.ts packages/openclaw-coven/src/client.test.ts packages/openclaw-coven/src/runtime.ts packages/openclaw-coven/src/runtime.test.ts
git commit -m "fix(openclaw): negotiate named Coven contract"
```

### Task 3: Interpret harness-session lifecycle explicitly

**Files:**
- Modify: `packages/openclaw-coven/src/runtime.ts:266-278`
- Test: `packages/openclaw-coven/src/runtime.test.ts`

- [ ] **Step 1: Add failing tests for `idle`, `active`, and unknown statuses**

Add:

```typescript
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

it.each([
  ["completed", "completed"],
  ["failed", "error"],
  ["killed", "cancelled"],
  ["orphaned", "error"],
])("maps terminal session status %s to %s", async (status, stopReason) => {
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
    runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-terminal" }),
  );

  expect(events.at(-1)).toEqual({ type: "done", stopReason });
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
      runtime.runTurn({ handle, text: "Fix tests", mode: "prompt", requestId: "req-unknown" }),
    );

    expect(events.at(-1)).toEqual({ type: "done", stopReason: "error" });
  },
);
```

- [ ] **Step 2: Run the lifecycle tests and confirm failure**

Run:

```bash
npm --prefix packages/openclaw-coven test -- src/runtime.test.ts -t \
  "idle|terminal session status|unsupported harness-session"
```

Expected: `idle` is currently treated as terminal completion, and the unknown status is also inferred terminal.

- [ ] **Step 3: Replace negative terminal inference with an exhaustive disposition**

Replace `sessionIsTerminal` with:

```typescript
type SessionDisposition = "nonterminal" | "terminal";

function sessionDisposition(status: string): SessionDisposition {
  switch (status) {
    case "created":
    case "running":
    case "idle":
      return "nonterminal";
    case "completed":
    case "failed":
    case "killed":
    case "orphaned":
      return "terminal";
    default:
      throw new Error(`Coven daemon returned unsupported session status: ${status}`);
  }
}
```

Change the poll condition to:

```typescript
if (sessionDisposition(latest.status) === "terminal") {
```

Also extend `normalizeStopReason` so persisted terminal results map explicitly:

```typescript
completed -> completed
failed -> error
killed -> cancelled
orphaned -> error
```

The `orphaned` value is unresolved ownership evidence and must never be reported
as successful completion. Do not describe `killed` as process-exit
acknowledgement. These helpers classify only the current ledger status; O5 owns
cancellation acknowledgement.

- [ ] **Step 4: Run the focused runtime tests**

Run:

```bash
npm --prefix packages/openclaw-coven test -- src/runtime.test.ts
```

Expected: all runtime tests pass; `idle` continues polling, unknown values end
in the existing sanitized polling-error path, `orphaned` reports an error, and
`killed` reports cancellation without claiming process-exit acknowledgement.

- [ ] **Step 5: Commit lifecycle interpretation**

```bash
git add packages/openclaw-coven/src/runtime.ts packages/openclaw-coven/src/runtime.test.ts
git commit -m "fix(openclaw): interpret Coven session states explicitly"
```

### Task 4: Correct canonical contract and lifecycle documentation

**Files:**
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/API.md`
- Modify: `docs/reference/api-contract.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/daemon/socket-api.md`
- Modify: `docs/daemon/capabilities-handshake.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/SESSION-LIFECYCLE.md`
- Modify: `docs/sessions/lifecycle.md`

- [ ] **Step 1: Make health the only recommended compatibility handshake**

Use this normative text in both API contract documents:

```markdown
Clients negotiate compatibility with `GET /api/v1/health`. Its `apiVersion`
field is the named contract `coven.daemon.v1`; clients must then check every
capability required by the operation before sending a dependent request.
Capabilities advertise availability and never grant permission.

`GET /api/v1/api-version` is a legacy route-family diagnostic. Its existing
`apiVersion: "v1"` and `supportedApiVersions: ["v1"]` values identify the
`/api/v1/*` route namespace, not the named compatibility contract. Existing
values remain wire-compatible, but new clients must not use this response as
proof of `coven.daemon.v1` support.
```

Remove examples that show named `coven.daemon.v1` values coming from
`/api-version`, and remove claims that `supportedApiVersions` is present in the
health response. Apply the same health-first rule and
capability-availability-versus-authorization sentence to `docs/API.md`,
`docs/reference/api.md`, `docs/daemon/socket-api.md`,
`docs/daemon/capabilities-handshake.md`, and `docs/ARCHITECTURE.md`.

- [ ] **Step 2: Correct the socket health example and endpoint index**

The health capability object in `docs/daemon/socket-api.md` must be:

```json
{
  "sessions": true,
  "events": true,
  "travel": true,
  "scheduler": true,
  "hub": true,
  "executorDispatch": true,
  "eventCursor": "sequence",
  "structuredErrors": true
}
```

Describe `/api-version` as `Read the legacy route-family token` in
`docs/daemon/socket-api.md` and `docs/reference/api.md`.

Change `docs/ARCHITECTURE.md` to:

```markdown
Clients should use `GET /api/v1/health` and its named `apiVersion` plus the
required `capabilities` fields before depending on session or event response
shapes.
```

- [ ] **Step 3: Publish the complete harness-session status table**

Add this table to `docs/API-CONTRACT.md`, `docs/SESSION-LIFECYCLE.md`, and
`docs/sessions/lifecycle.md`:

```markdown
| Status | Terminal in the current ledger | Meaning |
|---|---:|---|
| `created` | No | Durable row exists; no live runtime has been established. Recovery moves a stale unowned row to `failed`. |
| `running` | No | A daemon-owned or registered external runtime is live. |
| `idle` | No | A conversational turn completed and the session remains reusable. |
| `completed` | Yes | Runtime completion was successful. |
| `failed` | Yes | Launch or runtime completion failed. |
| `killed` | Yes | A kill request was accepted and persisted; this is not proof of acknowledged process termination. |
| `orphaned` | Yes | Recovery cannot prove ownership of a row previously marked running. |
```

State immediately after the table:

```markdown
Archive visibility is stored separately in `archived_at` and does not change
the lifecycle status. Synthetic Cast quest-anchor rows may use `active`; that
store value is not a harness-session state and must be classified by row kind
before interpreting status.
```

Update lifecycle diagrams to include `running -> idle` and `running -> killed`.
Omit archive and summon from lifecycle-state transitions, or show them only as
an independent `archived_at` visibility overlay applicable to every non-running
status, including `created` and `idle`. Do not rename `created` to `pending`,
and do not describe `killed` as termination acknowledgement.

- [ ] **Step 4: Inspect the documentation diff**

Run:

```bash
git diff --check
git diff -- docs/API-CONTRACT.md docs/API.md docs/reference/api-contract.md docs/reference/api.md docs/daemon/socket-api.md docs/daemon/capabilities-handshake.md docs/ARCHITECTURE.md docs/SESSION-LIFECYCLE.md docs/sessions/lifecycle.md
```

Expected: no whitespace errors; every handshake uses health; the legacy route
still documents literal `v1`; all lifecycle references include `idle`,
`killed`, and `orphaned`, with archive separate.

- [ ] **Step 5: Commit canonical documentation**

```bash
git add docs/API-CONTRACT.md docs/API.md docs/reference/api-contract.md docs/reference/api.md docs/daemon/socket-api.md docs/daemon/capabilities-handshake.md docs/ARCHITECTURE.md docs/SESSION-LIFECYCLE.md docs/sessions/lifecycle.md
git commit -m "docs(api): clarify contract and session lifecycle"
```

### Task 5: Add deterministic documentation drift checks

**Files:**
- Create: `scripts/check-api-contract-docs.py`
- Create: `scripts/check-api-contract-docs-test.py`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Write failing guardrail tests**

Build focused stdlib `unittest` fixtures around `validate_documents`. Tests must
prove the guard accepts the canonical corpus and rejects each independent drift:

- any contract guide fails to tie `/api/v1/health` to `coven.daemon.v1` as the
  `apiVersion`/named-contract compatibility handshake in the same paragraph or
  Markdown table row;
- `/api/v1/api-version` is presented as the named-contract handshake, including
  affirmative text followed by an unrelated or later denial;
- capabilities are described without the authorization boundary;
- any lifecycle status is missing or has the wrong terminal classification;
- stale, unowned `created` recovery is not mapped unambiguously to `failed` in
  the same sentence or Markdown table row, or that unit names a competing
  recovery target;
- `killed` is described as acknowledged termination;
- synthetic `active` is presented as a harness-session status; or
- archive visibility is not kept separate in `archived_at`.

Do not weaken a negative fixture by satisfying its requirement elsewhere in the
same document. Keep tests behavioral rather than embedding a full final script
snapshot.

- [ ] **Step 2: Run the guardrail tests and confirm failure**

```bash
python3 scripts/check-api-contract-docs-test.py
```

Expected: the new behavioral tests fail because the guard is absent or does not
yet enforce every relationship above.

- [ ] **Step 3: Implement the stdlib guardrail**

Implement `validate_documents(documents) -> list[str]` and a repository-reading
`main()` using only Python's standard library. The guard must cover every
contract and lifecycle guide listed by the tests, validate relationships within
the relevant paragraph/table row rather than by loose whole-file keyword
presence, report path-specific diagnostics, and exit nonzero for any error.
Keep the implementation synchronized with the committed guard tests; this plan
intentionally does not duplicate the script as an exact snapshot.

- [ ] **Step 4: Run unit and repository guardrail checks**

```bash
python3 scripts/check-api-contract-docs-test.py
python3 scripts/check-api-contract-docs.py
```

Expected: both commands exit 0 without reducing any negative coverage.

- [ ] **Step 5: Add the guardrail to the existing Python CI job**

Add both commands to the existing `secret-guard` job after its current Python
unit checks. Preserve the workflow's current checkout and setup-python action
versions. The OpenClaw job remains scoped to its package typecheck and tests.

- [ ] **Step 6: Commit the documentation guardrail**

```bash
git add scripts/check-api-contract-docs.py scripts/check-api-contract-docs-test.py .github/workflows/ci.yml
git commit -m "test(docs): guard Coven contract vocabulary"
```

### Task 6: Run the O1 release gate and prepare merge evidence

**Files:**
- Modify: `specs/psyche/O1_CONTRACT_DESIGN.md`
- Modify: `specs/psyche/PLAN.md`

- [ ] **Step 1: Run focused O1 verification**

```bash
cargo test -p coven-cli api::tests::legacy_api_version_route_remains_a_route_token_diagnostic
cargo test -p coven-cli api::tests::health_is_the_named_contract_handshake
cargo test -p coven-cli api::tests::rejects_unknown_api_version_prefixes
cargo test -p coven-cli api::tests::unknown_harness_capability_manifest_fails_closed_with_structured_error
cargo test -p coven-cli daemon::tests::exit_event_does_not_overwrite_killed_session_status
cargo test -p coven-cli daemon::tests::clean_exit_on_conversational_session_persists_as_idle
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test -- src/client.test.ts src/runtime.test.ts
python3 scripts/check-api-contract-docs-test.py
python3 scripts/check-api-contract-docs.py
```

Expected: every command exits 0.

- [ ] **Step 2: Run repository-required verification**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
npm --prefix packages/openclaw-coven test
```

Expected: every command exits 0.

- [ ] **Step 3: Record bounded pre-merge implementation status**

Change the O1 design status to:

```markdown
**Status:** Implementation verified; delivery evidence is tracked in issue #567 and Bead `coven-psy-o1`
```

Add an O1 evidence line to `specs/psyche/PLAN.md`:

```markdown
**O1 implementation candidate:** Named-contract negotiation and lifecycle
vocabulary pass focused Rust, TypeScript, and documentation guardrail tests.
O1 remains incomplete until the reviewed PR merges and issue #567 plus Bead
`coven-psy-o1` record the merge evidence. This candidate addresses only C-S1
vocabulary and C-S8 documentation; C-S3-C-S6 and C-S9-C-S12 remain planned,
and G4/G6 remain blocked.
```

- [ ] **Step 4: Stage only O1 paths and run the privacy guard**

```bash
git add crates/coven-cli/src/api.rs \
  crates/coven-cli/src/main.rs \
  crates/coven-cli/src/pty_runner.rs \
  crates/coven-cli/src/store.rs \
  crates/coven-cli/tests/stream_json_integration.rs \
  .github/workflows/ci.yml \
  packages/openclaw-coven/package.json \
  packages/openclaw-coven/package-lock.json \
  packages/openclaw-coven/src/client.ts \
  packages/openclaw-coven/src/client.test.ts \
  packages/openclaw-coven/src/runtime.ts \
  packages/openclaw-coven/src/runtime.test.ts \
  docs/API-CONTRACT.md \
  docs/API.md \
  docs/reference/api-contract.md \
  docs/reference/api.md \
  docs/reference/cli-run.md \
  docs/daemon/socket-api.md \
  docs/daemon/capabilities-handshake.md \
  docs/ARCHITECTURE.md \
  docs/SESSION-LIFECYCLE.md \
  docs/sessions/lifecycle.md \
  scripts/check-api-contract-docs.py \
  scripts/check-api-contract-docs-test.py \
  specs/psyche/O1_CONTRACT_DESIGN.md \
  specs/psyche/PLAN.md \
  docs/superpowers/plans/2026-08-03-psyche-o1-contract-implementation.md
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: the privacy guard and staged diff check exit 0, with no O2-O8 fields,
routes, migrations, or behavior in the staged diff.

- [ ] **Step 5: Commit the completion evidence**

```bash
git commit -m "docs(psyche): record O1 contract evidence"
```

- [ ] **Step 6: Run whole-branch pre-PR guards**

```bash
python3 scripts/check-coven-privacy.py --range origin/main...HEAD
git diff --check origin/main...HEAD
git status --short
```

Expected: the whole O1 branch passes the privacy and whitespace guards, and
the worktree is clean. This range check covers earlier implementation commits;
the staged check in Step 4 covers the final evidence commit before it is made.

- [ ] **Step 7: Request final review before merge**

Review the branch diff against the O1 design and confirm:

1. `/api-version` still returns the exact legacy `v1` values.
2. health is the only recommended named-contract handshake.
3. the OpenClaw bridge checks required capabilities before session launch.
4. discovery is never represented as authorization.
5. `idle` is nonterminal for one-shot work.
6. `killed` is not represented as acknowledged process termination.
7. `active` is scoped to synthetic rows.
8. no O2-O8 contract or production child-dispatch behavior was added.

- [ ] **Step 8: Record completion only after the reviewed PR merges**

After merge, update GitHub issue #567 and Bead `coven-psy-o1` with:

Record the observed merge commit SHA together with this exact evidence
statement: focused Rust API/lifecycle tests, OpenClaw bridge typecheck/Vitest,
documentation contract guardrail, full Rust workspace checks, secret scan, and
staged plus whole-branch privacy guards passed in the merged PR. Scope closed
is C-S1 vocabulary and C-S8 documentation only. C-S3-C-S6, C-S9-C-S12, G4,
G6, and production child dispatch remain blocked. Close issue #567 and mark
Bead `coven-psy-o1` complete only after both trackers contain that observed
merge evidence. Do not substitute a branch HEAD, proposed PR number, or
expected commit for the merge commit reported by GitHub.
