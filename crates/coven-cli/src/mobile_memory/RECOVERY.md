---
title: "Access recovery and identity rotation"
summary: "Which recovery and rotation operations exist, what each may change, and how each is authorized and audited."
read_when:
  - Recovering access after losing trusted devices
  - Changing owner-root, device or familiar identity
  - Reviewing whether an operation can mint device authority
source_adjacent_reason: "Defines the recovery invariants implemented beside recovery.rs, owner_policy.rs and introduction.rs and their fixtures."
---

# Access recovery and identity rotation

Tracks #788. The pairing protocol (`docs/architecture/mobile-device-pairing-v1.md`)
requires recovery to distinguish four operations. They are separate code
paths, separate signed transcripts and separate audit events. None of them
silently performs another.

| Operation | What changes | Authorized by | Audit event | Never changes |
|---|---|---|---|---|
| Enroll a device through a trusted device | adds one device and grant | N named introducers, each with fresh step-up | `device_introduced` | owner root, policy, familiars |
| **Recover access** | adds one device and grant | owner-root signature over the exact request | `access_recovered` | owner root, policy, other devices, familiars |
| Rotate or replace a device | moves a grant to an already-paired device | local owner (`coven device rotate`) | `device_authorization_reenrolled` | owner root, policy, familiars |
| Rotate the owner root | replaces the root key in the policy chain | old *and* new root keys | `owner_root_rotated` | policy, devices, grants, familiars |
| Register a passkey factor | adds one recovery credential | owner-root signature + the passkey's own assertion | `passkey_registered` | devices, grants, policy, familiars |
| Rotate a familiar identity | — | no such operation exists | — | — |

## Recovering access

`AccessRecoveryAuthority::recover` enrolls a device when no trusted introducer
is available, for example after every trusted device was lost. The owner-root
private key is the sole approver. It stays with the owner, outside
`COVEN_HOME` and off OpenCoven infrastructure.

`AccessRecoveryRequest` (v1) binds:

- the installation audience
- the **exact policy head**: revision plus chain digest, so the signature dies
  on any policy change
- the destination ID and P-256 key
- the complete grant
- the display name and context
- nonce, issued-at and expiry, with at most 300 seconds of validity

The owner-root key signs a `COVEN-ACTION/1` intent with operation
`owner.access_recovery.approve` and an effect digest over
`COVEN-ACCESS-RECOVERY/1\0` followed by the JCS-encoded request. The
destination signs the same intent with operation
`owner.access_recovery.accept`, proving it holds the key being enrolled.

Commit checks run under the policy lock and then the registry lock, in the
documented policy store → device registry order. The head is read and the
enrollment written under that one policy lock, so a rotation or replacement
cannot land between them:

- the installed policy head is still the one named
- the request is inside its validity window
- the grant has an explicit expiry, fits the policy lifetime, and stays within
  the policy's allowed scopes
- the destination's possession signature is valid
- the installed owner-root signature is valid

The commit shares the introduction nonce ledger, so a recovery cannot be
replayed and cannot reuse an introduction nonce. Its transition is marked
`kind: access_recovery`; plain introductions serialize exactly as before.

Recovery enrolls no authorization (step-up) key. A recovered device can use
its grant, but it cannot act as an introducer until a separate key-enrollment
ceremony exists. Recovery does not revoke the lost devices either. Revoke them
explicitly with `coven device revoke --reason lost`, and each revocation is
audited as its own event.

`coven device recover --request ENVELOPE --root-key PEM` prints every signed
field, plus what recovery will *not* do. It signs only after the owner types
`recover`. The root key must be the installed one and must live outside
`COVEN_HOME`.

## Rotating the owner root

`OwnerPolicyStore::rotate_root` appends one chain revision that keeps the
exact current policy and changes only the owner-root key. The outgoing key
signs for succession, and the incoming key signs to prove possession. Both
sign the same `COVEN-ACTION/1` intent, with operation `owner.root.rotate`
and the new record's digest, which binds the incoming key. A signature for
one successor therefore can never install another.

The rotation is refused if it:

- does not extend the current head
- keeps the same key
- changes the policy
- lacks either signature
- falls outside its validity window (at most 300 seconds)

The stored head re-verifies both signatures on every load, so editing in a
different incoming key is refused. Success is audited as
`owner_root_rotated`; failure as `owner_root_rotation_rejected`.

After rotation, the old key approves nothing. Every signature bound to the
old head digest is retired. That includes unused access-recovery approvals
and trusted-device introductions approved but not yet committed. Devices,
grants and familiars are unchanged.

`coven device owner-root rotate --root-key OLD --new-root-key NEW` prints both
key fingerprints and every signed field, and signs only after the owner types
`rotate`. A lost root key cannot be rotated. Recovering from that is an
explicit owner trust-domain reset: remove `mobile/introduction-policy.json`
and install a new policy. This is never automatic.

## Passkey recovery factor

