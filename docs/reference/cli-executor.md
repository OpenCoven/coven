---
summary: "Stateless executor-node protocol commands (coven.executor.v1)."
read_when:
  - Looking up executor
  - Wiring a machine into the hub as an executor node
title: "coven executor"
description: "Reference for coven executor probe and run-job: the stateless coven.executor.v1 protocol commands a hub invokes over SSH or a private network to poll availability and dispatch jobs."
---

`coven executor` implements the executor side of the multi-host protocol
(`coven.executor.v1`). These commands are not meant for interactive use: the
**hub** invokes them over an outbound transport (SSH or a local process
launch). Executors never push registration or heartbeats to the hub.

```sh
coven executor probe     # print this node's availability envelope as JSON
coven executor run-job   # run one hub-dispatched job from a JSON spec on stdin
```

## Probe

`probe` prints a JSON availability envelope — `protocolVersion`, `role`
(`stationary_executor` or `compute_executor`), advertised `capabilities`,
`available`, `queuePressure` (always 0: stateless executors hold no durable
queue; the hub owns queues), `covenVersion`, and `probedAt`. The node's
advertised role and capabilities come from the optional
`<covenHome>/executor.json`; an absent config means a stationary executor
with base capabilities. The hub polls the probe via
`POST /api/v1/hub/nodes/:id/poll` and fails closed when the advertised role
does not match the registration.

## Run-job

`run-job` reads one job spec from stdin — argv, cwd, env, stdin payload, and
opaque hub context, everything the node needs with no local durable
authority — executes it, and replies on stdout with a normalized result
envelope (stdout/stderr/exit metadata). Transport failures are normalized
into the same envelope shape by the hub-side dispatcher, so
`coven hub dispatch <jobId>` always has a record to show.

## Operating executors

Registration, dispatch, and recovery run through the hub API and CLI:

- `coven hub nodes [<id>]`, `coven hub jobs [<id>]`, `coven hub dispatch
  <jobId>` — read-side inspection ([cli-observe](cli-observe.md)).
- `POST /api/v1/hub/nodes/:id/{poll,dispatch}` — hub-initiated transport.
- [HUB-OPERATIONS](../HUB-OPERATIONS.md) — supervisor setup and restart
  runbook; the multi-host spec lives in
  `specs/coven-multi-host-daemon/TECH.md`.
