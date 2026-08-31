// Virtual-clock and schedule math for the conformance reference oracle.
//
// This module mirrors, exactly, the schedule contract the Coven scheduler
// implements (crates/coven-cli/src/automations/rrule.rs + schedule.rs) and the
// deterministic extensions the conformance vectors pin (IANA zones, gap/fold):
//   - scoped RRULE vocabulary: FREQ=DAILY|WEEKLY, BYHOUR (0-23, unique,
//     canonicalized ascending), BYDAY for WEEKLY (canonicalized to MO..SU);
//     every other key or frequency is refused;
//   - BYHOUR defaults to 9; WEEKLY BYDAY defaults to MO;
//   - a slot whose local wall time does not exist (DST spring gap) is skipped;
//   - a slot whose local wall time repeats (DST fall fold) takes the earliest
//     instant;
//   - next-due walks up to 10 candidate dates from the local date of `from`.
//
// Timezones: "utc" resolves in UTC; "local" resolves against the pinned host
// zone of the run; anything else must be an IANA zone name. All math is pure
// (no ambient clock) so vectors are deterministic.

const RRULE_SUPPORTED_KEYS = new Set(['FREQ', 'BYHOUR', 'BYDAY']);
const RRULE_SUPPORTED_FREQS = new Set(['DAILY', 'WEEKLY']);
const WEEKDAY_CODES = ['MO', 'TU', 'WE', 'TH', 'FR', 'SA', 'SU'];
const WEEKDAY_LONG = ['MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT', 'SUN'];

export class RruleError extends Error {}

export function parseRrule(text) {
  if (typeof text !== 'string' || text.trim() === '') {
    throw new RruleError('rrule is required');
  }
  let frequency = null;
  let byHour = null;
  let byDay = null;

  for (const rawPart of text.split(';')) {
    const part = rawPart.trim();
    if (part === '') continue;
    const eq = part.indexOf('=');
    if (eq === -1) throw new RruleError(`rrule part \`${part}\` is not KEY=VALUE`);
    const key = part.slice(0, eq).trim().toUpperCase();
    const value = part.slice(eq + 1).trim();
    if (value === '') throw new RruleError(`rrule ${key} has an empty value`);
    if (!RRULE_SUPPORTED_KEYS.has(key)) {
      throw new RruleError(`rrule key \`${key}\` is not supported`);
    }
    if (key === 'FREQ') {
      const freq = value.toUpperCase();
      if (!RRULE_SUPPORTED_FREQS.has(freq)) {
        throw new RruleError(`FREQ \`${freq}\` is not supported (DAILY or WEEKLY)`);
      }
      frequency = freq;
    } else if (key === 'BYHOUR') {
      const hours = [];
      for (const entry of value.split(',')) {
        const trimmed = entry.trim();
        if (trimmed === '') throw new RruleError('BYHOUR contains an empty entry');
        const parsedHour = Number(trimmed);
        if (!Number.isInteger(parsedHour) || String(parsedHour) !== trimmed) {
          throw new RruleError(`BYHOUR entry \`${trimmed}\` is not an integer`);
        }
        if (parsedHour < 0) throw new RruleError(`BYHOUR entry ${parsedHour} is negative`);
        if (parsedHour > 23) throw new RruleError(`BYHOUR entry ${parsedHour} exceeds 23`);
        if (hours.includes(parsedHour)) {
          throw new RruleError(`BYHOUR repeats entry ${parsedHour}`);
        }
        hours.push(parsedHour);
      }
      byHour = hours.sort((a, b) => a - b);
    } else if (key === 'BYDAY') {
      const days = new Set();
      for (const entry of value.split(',')) {
        const day = entry.trim().toUpperCase();
        const short = WEEKDAY_CODES.indexOf(day);
        const long = WEEKDAY_LONG.indexOf(day);
        if (short === -1 && long === -1) {
          throw new RruleError(`BYDAY entry \`${day}\` is not a weekday`);
        }
        days.add(WEEKDAY_CODES[short !== -1 ? short : long]);
      }
      if (days.size === 0) throw new RruleError('BYDAY must contain at least one weekday');
      byDay = [...days].sort();
    }
  }

  if (frequency === null) throw new RruleError('rrule requires FREQ');
  if (byHour === null) byHour = [9];
  if (byHour.length === 0) throw new RruleError('BYHOUR must contain at least one hour');
  if (frequency === 'DAILY') {
    byDay = [];
  } else if (byDay === null) {
    byDay = ['MO'];
  }

  return { frequency, byHour, byDay };
}

const partsFormatterCache = new Map();

function partsFormatter(tz) {
  let formatter = partsFormatterCache.get(tz);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat('en-US', {
      timeZone: tz,
      hourCycle: 'h23',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    });
    partsFormatterCache.set(tz, formatter);
  }
  return formatter;
}

