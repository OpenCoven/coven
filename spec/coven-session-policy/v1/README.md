# Coven session-policy admission v1

Contract: `coven.session-policy.v1`.
Owner: Coven Rust. Consumer implementations: OpenCoven SDK and Wand.
Decision authority: BunsDev, through the September 10, 2026 explicit delegation
of cross-repository design and implementation to Cody. Tracking:
OpenCoven/coven#990, OpenCoven/sdk#199, and OpenCoven/wand#9 / OpenCoven/wand#10.

## Scope and status

This version establishes strict discovery and a separate restricted-launch
admission boundary. It does **not** certify a running harness as constrained.
At the initial implementation revision all production harnesses report
`enforcement: "unavailable"` and restricted launch is refused before store
creation, familiar resolution or runtime launch.

The positive enforcement adapter is a separate unmet requirement, not a fake
implementation hidden behind a successful response. Owner-local Codex
workspace-write arguments and automation-authority projections do not satisfy
this contract's restrictive profile. Existing ordinary-chat and automation
authority ownership are unchanged.

## Discovery

`GET /api/v1/session-policy` returns:

```json
{
  "contract": "coven.session-policy.v1",
  "enforcement": "unavailable",
  "supportedProfiles": [],
  "reason": "no_verified_enforcement_backend"
}
```

The existing health capability object adds `sessionPolicyContracts`, containing
this contract only on owner-local IPC; TCP advertises an empty array. Discovery
is availability metadata, never permission or proof of daemon identity.
Existing health and legacy launch clients remain compatible.

## Response size boundary

Every response body from `GET /api/v1/session-policy` and
`POST /api/v1/sessions/restricted` is at most **16,384 UTF-8 bytes**, including
JSON whitespace. This limit covers discovery, correlated refusal, and structured
error responses, regardless of HTTP status. Consumers must enforce it while
reading and before JSON parsing, rejecting oversized or invalid-UTF-8 responses
rather than truncating them or relying only on a declared `Content-Length`.
The closed discovery and refusal schemas are intrinsically much smaller.

This bound does not change the existing health response contract. Health
advertisement and discovery remain metadata, and a refusal remains correlation;
none is an accepted grant or evidence of positive runtime enforcement. A response
rejected by the consumer for size or encoding cannot justify `not_started`,
legacy fallback, or automatic retry.

## Restricted launch request

The only entry point is `POST /api/v1/sessions/restricted`. There is no fallback
to `POST /api/v1/sessions`. The restricted route requires owner-local IPC,
independently of the JSON contents. A top-level `sessionPolicy` key on the
legacy launch route is rejected before launch, rather than ignored.

The request is a closed JSON object:

```json
{
  "contract": "coven.session-policy.v1",
  "requestId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  "invocationId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
  "profile": "workspace-readonly-no-network.v1",
  "expiresAtUnixMs": 1800000000000,
  "launch": {
    "projectRoot": "/example/project",
    "cwd": "/example/project",
    "harness": "codex",
    "familiarId": "sage",
    "launchMode": "nonInteractive",
    "prompt": "Review the supplied material.",
    "title": "Wand review"
  }
}
```

`requestId` and `invocationId` are lowercase, canonical hyphenated UUID strings.
`expiresAtUnixMs` is an integer in the JavaScript safe-integer range. It must
be strictly after server admission time and at most 300,000 ms after that time.
It is a request admission deadline, **not an implemented running-process lease**.

The complete request is at most 1,048,576 UTF-8 bytes and JSON depth is at most
16. Reject duplicate keys, invalid Unicode, nonobjects, unknown keys, wrong
types and unknown required contract/profile values. `launch` has exactly the
seven legacy fields shown. Strings must not contain NUL. Limits in UTF-8 bytes:
projectRoot/cwd 4,096 each; familiarId/harness 128 each; prompt 1,000,000; title
512. All are nonempty except title. Familiar IDs have no surrounding whitespace.
`launchMode` must be `nonInteractive`; the current bundled API harness IDs are
`codex`, `claude`, `coven-code`, and `copilot`, taken from Coven's existing pure
bundled registry. Configured external adapters are not scanned at this boundary.
Paths remain opaque request values at the refusal boundary: they are
not resolved or granted merely because this envelope parses.

