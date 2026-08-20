/** Live dashboard for the local demo. Read-only: it can observe, never authorize. */

import type { ServerResponse } from 'node:http';
import { html } from './http.ts';
import type { LedgerState } from './ledger.ts';

type Snapshot = LedgerState & { reservedUsd: number; remainingUsd: number };

interface PolicyView {
  ok: boolean;
  reason?: string;
  detail?: string;
  policy?: {
    ceilingUsd: number;
    mode: string;
    period: string;
    subBudgets: Record<string, number>;
    digest: string;
    grantTtlSeconds: number;
  };
  rejected: Record<string, { reason: string; detail: string }>;
}

function esc(value: unknown): string {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function usd(value: number): string {
  return `$${value.toFixed(4)}`;
}

export function renderDashboard(res: ServerResponse, snapshot: Snapshot, policy: PolicyView): void {
  const pct =
    snapshot.ceilingUsd > 0
      ? Math.min(100, ((snapshot.settledUsd + snapshot.reservedUsd) / snapshot.ceilingUsd) * 100)
      : 100;

  const policyBanner = policy.ok
    ? `<div class="ok">Policy verified · mode <b>${esc(policy.policy?.mode)}</b> · digest <code>${esc(
        policy.policy?.digest.slice(0, 16),
      )}…</code> · grant TTL ${esc(policy.policy?.grantTtlSeconds)}s</div>`
    : `<div class="bad">POLICY NOT VERIFIED — all paid actions denied<br><small>${esc(
        policy.reason,
      )}: ${esc(policy.detail)}</small></div>`;

  const subBudgets = Object.entries(policy.policy?.subBudgets ?? {})
    .map(([familiar, cap]) => {
      const used = snapshot.receipts
        .filter((r) => r.familiar === familiar)
        .reduce((sum, r) => sum + r.actualUsd, 0);
      const rejected = policy.rejected[familiar];
      return `<tr><td>${esc(familiar)}</td><td>${usd(used)}</td><td>${usd(cap)}</td><td>${
        rejected ? `<span class="bad-inline">${esc(rejected.reason)}</span>` : 'signed'
      }</td></tr>`;
    })
    .join('');

  const receipts = snapshot.receipts
    .slice(-12)
    .reverse()
    .map(
      (r) =>
        `<tr><td>${esc(r.settledAt.slice(11, 19))}</td><td>${esc(r.familiar)}</td><td>${esc(
          r.provider,
        )}/${esc(r.model)}</td><td>${esc(r.inputUnits)}→${esc(r.outputUnits)}</td><td>${usd(
          r.actualUsd,
        )}</td></tr>`,
    )
    .join('');

  const denials = snapshot.denials
    .slice(-12)
    .reverse()
    .map(
      (d) =>
        `<tr><td>${esc(d.at.slice(11, 19))}</td><td>${esc(d.familiar)}</td><td><code>${esc(
          d.reason,
        )}</code></td><td class="detail">${esc(d.detail)}</td></tr>`,
    )
    .join('');

  const page = `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>Bound — spend authority</title>
<meta http-equiv="refresh" content="2">
<style>
:root { color-scheme: dark; }
body { margin:0; padding:32px; background:#0b0b12; color:#e8e6f0;
  font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace; }
h1 { font-size:22px; margin:0 0 4px; letter-spacing:.02em; }
h1 span { color:#8b7ee8; }
h2 { font-size:13px; text-transform:uppercase; letter-spacing:.12em; color:#8a86a3;
  margin:28px 0 10px; }
.sub { color:#8a86a3; margin:0 0 20px; }
.ok, .bad { padding:10px 14px; border-radius:8px; margin-bottom:20px; }
.ok { background:#12251a; border:1px solid #2c6b45; color:#8ce0ac; }
.bad { background:#2a1216; border:1px solid #7a2b38; color:#ff9aa8; }
.bad-inline { color:#ff9aa8; }
.cards { display:flex; gap:14px; flex-wrap:wrap; }
.card { flex:1 1 170px; background:#14141f; border:1px solid #262636; border-radius:10px;
  padding:14px 16px; }
.card .k { color:#8a86a3; font-size:11px; text-transform:uppercase; letter-spacing:.1em; }
.card .v { font-size:24px; margin-top:6px; }
.bar { height:12px; background:#1c1c2a; border-radius:6px; overflow:hidden; margin:18px 0 4px;
  border:1px solid #262636; }
.bar i { display:block; height:100%; background:linear-gradient(90deg,#5b4fd6,#b06be0); }
.bar.full i { background:linear-gradient(90deg,#c0344a,#e0576b); }
table { width:100%; border-collapse:collapse; }
td, th { text-align:left; padding:7px 10px; border-bottom:1px solid #1e1e2c; }
th { color:#8a86a3; font-weight:400; font-size:11px; text-transform:uppercase;
  letter-spacing:.1em; }
code { color:#b7a9ff; }
.detail { color:#8a86a3; }
.empty { color:#55506b; padding:10px; }
footer { margin-top:32px; color:#55506b; font-size:12px; }
</style></head><body>
<h1>Bound <span>·</span> spend authority</h1>
<p class="sub">Coven <b>${esc(snapshot.coven)}</b> · period <b>${esc(
    snapshot.period,
  )}</b> · this page observes, it cannot authorize.</p>
${policyBanner}
<div class="cards">
  <div class="card"><div class="k">Ceiling</div><div class="v">${usd(snapshot.ceilingUsd)}</div></div>
  <div class="card"><div class="k">Settled</div><div class="v">${usd(snapshot.settledUsd)}</div></div>
  <div class="card"><div class="k">Reserved</div><div class="v">${usd(snapshot.reservedUsd)}</div></div>
  <div class="card"><div class="k">Remaining</div><div class="v">${usd(snapshot.remainingUsd)}</div></div>
</div>
<div class="bar${pct >= 100 ? ' full' : ''}"><i style="width:${pct.toFixed(2)}%"></i></div>
<p class="sub">${pct.toFixed(1)}% of the signed ceiling committed</p>

<h2>Familiar sub-budgets</h2>
<table><tr><th>Familiar</th><th>Used</th><th>Sub-budget</th><th>Override</th></tr>
${subBudgets || '<tr><td colspan="4" class="empty">no signed overrides</td></tr>'}</table>

<h2>Receipts (${snapshot.receipts.length})</h2>
<table><tr><th>Time</th><th>Familiar</th><th>Model</th><th>Units</th><th>Cost</th></tr>
${receipts || '<tr><td colspan="5" class="empty">no paid actions yet</td></tr>'}</table>

<h2>Denials (${snapshot.denials.length})</h2>
<table><tr><th>Time</th><th>Familiar</th><th>Reason</th><th>Detail</th></tr>
${denials || '<tr><td colspan="4" class="empty">nothing denied yet</td></tr>'}</table>

<footer>Refreshes every 2s · JSON at <code>/v1/state</code></footer>
</body></html>`;

  html(res, 200, page);
}