// Offset (ms east of UTC) in effect at `utcMillis` in `tz`.
export function tzOffsetMs(tz, utcMillis) {
  const base = Math.floor(utcMillis / 1000) * 1000;
  const parts = partsFormatter(tz).formatToParts(new Date(base));
  const get = (type) => Number(parts.find((part) => part.type === type).value);
  const asUTC = Date.UTC(get('year'), get('month') - 1, get('day'), get('hour'), get('minute'), get('second'));
  return asUTC - base;
}

// Resolves a naive wall-clock time (given as the pseudo-UTC millis of that
// wall time) in `tz`. Returns:
//   { status: 'single', instant }
//   { status: 'fold',   instant, latestInstant }  // instant = earliest pass
//   { status: 'gap' }                              // wall time does not exist
export function resolveWall(tz, wallMillis) {
  const oBefore = tzOffsetMs(tz, wallMillis - 6 * 3600e3);
  const oAfter = tzOffsetMs(tz, wallMillis + 6 * 3600e3);
  const candidates = [...new Set([wallMillis - oBefore, wallMillis - oAfter])];
  const valid = candidates.filter((t) => t + tzOffsetMs(tz, t) === wallMillis);
  if (valid.length === 0) return { status: 'gap' };
  if (valid.length === 1) return { status: 'single', instant: valid[0] };
  const sorted = [...valid].sort((a, b) => a - b);
  return { status: 'fold', instant: sorted[0], latestInstant: sorted[sorted.length - 1] };
}

function resolveTimezone(timezone, hostTimezone) {
  if (timezone === 'utc') return 'UTC';
  if (timezone === 'local') {
    if (!hostTimezone) throw new RruleError('local timezone requires a pinned hostTimezone');
    return hostTimezone;
  }
  return timezone;
}

function localDateParts(tz, utcMillis) {
  const parts = partsFormatter(tz).formatToParts(new Date(utcMillis));
  const get = (type) => Number(parts.find((part) => part.type === type).value);
  return { year: get('year'), month: get('month'), day: get('day') };
}

function weekdayIndexMondayFirst(dateParts) {
  const asUTC = Date.UTC(dateParts.year, dateParts.month - 1, dateParts.day);
  return (new Date(asUTC).getUTCDay() + 6) % 7; // Monday = 0 .. Sunday = 6
}

// Next scheduled instant strictly after `fromMillis`, or null. Mirrors the
// Rust next_due walk (up to 10 candidate dates).
export function nextDue(rruleText, timezone, fromMillis, hostTimezone = 'UTC') {
  const parsed = typeof rruleText === 'string' ? parseRrule(rruleText) : rruleText;
  const tz = resolveTimezone(timezone, hostTimezone);
  const windowStart = localDateParts(tz, fromMillis);
  const allowedDays =
    parsed.frequency === 'DAILY'
      ? null
      : new Set(
          parsed.byDay.map((code) => WEEKDAY_CODES.indexOf(code)).filter((index) => index >= 0)
        );

  for (let offsetDays = 0; offsetDays < 10; offsetDays += 1) {
    const date = new Date(
      Date.UTC(windowStart.year, windowStart.month - 1, windowStart.day + offsetDays)
    );
    const parts = {
      year: date.getUTCFullYear(),
      month: date.getUTCMonth() + 1,
      day: date.getUTCDate()
    };
    if (allowedDays !== null && !allowedDays.has(weekdayIndexMondayFirst(parts))) continue;

    for (const hour of parsed.byHour) {
      const wallMillis = Date.UTC(parts.year, parts.month - 1, parts.day, hour, 0, 0);
      const resolved = resolveWall(tz, wallMillis);
      if (resolved.status === 'gap') continue; // DST gap: the slot does not exist
      const instant = resolved.instant; // DST fold: earliest pass
      if (instant > fromMillis) return instant;
    }
  }
  return null;
}

