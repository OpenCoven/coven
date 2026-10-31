// Real CLI/daemon acceptance with synthetic trusted-device and WebAuthn keys.
// The initial registry is a fixture, not proof of physical-device enrollment.
// Requires a usable private-interface listener; a networking failure is fatal.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import https from "node:https";
import http from "node:http";
import { execFileSync, spawnSync } from "node:child_process";
import assert from "node:assert/strict";
assert(process.argv[2], "binary path required");
const binary = path.resolve(process.argv[2]);
// Keep the owner Unix socket path short on macOS.
const scratch = fs.mkdtempSync("/tmp/c1556-e2e-");
const home = path.join(scratch, "home");
fs.mkdirSync(home, { mode: 0o700 });
const env = { ...process.env, COVEN_HOME: home, NO_COLOR: "1" };
const checks = [];
const check = (name) => {
  checks.push(name);
  console.log("PASS " + name);
};
const run = (args, input) =>
  execFileSync(binary, args, { env, input, encoding: "utf8", timeout: 45000 });
const b64 = (b) => Buffer.from(b).toString("base64url");
const sha = (b) => crypto.createHash("sha256").update(b).digest();
const jcs = (v) => JSON.stringify(sort(v));
function sort(v) {
  if (Array.isArray(v)) return v.map(sort);
  if (v && typeof v === "object")
    return Object.fromEntries(
      Object.keys(v)
        .sort()
        .map((k) => [k, sort(v[k])]),
    );
  return v;
}
const key = () =>
  crypto.generateKeyPairSync("ec", { namedCurve: "prime256v1" }).privateKey;
const pub = (k) => {
  let j = crypto.createPublicKey(k).export({ format: "jwk" });
  return b64(
    Buffer.concat([
      Buffer.from([4]),
      Buffer.from(j.x, "base64url"),
      Buffer.from(j.y, "base64url"),
    ]),
  );
};
const sign = (k, b) => b64(crypto.sign("sha256", b, k));
const fields = (domain, values) =>
  Buffer.concat([
    Buffer.from(domain),
    ...values.flatMap((v) => {
      const b = Buffer.isBuffer(v) ? v : Buffer.from(v);
      const n = Buffer.alloc(4);
      n.writeUInt32BE(b.length);
      return [n, b];
    }),
  ]);
const actionBytes = (a) =>
  fields("COVEN-ACTION/1\0", [
    a.scope,
    a.operation,
    a.target,
    a.effectDigest,
    a.nonce,
    new Date(a.issuedAt).toISOString(),
    new Date(a.expiresAt).toISOString(),
  ]);
const write = (p, x) => fs.writeFileSync(p, JSON.stringify(x), { mode: 0o600 });
// Use a nonzero millisecond fraction for stable JCS timestamps, in the
// preceding second so a fast local request is never issued in the future.
const iso = (ms = Date.now()) =>
  new Date(Math.floor(ms / 1000) * 1000 - 1000 + 123).toISOString();
const now = iso(Date.now() - 1000),
  until = iso(Date.now() + 86400000);
const grant = (id, k, scopes, issued = now) => ({
  version: 1,
  id: crypto.randomUUID(),
  subjectKeyId: b64(sha(Buffer.from(pub(k), "base64url"))),
  audience: "local_coven_authority",
  scopes,
  restrictions: {
    transport: "any_authenticated",
    requireFreshUserVerificationFor: [],
  },
  minimumAssurance: "possession",
  issuedAt: issued,
  notBefore: issued,
  expiresAt: until,
  revocationEpoch: 0,
});
const devices = [0, 1]
  .map((i) => {
    const id = crypto.randomUUID(),
      possession = key(),
      step = key();
    return {
      id,
      possession,
      step,
      grant: grant(id, possession, ["memory_read", "device_admin"]),
    };
  })
  .sort((a, b) => a.id.localeCompare(b.id));
const ip = Object.values(os.networkInterfaces())
  .flat()
  .find(
    (x) =>
      x?.family === "IPv4" &&
      !x.internal &&
      /^(10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)/.test(x.address),
  )?.address;
