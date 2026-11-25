---
title: "Open work by plan"
description: "Maintainer ledger of every open Coven plan and epic: its open issues, what is already on main, the next unblocked step, and who decides."
source_adjacent_reason: "Tracks issue-level delivery state of this repository against its main branch."
---

# Open work by plan

_Reconciled 2026-11-25 against `main` at `9415c309`._

This page maps each open plan or epic to the issues still open under it, what
has already landed on `main`, the next step that nobody is blocked on, and who
has to decide anything before work can continue. It replaces the MVP-era
milestone board in [MVP-PLAN.md](../MVP-PLAN.md). The public direction remains
in the [roadmap](../ROADMAP.md).

GitHub issues stay the only work tracker, as
[work tracking](../work-tracking.md) requires. This page is a dated snapshot of
them. When it disagrees with an issue, the issue wins; refresh this page
instead of editing around it.

"Owner" below means the repository maintainer. An item marked **owner** is
waiting for a maintainer decision, not for engineering work.

## Summary

| Plan | Open issues under it | On `main` | Next unblocked step | Waiting on the owner |
|---|---|---|---|---|
| [#787](https://github.com/OpenCoven/coven/issues/787) Trusted-device reconnection | #1529–#1532, #1612, #1679, #1682, #1686, #1732, #1744, #1765 | Steps 1–3: relay client, v2 routing, authenticated migration | #1529: choose and audit a LAN-discovery dependency | push hosting (#1531), #1732 mechanism, #1765 |
| [#788](https://github.com/OpenCoven/coven/issues/788) Recovery and trusted-device introduction | none | Every planned slice | Owner decides where three untracked items go, then closes or narrows | yes |
| [#804](https://github.com/OpenCoven/coven/issues/804) Agent invocation contracts | none | Steps 1–5 | Rebuild the recovered attempt-observability patch on `main` | second-executor backend |
| [#806](https://github.com/OpenCoven/coven/issues/806) Authority-module concentration | none | Slices 1–6 | Publish slice 7 from `refactor/806-replay` | none |
| [#854](https://github.com/OpenCoven/coven/issues/854) Automations v1 program | #857, #858, #1378, #1608 | Gate A fixes, Gate B adapter | Refresh the A1–D4 go/no-go matrix | release go/no-go |
| [#857](https://github.com/OpenCoven/coven/issues/857) Run authority binding | #1313, #1378, #1608, #858 | Receipts, sidecar, scoped reads, trusted adapter | Rebase the #1608 tuple work and add the binary pin | #1608 tuple approval |
| [#858](https://github.com/OpenCoven/coven/issues/858) Conformance and diagnostics | #1378, #1608 | Runner, diagnostics, Windows runner foundation | Same as #857 | win32 refusal (#1378), SLOs |
| [#1183](https://github.com/OpenCoven/coven/issues/1183) Daemon-owned main session | #1137, #1182 | P1–P5 | #1182 slice 1: route main-session start through admission | #1137 questions |
| [#1238](https://github.com/OpenCoven/coven/issues/1238) Psyche | #1230–#1237, #1629 | O2, O3, O4 design and plan | Land the recovered #1629 O4 implementation | psyche#12, #1232 D1–D7 |
| [#1442](https://github.com/OpenCoven/coven/issues/1442) Reproducible cloud environment | #1446 | Bootstrap and readiness scripts | none for an agent | Codex Cloud environment |
| [#1444](https://github.com/OpenCoven/coven/issues/1444) Autonomous issue-to-PR delivery | #1451, #1452, #1453 | Maintainer skill, preflight | Review and queue #1819 | #1819, Cloud environment |

Issues outside these plans are listed under [Standalone issues](#standalone-issues).

## Trusted devices

### #787 Seamless trusted-device reconnection

**Goal:** an enrolled phone reconnects without a QR code. Recognition is a device-key challenge. The relay comes first, with a private LAN fast path and automatic fallback, no linkage across Covens, and push-assisted approval.

**On `main`:**

- The reversible `suspended` device state (#1139).
- The returning-device transport contract (#1539) and the accepted v2 routing and wire profile (#1623).
- The outbound encrypted relay client (#1551) and its limit fixes (#1633, #1642, #1649, #1731).
- v2 rendezvous routing (#1677).
- Connection state and doctor (#1694), replay-nonce retention (#1712), and authenticated relay↔direct migration (#1718).
- The integrated v2 journeys (#1728, #1699) and relay follow-ups (#1754, #1787).

**Open issues:**

| Issue | State | Next step |
|---|---|---|
| #1529 LAN discovery and fallback | not started | Choose and audit an mDNS/DNS-SD dependency, or record a no-dependency design; write the privacy capture test first |
| #1530 Multi-Coven unlinkability | not started; needs #1529 | A relay-only two-Coven capture harness can start now |
| #1531 Push-assisted approval | not started | **Owner:** push provider and hosting |
| #1532 BLE introduction research | not started | A docs-only research note; then **owner** disposition |
| #1612 Relay manager reload cadence | draft #1675 | Rebase #1675 onto `main` |
| #1679 v2 power-loss durability | not started | Choose a power-loss harness |
| #1682 Windows and physical-device acceptance | not started | Native Windows journey; real-network mobile journey |
| #1686 Revocation during transmission | not started | Choose bounded-window or release-prevention; take a contract change to #1526 first |
| #1732 Step-up grant binding after reissue | blocked | **Owner:** choose the mechanism on #1526 |
| #1744 Mobile transport extraction for Chat | not started | Allocate a worktree and do the extraction checkpoint |
| #1765 Pairing worker cancelled mid-handshake | blocked | **Owner:** is losing one room acceptable? |

**Not covered by any issue:** an independent review of the cryptographic profile; physical Wi-Fi, cellular and NAT switching beyond #1682; and an explicit decision on sender-constrained resumption tokens, which were deliberately not shipped.

### #788 Recovery, trusted-device introduction, optional attestation

**On `main`:** owner introduction policy and CLI (#1535, #1541), local enrollment (#1546), N-of-M introductions (#1549), owner-root recovery and rotation (#1552, #1639), the optional passkey factor (#1556), and optional device and Apple App Attest attestation (#1558, #1650).

**Open issues:** none.

**Owner decision:** three items have no issue: remote or relay introduction transport, passkey account sign-in, and certification on physical devices and browsers. Each stays on #788 or moves to its own issue before the epic closes.

## Agents and authority modules

### #804 Agent invocation and delegation contracts

**On `main`:** ingress guardrails (#866), invocation correlation and revisioned events (#956, #1034), bounded delegation (#1369), `AgentLoop` behind the `Runner` facade (#1376), lifecycle bounds (#1420), the durable invocation record and delegated-child journal (#1425, #1536, #1537, #1542), and bounded durable admission (#1666).

**Open issues:** none. Steps 6–11 have no child issues.

**Next:** rebuild the recovered attempt-observability patch on `main` and publish it.

**Owner decision:** the production backend needed for a second real executor.

### #806 Authority-module concentration

**On `main`:** response and request mapping (#929, #930), the module inventory (#958, #1043), store schema initialization (#1033), and proposal slices 1–6 (#1456, #1579, #1631, #1634, #1654, #1665).

**Next:** reconcile the local `refactor/806-replay` branch with `main` and publish slice 7 alone. The merge queue takes one slice at a time.

**Remaining:** daemon transport and supervision in `daemon.rs` have not started, and dead compatibility paths have not been removed.

## Automations

### #854 Automations v1 program

**On `main`:** the tracker (#910), native readiness audits (#1060), and recent Gate A fixes (#1561, #1797, #1801, #1804, #1820). Both conformance manifests still report `productionReady: false`.

**Next:** post a refreshed A1–D4 go/no-go matrix. Every row needs a merged commit and CI run, or a named gap owner. The last matrix predates the trusted adapter.

**Owner decision:** the exact-release go/no-go. The previous decision was no-go.

### #857 Run authority binding

**On `main`:**

- Receipt reads, the authority sidecar, no-launch receipts and the terminal-evidence inbox (#975, #980, #983, #989).
- Signed Threads decisions (#1602).
- The scoped-read stack (#1426, #1441, #1455, #1460, #1471).
- `mission.submit` scopes (#1598).
- The trusted Runtime Authority adapter (#1604), commitment reconciliation (#1671), and the launch-folder reaper (#1674).

**Open issues:**

| Issue | State | Next step |
|---|---|---|
| #1608 Scoped-read tuple certification | draft #1637 on a stale base | Rebase, add the binary sha256 pin, run the packed artifact through the #858 runner; then **owner** tuple approval |
| #1313 Mission contract | contract, daemon commands, SDK and CLI on `main`; mission launch in draft #1676 | Record which release Chat pins; decide where the launch vectors live |
| #1378 Windows containment | containment and native fixtures on `main` | **Owner:** lift the win32 refusal in the runner, or keep it |

**Not covered by any issue:** the approval lifecycle (requested, approved, consumed) for R1–R4 runs.

### #858 Conformance and operator diagnostics

**On `main`:** the independent runner (#1006), diagnostics and explain (#1381, #1386, #1515, #1519, #1621), `automations retry` (#1572), and the Windows runner foundation (#1384, #1385, #1389).

**Remaining with no issue:** the missing `events --after`, `schedule --dry-run`, `reconcile --dry-run` and `cancel` commands; continuity, privacy and full-profile suites; the golden-scenario set; load and SLO harnesses; fault suites; and consumer canaries.

**Owner decisions:** the win32 refusal (#1378) and SLO ratification.

## Main session and chat context

### #1183 Daemon-owned main session

**On `main`:** phases P1–P5 (#1184, #1186, #1269, #1321, #1324, #1349, #1307, #1365, #1374, #1390, #1397).

| Issue | State | Next step |
|---|---|---|
| #1137 Ordinary-chat context admission | blocked; every intent is refused | **Owner:** answer the context-binding and receipt-reconciliation questions in a decision PR against `docs/architecture/ordinary-chat-context-admission-v1.md` |
| #1182 Memory priming | gated by #1137 | Slice 1 needs no decision: route main-session start through admission and log the refusal |

**Not covered by any issue:** an installed-runtime matrix that covers one shared mobile and desktop flow.

## Psyche

### #1238 Psyche

**On `main`:** the O2 and O3 contracts (#732, #753, #754), the O4 design (#1627), and the O4 plan (#1630).

| Issue | State | Next step |
|---|---|---|
| #1230 Real-Coven authority conformance | partial | Land O4 (#1629), design O5–O8, implement the identityRebind mapping |
| #1629 O4 adoption lookup | design and plan only | Reconcile the recovered candidate with `main` |
| #1237 Identity and session binding | not started | Needs #1230 |
| #1235 Typed Telegram transports | not started | Activation on psyche#12, then a transport plan |
| #1236 Access, routing and commands | not started | Needs #1235 and #1237 |
| #1234 Delivery and streaming | not started | Needs #1235 and #1237 |
| #1232 Media and interactions | not started | **Owner:** decisions D1–D7 |
| #1233 Reliability and operations | not started | **Owner:** webhook decisions |
| #1231 Parity, migration and release | not started | **Owner:** canary shadow mode against P-36 |

The Telegram work in #1231–#1236 is implemented in OpenCoven/psyche. Coven owes the producer contracts and the real-daemon conformance packet in #1230.

## Cloud delivery workflow

### #1442 Reproducible cloud environment

**On `main`:** bootstrap and readiness scripts (#1459, #1465), the container matrix (#1482), and the installer bounds (#1538, #1544, #1632).

**Open issues:** #1446, which needs a managed run.

**Owner decision:** create the Codex Cloud environment for this repository. No agent step remains until it exists.

### #1444 Autonomous issue-to-PR delivery

**On `main`:** the maintainer skill and checkpoint validator (#1573), and the publication preflight with a blocked handoff (#1575).

| Issue | State | Next step |
|---|---|---|
| #1451 Maintainer skill | delivered locally | **Owner:** managed Cloud resume, or a waiver |
| #1452 Publication preflight | fix in #1819, green | **Owner:** review and queue #1819 |
| #1453 Reliability scorecard | three local pilots | Needs the Cloud environment |

## Standalone issues

| Issue | State | Next step |
|---|---|---|
| #924 Identity-bound Ward disposal | Windows half on `main` (#955) | **Owner:** choose Option A or B for Unix |
| #1067 macOS piped-lifecycle flakes | only row 11 is open | Markers inside the startup-pass reconciliation |
| #1227 Windows suite failures | most families closed | #1067 row 11, #1765, #1413 acceptance |
| #1413 Template rejection reasons | delivered in #1574 | Independent review or waiver; fetched-main verification |
| #1701 Engine update and activation | pin side on `main`; rest in draft #1703 | Rebase #1703 |
| #1702 Codex familiar-chat plugin | not started on `main` | Recover and publish the candidate |
| #1716 Threads audit lifecycle | held candidate, not published | **Owner:** publish or not |
| #1736 Ward-bound session execution | not on `main`; candidate lost | **Owner:** rebuild from the rescued diff, or retire |
| #1738 Claude canonical spawn policy | both halves on `main` (engine 0.9.3) | Repeat the behavior check against 0.9.3, then close |
| #1768 Release v0.5.1 | candidate `9415c309` prepared | **Owner:** signed tag |
| #1792 Denied-only hardlinks | fixed in #1813 | **Owner:** authorize the installed native retry |
| #1798 chrono advisories | ignored in `deny.toml` (#1799) | **Owner:** migrate to jiff, or wait |
| #1805 Castle folder sync | #1809, #1815, #1830 | Registry install receipt; Linux and Windows named-Castle journeys |
| #1811 familiar-edit audit flake | one local failure | Add test-only phase markers |
| #1814 run-budget adapter flake | investigation active | Capture the stdout-disconnect failure |

## Refreshing this page

1. Fetch `origin`, then list open issues over the REST API.
2. For each plan, check its open children and the merged PRs linked from their timelines.
3. Count a requirement as delivered only when `main` has the change and any required native, hosted or owner evidence is recorded on the issue.
4. Update the date at the top and the summary table in the same change.
