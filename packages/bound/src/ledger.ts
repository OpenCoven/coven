/**
 * SpendLedger — reserve/settle accounting for one (coven, period).
 *
 * Every mutation runs through a single serialized queue, so concurrent
 * reserves cannot race past the ceiling. This is the local stand-in for the
 * one-per-period Durable Object described in the design.
 */

import { mkdirSync, readFileSync, writeFileSync, existsSync, renameSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { round6 } from './pricing.ts';

export interface Reservation {
  grantId: string;
  familiar: string;
  actionClass: string;
  provider: string;
  model: string;
  maxUsd: number;
  createdAt: number;
  expiresAt: number;
}

export interface Receipt {
  receiptId: string;
  grantId: string;
  familiar: string;
  actionClass: string;
  provider: string;
  model: string;
  actualUsd: number;
  inputUnits: number;
  outputUnits: number;
  settledAt: string;
}

export interface Denial {
  denialId: string;
  familiar: string;
  reason: string;
  detail: string;
  at: string;
}

export interface LedgerState {
  coven: string;
  period: string;
  ceilingUsd: number;
  settledUsd: number;
  reservations: Reservation[];
  receipts: Receipt[];
  denials: Denial[];
}

export type ReserveResult =
  | { ok: true; reservation: Reservation; remainingUsd: number; forced?: boolean }
  | { ok: false; reason: 'cap_reached' | 'sub_budget_reached'; detail: string; remainingUsd: number };

export function currentPeriod(now: Date = new Date()): string {
  return `${now.getUTCFullYear()}-${String(now.getUTCMonth() + 1).padStart(2, '0')}`;
}

export class SpendLedger {
  #state: LedgerState;
  #path: string | null;
  #queue: Promise<unknown> = Promise.resolve();

  constructor(options: {
    coven: string;
    ceilingUsd: number;
    period?: string;
    path?: string | null;
    now?: Date;
  }) {
    const period = options.period ?? currentPeriod(options.now);
    this.#path = options.path ?? null;
    this.#state = {
      coven: options.coven,
      period,
      ceilingUsd: options.ceilingUsd,
      settledUsd: 0,
      reservations: [],
      receipts: [],
      denials: [],
    };

    if (this.#path && existsSync(this.#path)) {
      const loaded = JSON.parse(readFileSync(this.#path, 'utf8')) as LedgerState;
      // Period rollover starts clean: a new month never inherits reservations.
      if (loaded.period === period && loaded.coven === options.coven) {
        this.#state = { ...loaded, ceilingUsd: options.ceilingUsd };
      }
    }
    this.#persist();
  }

  /** Serialize every mutation so concurrent reserves cannot both see headroom. */
  #run<T>(fn: () => T): Promise<T> {
    const next = this.#queue.then(fn, fn);
    this.#queue = next.catch(() => undefined);
    return next;
  }

  #persist(): void {
    if (!this.#path) return;
    mkdirSync(dirname(this.#path), { recursive: true });
    const tmp = `${this.#path}.${process.pid}.tmp`;
    writeFileSync(tmp, `${JSON.stringify(this.#state, null, 2)}\n`, { mode: 0o600 });
    renameSync(tmp, this.#path);
  }

  #expire(nowMs: number): void {
    if (this.#state.reservations.length === 0) return;
    this.#state.reservations = this.#state.reservations.filter((r) => r.expiresAt > nowMs);
  }

  #reservedUsd(): number {
    return round6(this.#state.reservations.reduce((sum, r) => sum + r.maxUsd, 0));
  }

  #familiarUsd(familiar: string): number {
    const settled = this.#state.receipts
      .filter((r) => r.familiar === familiar)
      .reduce((sum, r) => sum + r.actualUsd, 0);
    const reserved = this.#state.reservations
      .filter((r) => r.familiar === familiar)
      .reduce((sum, r) => sum + r.maxUsd, 0);
    return round6(settled + reserved);
  }

  remainingUsd(now: Date = new Date()): number {
    this.#expire(now.getTime());
    return round6(this.#state.ceilingUsd - this.#state.settledUsd - this.#reservedUsd());
  }

  reserve(input: {
    grantId: string;
    familiar: string;
    actionClass: string;
    provider: string;
    model: string;
    maxUsd: number;
    ttlSeconds: number;
    subBudgetUsd?: number;
    /** Shadow rollout only: record the denial but still issue the hold. */
    force?: boolean;
    now?: Date;
  }): Promise<ReserveResult> {
    return this.#run(() => {
      const now = input.now ?? new Date();
      const nowMs = now.getTime();
      this.#expire(nowMs);

      const remaining = round6(
        this.#state.ceilingUsd - this.#state.settledUsd - this.#reservedUsd(),
      );
      let forced = false;

      if (input.subBudgetUsd !== undefined) {
        const familiarUsed = this.#familiarUsd(input.familiar);
        if (round6(familiarUsed + input.maxUsd) > input.subBudgetUsd) {
          const detail = `${input.familiar} sub-budget ${input.subBudgetUsd.toFixed(2)} would be exceeded (used ${familiarUsed.toFixed(6)}, requested ${input.maxUsd.toFixed(6)})`;
          this.#recordDenial(input.familiar, 'sub_budget_reached', detail, now);
          if (!input.force) {
            this.#persist();
            return {
              ok: false,
              reason: 'sub_budget_reached',
              detail,
              remainingUsd: remaining,
            } satisfies ReserveResult;
          }
          forced = true;
        }
      }

      if (round6(input.maxUsd) > remaining) {
        const detail = `remaining ${remaining.toFixed(6)} USD is below the ${input.maxUsd.toFixed(6)} USD reservation`;
        this.#recordDenial(input.familiar, 'cap_reached', detail, now);
        if (!input.force) {
          this.#persist();
          return {
            ok: false,
            reason: 'cap_reached',
            detail,
            remainingUsd: remaining,
          } satisfies ReserveResult;
        }
        forced = true;
      }

      const reservation: Reservation = {
        grantId: input.grantId,
        familiar: input.familiar,
        actionClass: input.actionClass,
        provider: input.provider,
        model: input.model,
        maxUsd: round6(input.maxUsd),
        createdAt: nowMs,
        expiresAt: nowMs + input.ttlSeconds * 1000,
      };
      this.#state.reservations.push(reservation);
      this.#persist();
      return {
        ok: true,
        reservation,
        remainingUsd: round6(remaining - reservation.maxUsd),
        forced,
      } satisfies ReserveResult;
    });
  }

  /** Idempotent on grantId: duplicate delivery must not double-charge. */
  settle(input: {
    grantId: string;
    actualUsd: number;
    inputUnits: number;
    outputUnits: number;
    now?: Date;
  }): Promise<{ ok: true; receipt: Receipt; duplicate: boolean; overReservation: boolean }> {
    return this.#run(() => {
      const now = input.now ?? new Date();
      const existing = this.#state.receipts.find((r) => r.grantId === input.grantId);
      if (existing) {
        return { ok: true as const, receipt: existing, duplicate: true, overReservation: false };
      }

      const index = this.#state.reservations.findIndex((r) => r.grantId === input.grantId);
      const reservation = index >= 0 ? this.#state.reservations[index]! : null;
      if (index >= 0) this.#state.reservations.splice(index, 1);

      const actualUsd = round6(Math.max(0, input.actualUsd));
      const overReservation = reservation ? actualUsd > reservation.maxUsd : false;

      const receipt: Receipt = {
        receiptId: randomUUID(),
        grantId: input.grantId,
        familiar: reservation?.familiar ?? 'unknown',
        actionClass: reservation?.actionClass ?? 'unknown',
        provider: reservation?.provider ?? 'unknown',
        model: reservation?.model ?? 'unknown',
        actualUsd,
        inputUnits: input.inputUnits,
        outputUnits: input.outputUnits,
        settledAt: now.toISOString(),
      };

      this.#state.settledUsd = round6(this.#state.settledUsd + actualUsd);
      this.#state.receipts.push(receipt);
      if (overReservation) {
        this.#recordDenial(
          receipt.familiar,
          'over_reservation',
          `actual ${actualUsd} exceeded reservation ${reservation!.maxUsd}`,
          now,
        );
      }
      this.#persist();
      return { ok: true as const, receipt, duplicate: false, overReservation };
    });
  }

  recordDenial(familiar: string, reason: string, detail: string, now: Date = new Date()): Promise<void> {
    return this.#run(() => {
      this.#recordDenial(familiar, reason, detail, now);
      this.#persist();
    });
  }

  #recordDenial(familiar: string, reason: string, detail: string, now: Date): void {
    this.#state.denials.push({
      denialId: randomUUID(),
      familiar,
      reason,
      detail,
      at: now.toISOString(),
    });
    if (this.#state.denials.length > 200) this.#state.denials.splice(0, this.#state.denials.length - 200);
  }

  snapshot(now: Date = new Date()): LedgerState & { reservedUsd: number; remainingUsd: number } {
    this.#expire(now.getTime());
    return {
      ...this.#state,
      reservations: [...this.#state.reservations],
      receipts: [...this.#state.receipts],
      denials: [...this.#state.denials],
      reservedUsd: this.#reservedUsd(),
      remainingUsd: round6(this.#state.ceilingUsd - this.#state.settledUsd - this.#reservedUsd()),
    };
  }
}

export function defaultLedgerPath(coven: string, period: string): string {
  const base = process.env.BOUND_STATE_DIR ?? join(process.cwd(), '.bound-state');
  return join(base, `${coven}-${period}.json`);
}