The profile requests reads confined to the canonical project root, no
filesystem writes, no child processes after the initial harness admission, and
no network connections. No inference/provider exception is implied. A future
backend must prove those semantics and operation-time target identity before
it can support the profile.

## Refusal and binding

Successful parsing of a currently unsupported restricted request returns HTTP
409, not 200 or 201, with this closed response:

```json
{
  "contract": "coven.session-policy.v1",
  "requestId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  "invocationId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
  "requestDigest": "sha256:HEX",
  "decision": "rejected",
  "code": "enforcement_unavailable",
  "admission": "not_started"
}
```

`requestDigest` is lowercase SHA-256 of the **exact transmitted request-body
bytes**, prefixed with `sha256:`. Serialize/freeze once and send those bytes;
do not reserialize after binding. This is not semantic JSON canonicalization.
Whitespace and Unicode byte changes intentionally change the digest.

The digest and echoed IDs correlate a refusal; they are not a signature,
authentication, a grant, or evidence that any other process is stopped. The
server can assert `not_started` here because this route has no admission path
to a store or runtime. Clients must not infer that property from arbitrary
HTTP errors, malformed replies, cancellation, timeout or connection loss.

Unknown contract/profile, malformed request, expired request and non-owner
transport use existing structured HTTP errors (400, 409 or 403 as appropriate).
Malformed/unsupported requests use `400 invalid_request`, elapsed deadlines use
`409 session_policy_expired`, and non-owner transports use `403 forbidden`.
Transport authority is checked before reading or parsing the body. On owner-local
IPC the complete shape and field constraints precede deadline evaluation.
Safe integers include negative values; these are already expired at present-day
admission times. A future deadline exceeding the 300,000-ms window is invalid.
They cannot cause legacy fallback or automatic POST retries. No accepted
response, session ID, receipt, effective grant, revocation event or mutation
permission exists in this initial version. Consumers reject fabricated
`accepted` responses rather than coercing them into success.

## Activation requirements

Production support remains unavailable until an actual backend has:

1. Effect enforcement installed before the first harness instruction.
2. Defined filesystem, process, network, target-identity and lifetime semantics
   covering descendants, inherited handles and direct tools.
3. An owner-approved exact daemon/harness/adapter/OS/backend/configuration tuple
   and independently reviewable positive and negative conformance evidence.
4. A separately reviewed accepted-response and lifecycle contract, including
   request/authority bindings, expiry, revocation and uncertainty.

Adding an `accepted` response or advertising a supported profile is therefore
not an additive metadata change. It requires contract review and a new
explicitly negotiated revision. This refusal-only version must never become
a metadata-based grant factory.

## Shared fixtures

[`fixtures/manifest.json`](fixtures/manifest.json) pins the admission time to
`1799999700000`, exactly 300,000 ms before the request's `expiresAtUnixMs`.
The request, refusal, and discovery files are UTF-8 JSON with a single trailing
LF. File digests include that LF. Send `request.json` bytes unchanged: its exact
421 bytes hash to
`sha256:c61bac56f84d1f3fddd5e70da35a7e8906f9bd4bac769b5d4ad61769dae83fdb`.
This is a byte-exact shared sample, not a semantic JSON canonicalization rule.
Tests inject the manifest admission time; these fixtures are not live launch
instructions or evidence of enforcement.

The server emits compact refusal/discovery response JSON without a trailing
newline; their fixture files add one LF for source-file hygiene. JSON consumers
may accept either response whitespace form, but must never strip or normalize
request bytes when checking `requestDigest`.