A passkey is an optional owner credential, **never a device**. It has no `DeviceRecord`, grant, introducer identity or pairwise ID, and it is never listed by `coven device list`. Its WebAuthn backup flags are recorded as `backupEligible` and `backedUp`, so a synced passkey is shown as synced and is never treated as evidence of one physical device. Backup eligibility remains fixed after registration; backup state may change, following the [WebAuthn credential backup rules](https://www.w3.org/TR/webauthn-3/#sctn-credential-backup).

**Registration.** `PasskeyStore::register` needs:

- the owner-root signature over `owner.passkey.register`, which binds the audience, current root and root generation, credential ID, RP ID, canonical HTTPS origins and ES256 key
- an assertion from that passkey, proving possession, whose WebAuthn challenge is the SHA-256 of the same action's canonical bytes

The CLI shows the owner-root and credential public keys, root generation, relying party and origins, scope, audience, nonce, validity window and effect digest before asking for confirmation. Origins must be canonical HTTPS origins without credentials, paths, query strings, fragments or terminal controls. Stored credential material must match the root-signed registration digest and the **current root generation**. Rotating the root retires every registration, including when a formerly used root key returns; the owner re-registers. Ordinary policy replacement preserves the generation, while a fresh installation uses its proposal nonce to distinguish the new generation. Registration reads the root and writes the credential under one policy lock (policy store → passkey store), so a rotation lands either before it, which then refuses the old root's signature, or after it, which retires the new credential.

Registration persists an audit outbox with the credential. Success is returned only after the deduplicated `passkey_registered` transition and its local receipt are durable. A failure after replacement reports an uncertain outcome, and an audit failure reports pending reconciliation; neither is a rejection. Reopening through `passkey list`, registration or assertion verification reconciles pending transitions, including retired registrations, before using a factor. An unavailable audit sink prevents that reconciliation and factor use.

The passkey store is version 2; generation-less preview records are refused rather than reinterpreted. Preserve any old store for recovery outside the active path and explicitly re-register credentials with the owner root. Existing owner-policy heads without generation metadata still verify using their original signed bytes; the first updated policy replacement carries forward a stable generation derived from that legacy head. New policy writes include generation metadata in the transition digest. Older binaries refuse those fields, so downgrade requires a compatible recovery plan. This preserves the existing protected-storage trust model: root-approved heads reverify their signatures, while introducer-approved persisted policy heads retain their existing metadata validation and do not gain signature verification from generation metadata.

**What a passkey may do.** It may fill **at most one** of the N approvals of a v2 threshold introduction (`passkeyApprover`). The request must still name at least one trusted device, so N ≥ 2. A passkey can never approve alone, and it has no slot under N = 1.

**Verification.** The passkey's assertion over the exact introduction action (challenge = SHA-256 of its canonical bytes) is verified in Rust, inside the same registry commit as the device approvals:

- the client data type, challenge and origin
- the RP ID hash
- user presence *and* verification
- consistent backup flags and unchanged backup eligibility
- the ES256 signature
- counter advancement when either stored or presented counter is nonzero

An authenticator that always reports zero has no signature-counter protection. It still requires a trusted-device approval and an exact introduction action; the registry's consumed-nonce ledger refuses replay even with fresh signatures and a different destination. A counter that was already nonzero cannot fall back to zero. Counter advancement is a rejection signal for a possible cloned authenticator, not proof that cloning is impossible.

The passkey counter is persisted before the device-registry commit. If the registry write fails before replacement, no destination is enrolled, but the counter and other consumed proofs may already be spent. A replacement can also fail after its rename, yielding an uncertain introduction outcome with enrollment possibly present; reconcile the persisted introduction and device state before considering another attempt. Reusing the assertion is refused. When reconciliation establishes that a fresh attempt is needed, use a fresh introduction nonce, fresh trusted-device approval and a fresh passkey assertion. This ordering prevents replay rather than rolling back an already used factor proof.

No vendor SDK, browser, relying-party server or hosted account service is involved in the decision. Optional account sign-in with a passkey needs a hosted account service and is out of scope here.

The Linux CI job also runs `node crates/coven-cli/tests/fixtures/passkey-recovery/daemon-e2e.mjs "$CARGO_TARGET_DIR/debug/coven"` after the workspace suite. The fixture seeds synthetic trusted devices and uses a synthetic WebAuthn authenticator, then exercises the real CLI, owner socket and pinned-TLS daemon approval. It checks cancellation, review fields, unchanged device membership during registration, enrollment only after the trusted-device approval, replay refusal, and retirement followed by current-root re-registration. A usable private-interface listener is required; networking errors fail the check. Its receipt records the tested binary digest and source commit, and contains no private keys.

## Why a compromised service cannot recover access

A relay or account service holds neither the owner-root key nor the
destination's private key.

- It cannot forge the root signature.
- It cannot swap in its own destination key, because the root signature binds
  that key.
- It cannot replay an approved request, because the nonce is consumed.
- It cannot reuse an unused signature after the owner changes policy, because
  the signature binds the policy head.

Remote transport (#787) will carry the same signed envelope. It adds no
authority.

## Familiar identity

Familiars in this codebase have no cryptographic identity key. A familiar is a
local record (ID, name, role). No recovery, introduction or rotation operation
reads or writes familiar state, and the fixtures check this. Any future
familiar key rotation must be its own explicit, separately audited operation.

## Fixtures

`tests/fixtures/access-recovery/mod.rs`:

```sh
cargo test -p coven-cli --bin coven mobile_memory::recovery --locked
```
