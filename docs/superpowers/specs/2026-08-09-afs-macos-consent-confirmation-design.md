# AgentFS macOS Consent Confirmation Documentation Design

**Status:** Approved documentation update
**Bead:** `coven-x77`

## Evidence

On 2026-08-09, a human-operated Terminal successfully:

1. observed `afs_serve` listening on `127.0.0.1:12049`;
2. mounted the loopback NFSv3 export at `/tmp/afsmnt`;
3. created a directory and file through the mounted path; and
4. read the written content back through the mounted path.

The initial connection-refused attempt occurred before the server finished
compiling and listening, and its local write is not evidence. The later mounted
write/read is the accepted confirmation.

## Decision

The macOS NFS path is viable for a client process granted the required
network-volume access. This resolves the manual confirmation gate from
`coven-x77`; it does not make the NFS export a supported CLI workflow,
sandbox, access-control boundary, or default-on feature.

The prior agent-process `EPERM` observation remains relevant: client-process
privacy/consent is a deployment requirement. Loopback NFS authentication,
concurrent-client behavior, Linux/FUSE validation, sandbox enforcement, and
default-on safety remain unresolved.

## Documentation changes

- Update the source mount-spike result, research summary, and AgentFS design
  sequencing to replace the outstanding confirmation with the verified
  consent-enabled Terminal result.
- Update the public AgentFS guide, architecture overview, and security posture
  so they distinguish passed Terminal validation from the unresolved
  process-consent and security boundaries.
- Add the observed command outcome to `coven-x77`, then close it after both
  source and public documentation PRs merge.

## Validation

Check documentation diffs for whitespace; run source privacy and secret guards;
build and link-check the public docs site; and verify all changed text retains
the experimental, feature-gated, non-sandboxed status.