// Latest due slot in (cursorMs, nowMs], computed directly from the calendar.
//
// The previous implementation walked forward one slot at a time (nextDue in
// a loop) under a 4096-step cap: after a long outage the walk ran out of
// steps and silently returned a stale slot. There is no walk here and no
// cap to exhaust: candidate local dates descend from the local date of
// `now` (eight days reaches every weekday once plus one spare day, which
// covers DST transition days whose slot may not exist), hours descend, DST
// gaps do not exist, and DST folds take the earliest pass. The first slot
// found with cursorMs < instant <= nowMs is the latest one; because scan
// order is descending in instant, an instant at or below cursorMs proves no
// qualifying slot exists.
export function latestDueSlot(rruleText, timezone, cursorMs, nowMs, hostTimezone = 'UTC') {
  if (!Number.isFinite(cursorMs) || !Number.isFinite(nowMs) || nowMs <= cursorMs) return null;
  const parsed = typeof rruleText === 'string' ? parseRrule(rruleText) : rruleText;
  const tz = resolveTimezone(timezone, hostTimezone);
  const allowedDays =
    parsed.frequency === 'DAILY'
      ? null
      : new Set(
          parsed.byDay.map((code) => WEEKDAY_CODES.indexOf(code)).filter((index) => index >= 0)
        );
  const nowParts = localDateParts(tz, nowMs);
  const hoursDescending = [...parsed.byHour].sort((a, b) => b - a);
  for (let back = 0; back <= 8; back += 1) {
    const date = new Date(Date.UTC(nowParts.year, nowParts.month - 1, nowParts.day - back));
    const parts = {
      year: date.getUTCFullYear(),
      month: date.getUTCMonth() + 1,
      day: date.getUTCDate()
    };
    if (allowedDays !== null && !allowedDays.has(weekdayIndexMondayFirst(parts))) continue;
    for (const hour of hoursDescending) {
      const wallMillis = Date.UTC(parts.year, parts.month - 1, parts.day, hour, 0, 0);
      const resolved = resolveWall(tz, wallMillis);
      if (resolved.status === 'gap') continue; // the slot does not exist
      const instant = resolved.instant; // fold: earliest pass
      if (instant > nowMs) continue;
      if (instant <= cursorMs) return null; // scan order is descending: nothing qualifies
      return instant;
    }
  }
  return null;
}

// Independent brute-force oracle for the latest due slot: enumerates every
// candidate slot day by day over the whole (cursorMs, nowMs] window and
// returns the maximum. Deliberately O(days) and structurally unlike the
// direct computation above, so agreement between the two is real evidence:
// the no-silent-eligible-occurrence-loss invariant checks the planner
// against THIS implementation, never against itself.
export function latestDueSlotBrute(rruleText, timezone, cursorMs, nowMs, hostTimezone = 'UTC') {
  if (!Number.isFinite(cursorMs) || !Number.isFinite(nowMs) || nowMs <= cursorMs) return null;
  if (nowMs - cursorMs > 200 * 366 * 86400e3) {
    throw new Error(
      `latestDueSlotBrute window exceeds 200 years (${Math.round((nowMs - cursorMs) / (365 * 86400e3))}y); refusing to scan`
    );
  }
  const parsed = typeof rruleText === 'string' ? parseRrule(rruleText) : rruleText;
  const tz = resolveTimezone(timezone, hostTimezone);
  const allowedDays =
    parsed.frequency === 'DAILY'
      ? null
      : new Set(
          parsed.byDay.map((code) => WEEKDAY_CODES.indexOf(code)).filter((index) => index >= 0)
        );
  const startParts = localDateParts(tz, cursorMs);
  const endParts = localDateParts(tz, nowMs);
  const startPseudo = Date.UTC(startParts.year, startParts.month - 1, startParts.day);
  const endPseudo = Date.UTC(endParts.year, endParts.month - 1, endParts.day);
  let latest = null;
  for (let pseudo = startPseudo; pseudo <= endPseudo; pseudo += 86400e3) {
    const date = new Date(pseudo);
    const parts = {
      year: date.getUTCFullYear(),
      month: date.getUTCMonth() + 1,
      day: date.getUTCDate()
    };
    if (allowedDays !== null && !allowedDays.has(weekdayIndexMondayFirst(parts))) continue;
    for (const hour of parsed.byHour) {
      const wallMillis = Date.UTC(parts.year, parts.month - 1, parts.day, hour, 0, 0);
      const resolved = resolveWall(tz, wallMillis);
      if (resolved.status === 'gap') continue;
      const instant = resolved.instant;
      if (instant <= cursorMs || instant > nowMs) continue;
      if (latest === null || instant > latest) latest = instant;
    }
  }
  return latest;
}

const ISO_PATTERN = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,3})\d*)?(?:[Zz]|([+-]\d{2}):?(\d{2}))$/;

// Parses an RFC3339 timestamp (millisecond precision) into epoch millis.
export function parseIso(text) {
  const match = ISO_PATTERN.exec(text);
  if (!match) throw new Error(`not a valid RFC3339 timestamp: ${text}`);
  const [, y, mo, d, h, mi, s, frac, offH, offM] = match;
  let millis = Date.UTC(
    Number(y),
    Number(mo) - 1,
    Number(d),
    Number(h),
    Number(mi),
    Number(s),
    frac ? Number(frac.padEnd(3, '0')) : 0
  );
  if (offH !== undefined) {
    const offsetSign = offH.startsWith('-') ? -1 : 1;
    millis -= offsetSign * (Math.abs(Number(offH)) * 3600e3 + Number(offM) * 60e3);
  }
  return millis;
}

// Formats epoch millis as an RFC3339 UTC timestamp with millisecond precision
// and a literal Z — the exact form the Coven store persists.
export function iso(millis) {
  return new Date(millis).toISOString();
}

export function isoOrThrow(millis) {
  if (!Number.isInteger(millis)) throw new Error(`not an integer epoch: ${millis}`);
  return iso(millis);
}