assert(ip, "private IPv4 interface required for pinned TLS run");
const port = 19000 + crypto.randomInt(10000);
let daemon = false;
let completed = false;
try {
  run([
    "memory",
    "mobile",
    "enable",
    "--bind",
    `${ip}:${port}`,
    "--endpoint",
    `https://${ip}:${port}`,
  ]);
  const mobile = path.join(home, "mobile");
  write(path.join(mobile, "devices.json"), {
    version: 2,
    devices: devices.map((d) => ({
      device: {
        id: d.id,
        displayName: "Synthetic E2E trusted device",
        publicKeyX963: pub(d.possession),
        pairedAt: now,
        revokedAt: null,
        scopes: d.grant.scopes,
      },
      grant: d.grant,
    })),
  });
  write(path.join(mobile, "authorization-keys.json"), {
    version: 1,
    keys: devices.map((d) => ({
      deviceId: d.id,
      publicKeyX963: pub(d.step),
      subjectKeyId: b64(sha(Buffer.from(pub(d.step), "base64url"))),
      assuranceClass: "biometric_only",
      enrolledAt: now,
      revokedAt: null,
      keyEpoch: 1,
    })),
  });
  const rootPath = path.join(scratch, "root.pem");
  run(["device", "owner-root", "generate", "--out", rootPath]);
  run([
    "device",
    "introduction-policy",
    "install",
    "--root-key",
    rootPath,
    ...devices.flatMap((d) => ["--introducer", d.id]),
    "--required-approvals",
    "2",
    "--scope",
    "memory-read,device-admin",
    "--max-grant-days",
    "1",
    "--assurance",
    "fresh-biometric",
    "--max-step-up-seconds",
    "60",
    "--yes",
  ]);
  const installed = JSON.parse(
    run(["device", "introduction-policy", "show", "--json"]),
  );
  const policy = Object.fromEntries(
    [
      "eligibleIntroducers",
      "requiredApprovals",
      "allowedScopes",
      "maximumGrantLifetimeSeconds",
      "requiredAssurance",
      "maximumStepUpAgeSeconds",
    ].map((k) => [k, installed[k]]),
  );
  const host = JSON.parse(
    fs.readFileSync(path.join(mobile, "host-identity.json")),
  );
  const root = crypto.createPrivateKey(fs.readFileSync(rootPath));
  const credential = key(),
    credentialId = b64(crypto.randomBytes(32)),
    rp = "coven.example.org",
    origin = "https://coven.example.org";
  let counter = 0;
  function assertion(a) {
    const client = Buffer.from(
      JSON.stringify({
        type: "webauthn.get",
        challenge: b64(sha(actionBytes(a))),
        origin,
        crossOrigin: false,
      }),
    );
    const n = Buffer.alloc(4);
    n.writeUInt32BE(++counter);
    const auth = Buffer.concat([sha(rp), Buffer.from([0x1d]), n]);
    return {
      credentialId,
      authenticatorData: b64(auth),
      clientDataJson: b64(client),
      signature: sign(credential, Buffer.concat([auth, sha(client)])),
    };
  }
  const reg = {
    credentialId,
    rpId: rp,
    origins: [origin],
    publicKeySpki: b64(
      crypto
        .createPublicKey(credential)
        .export({ type: "spki", format: "der" }),
    ),
    nonce: [...crypto.randomBytes(32)],
    issuedAt: now,
    expiresAt: iso(Date.now() + 180000),
  };
  const material = {
    audience: host.publicKeyFingerprint,
    ownerRootKey: pub(root),
    ownerRootGeneration: installed.rootGeneration,
    credentialId,
    rpId: rp,
    origins: [origin],
    publicKeyX963: pub(credential),
  };
  const a = {
    version: 1,
    scope: "device_admin",
    operation: "owner.passkey.register",
    target: host.publicKeyFingerprint,
    effectDigest: b64(
      sha(
        Buffer.concat([
          Buffer.from("COVEN-PASSKEY/1\0"),
          Buffer.from(jcs(material)),
        ]),
      ),
    ),
    nonce: b64(reg.nonce),
    issuedAt: reg.issuedAt,
    expiresAt: reg.expiresAt,
  };
  const regPath = path.join(scratch, "registration.json"),
    assertPath = path.join(scratch, "assertion.json");
  write(regPath, reg);
  write(assertPath, assertion(a));
  const before = run(["device", "list", "--json"]);
  const cancel = spawnSync(
    binary,
    [
      "device",
      "passkey",
      "register",
      "--registration",
      regPath,
      "--assertion",
      assertPath,
      "--root-key",
      rootPath,
    ],
    { env, input: "cancel\n", encoding: "utf8", timeout: 45000 },
  );
  assert.notEqual(cancel.status, 0);
  assert(cancel.stderr.includes("cancelled"));
  check("typed cancellation does not register");
  const review = run(
    [
      "device",
      "passkey",
      "register",
      "--registration",
      regPath,
      "--assertion",
      assertPath,
      "--root-key",
      rootPath,
    ],
    "register\n",
  );
  for (const value of [
    pub(root),
    installed.rootGeneration,
    pub(credential),
    reg.issuedAt,
    reg.expiresAt,
    "device_admin",
  ])
    assert(review.includes(value), "registration review omitted " + value);
  check("registration review identifies both keys, scope and validity window");
  const listed = JSON.parse(run(["device", "passkey", "list", "--json"]));
  assert.equal(listed.length, 1);
  assert.equal(listed[0].backedUp, true);
  assert.equal(run(["device", "list", "--json"]), before);
  check("synced passkey registered without changing device list");
  run(["daemon", "start"]);
  daemon = true;
  function request(method, target, body, local = false) {
    return new Promise((resolve, reject) => {
      const bytes = Buffer.from(body === undefined ? "" : JSON.stringify(body));
      let headers = {
        "content-type": "application/json",
        "content-length": bytes.length,
        connection: "close",
      };
      if (!local) {
        const ts = Math.floor(Date.now() / 1000),
          nonce = b64(crypto.randomBytes(32)),
          digest = b64(sha(bytes));
        const canonical = Buffer.from(
          `COVEN-MEMORY/1\n${method}\n${target}\n${ts}\n${nonce}\n${digest}`,
        );
        Object.assign(headers, {
          "x-coven-protocol": "1",
          "x-coven-device": devices[0].id,
          "x-coven-timestamp": String(ts),
          "x-coven-nonce": nonce,
          "x-coven-body-sha256": digest,
          "x-coven-signature": sign(devices[0].possession, canonical),
        });
      }
      const options = local
        ? {
            socketPath: path.join(home, "coven.sock"),
            path: target,
            method,
            headers,
            agent: false,
          }
        : {
            host: ip,
            port,
            path: target,
            method,
            headers,
            ca: host.certificatePem,
            agent: false,
          };
      const req = (local ? http : https).request(options, (res) => {
        let text = "";
        res.on("data", (x) => (text += x));
        res.on("end", () => {
          let value;
          try {
            value = JSON.parse(text);
          } catch {
            value = text;
          }
          resolve({ status: res.statusCode, body: value });
        });
      });
      req.setTimeout(15000, () => req.destroy(new Error("request timed out")));
      req.on("error", reject);
      req.end(bytes);
    });
  }
  function makeIntroduction() {
    const destination = key(),
      id = crypto.randomUUID(),
      issued = iso(),
      expires = iso(Date.now() + 120000),
      g = grant(id, destination, ["memory_read"], issued);
    g.expiresAt = iso(Date.now() + 3600000);
    const req = {
      version: 2,
      audience: [...Buffer.from(host.publicKeyFingerprint, "base64url")],
      policyDigest: b64(sha(jcs(policy))),
      approvers: [
        {
          deviceId: devices[0].id,
          grantId: devices[0].grant.id,
          revocationEpoch: 0,
        },
      ],
      passkeyApprover: credentialId,
      destinationId: id,
      destinationPublicKey: pub(destination),
      deviceName: "Synthetic E2E destination",
      deviceContext: "Pinned TLS device plus synthetic synced passkey",
      grant: g,
      nonce: [...crypto.randomBytes(32)],
      issuedAt: issued,
      expiresAt: expires,
    };
    const act = {
      version: 1,
      scope: "device_admin",
      operation: "devices.introduction.approve",
      target: host.publicKeyFingerprint,
      effectDigest: b64(
        sha(
          Buffer.concat([
            Buffer.from("COVEN-INTRODUCTION/2\0"),
            Buffer.from(jcs(req)),
          ]),
        ),
      ),
      nonce: b64(req.nonce),
      issuedAt: issued,
      expiresAt: expires,
    };
    const envelope = {
      request: req,
      destinationSignature: sign(
        destination,
        actionBytes({ ...act, operation: "devices.introduction.accept" }),
      ),
      passkeyAssertion: assertion(act),
    };
    return { id, act, envelope };
  }
  const { id, act, envelope } = makeIntroduction();
  const admitted = await request(
    "POST",
    "/api/v1/internal/mobile/introductions",
    envelope,
    true,
  );
  assert.equal(admitted.status, 201, JSON.stringify(admitted));
  assert.equal(admitted.body.state, "awaiting_approval");
  assert.equal(run(["device", "list", "--json"]), before);
  check("passkey assertion alone leaves enrollment awaiting device approval");
  async function approveIntroduction(admitted, act) {
    const challenge = await request(
      "POST",
      "/api/v1/mobile/assurance/challenge",
      {},
    );
    assert.equal(challenge.status, 200, JSON.stringify(challenge));
    const received = Date.now();
    while (Date.now() <= received)
      await new Promise((resolve) => setTimeout(resolve, 1));
    const step = {
      challenge: challenge.body.challenge,
      issuedAt: new Date().toISOString(),
      expiresAt: new Date(
        Math.min(
          Date.parse(challenge.body.expiresAt),
          Date.parse(act.expiresAt),
        ),
      ).toISOString(),
      requestedAssurance: "fresh_biometric",
      signature: "",
    };
    const uuid = (x) => Buffer.from(x.replaceAll("-", ""), "hex");
    const epoch = Buffer.alloc(8);
    epoch.writeBigUInt64BE(0n);
    step.signature = sign(
      devices[0].step,
      fields("COVEN-ASSURANCE/1\0", [
        uuid(devices[0].id),
        uuid(devices[0].grant.id),
        epoch,
        b64(sha(Buffer.from(pub(devices[0].step), "base64url"))),
        "action",
        sha(actionBytes(act)),
        Buffer.from(step.challenge, "base64url"),
        new Date(step.issuedAt).toISOString(),
        step.expiresAt,
        "fresh_biometric",
      ]),
    );
    const approval = {
      possessionSignature: sign(devices[0].possession, actionBytes(act)),
      stepUp: step,
    };
    const committed = await request(
      "POST",
      `/api/v1/mobile/introductions/${admitted.body.id}/approve`,
      approval,
    );
    return { committed, approval };
  }
  const { committed, approval } = await approveIntroduction(admitted, act);
  assert.equal(committed.status, 200, JSON.stringify(committed));
  assert.equal(committed.body.state, "introduced");
  assert(run(["device", "list", "--json"]).includes(id));
  check("real daemon commits one trusted device plus passkey over pinned TLS");
  const replay = await request(
    "POST",
    `/api/v1/mobile/introductions/${admitted.body.id}/approve`,
    approval,
  );
  assert.notEqual(replay.status, 200);
  check("committed approval replay refused");
  const nextRoot = path.join(scratch, "next-root.pem");
  run(["device", "owner-root", "generate", "--out", nextRoot]);
  run(
    [
      "device",
      "owner-root",
      "rotate",
      "--root-key",
      rootPath,
      "--new-root-key",
      nextRoot,
    ],
    "rotate\n",
  );
  assert.equal(
    JSON.parse(run(["device", "passkey", "list", "--json"])).length,
    0,
  );
  check("root rotation retires passkey registration");
  const retired = makeIntroduction();
  const retiredAdmitted = await request(
    "POST",
    "/api/v1/internal/mobile/introductions",
    retired.envelope,
    true,
  );
  assert.equal(retiredAdmitted.status, 201, JSON.stringify(retiredAdmitted));
  const retiredAttempt = await approveIntroduction(
    retiredAdmitted,
    retired.act,
  );
  assert.equal(
    retiredAttempt.committed.status,
    409,
    JSON.stringify(retiredAttempt.committed),
  );
  assert(!run(["device", "list", "--json"]).includes(retired.id));
  check("retired passkey with fresh device approval enrolls no destination");

  const successor = crypto.createPrivateKey(fs.readFileSync(nextRoot));
  const renewed = {
    ...reg,
    nonce: [...crypto.randomBytes(32)],
    issuedAt: iso(Date.now() - 1000),
    expiresAt: iso(Date.now() + 180000),
  };
  const renewedAction = {
    ...a,
    nonce: b64(renewed.nonce),
    issuedAt: renewed.issuedAt,
    expiresAt: renewed.expiresAt,
    effectDigest: b64(
      sha(
        Buffer.concat([
          Buffer.from("COVEN-PASSKEY/1\0"),
          Buffer.from(
            jcs({
              ...material,
              ownerRootKey: pub(successor),
              ownerRootGeneration: JSON.parse(
                run(["device", "introduction-policy", "show", "--json"]),
              ).rootGeneration,
            }),
          ),
        ]),
      ),
    ),
  };
  write(regPath, renewed);
  write(assertPath, assertion(renewedAction));
  const beforeRenewal = run(["device", "list", "--json"]);
  run(
    [
      "device",
      "passkey",
      "register",
      "--registration",
      regPath,
      "--assertion",
      assertPath,
      "--root-key",
      nextRoot,
    ],
    "register\n",
  );
  assert.equal(run(["device", "list", "--json"]), beforeRenewal);
  const fresh = makeIntroduction();
  const freshAdmitted = await request(
    "POST",
    "/api/v1/internal/mobile/introductions",
    fresh.envelope,
    true,
  );
  assert.equal(freshAdmitted.status, 201, JSON.stringify(freshAdmitted));
  const freshAttempt = await approveIntroduction(freshAdmitted, fresh.act);
  assert.equal(
    freshAttempt.committed.status,
    200,
    JSON.stringify(freshAttempt.committed),
  );
  assert.equal(freshAttempt.committed.body.state, "introduced");
  assert(run(["device", "list", "--json"]).includes(fresh.id));
  check(
    "current-root re-registration restores the factor and a fresh introduction succeeds",
  );

  const audit = fs.readFileSync(path.join(mobile, "audit.jsonl"), "utf8");
  assert(audit.includes("passkey_registered"));
  assert(audit.includes("device_introduced"));
  assert(audit.includes("owner_root_rotated"));
  check("registration, introduction and rotation audited");
  const binaryDigest = crypto.createHash("sha256");
  for await (const chunk of fs.createReadStream(binary))
    binaryDigest.update(chunk);
  const receipt = {
    sourceCommit: process.env.GITHUB_SHA ?? null,
    binarySha256: binaryDigest.digest("hex"),
    setup:
      "synthetic trusted-device registry and step-up fixtures; real CLI and daemon; synthetic WebAuthn authenticator",
    checks,
  };
  if (process.env.COVEN_PASSKEY_RECEIPT)
    fs.writeFileSync(
      process.env.COVEN_PASSKEY_RECEIPT,
      JSON.stringify(receipt, null, 2) + "\n",
    );
  console.log("PASS real-daemon receipt " + JSON.stringify(receipt));
  completed = true;
} finally {
  if (daemon) {
    try {
      run(["daemon", "stop"]);
    } catch (e) {
      throw new Error("daemon cleanup failed: " + e.message, { cause: e });
    }
  }
  if (completed) fs.rmSync(scratch, { recursive: true });
  else console.error("Failed fixture retained at " + scratch);
}
