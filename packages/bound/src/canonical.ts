/**
 * Canonical serialization for `bound:v1` policy documents.
 *
 * A policy document is ordinary Markdown with exactly one fenced ```bound
 * block. Only the block is signed, so prose can be edited freely without
 * invalidating authority — and editing prose can never grant authority.
 */

export type BoundValue = string | number | boolean | string[];
export type BoundBlock = Record<string, BoundValue>;

export type ParseResult =
  | { ok: true; block: BoundBlock; raw: string }
  | { ok: false; reason: BoundParseReason; detail: string };

export type BoundParseReason =
  | 'no_block'
  | 'multiple_blocks'
  | 'duplicate_key'
  | 'bad_syntax'
  | 'bad_version'
  | 'missing_field';

const BLOCK_RE = /^```bound[ \t]*\r?\n([\s\S]*?)^```[ \t]*$/gm;

const REQUIRED_FIELDS = ['version', 'scope', 'issued_at'] as const;

function fail(reason: BoundParseReason, detail: string): ParseResult {
  return { ok: false, reason, detail };
}

function coerce(raw: string): BoundValue {
  const trimmed = raw.trim();
  if (trimmed === 'true') return true;
  if (trimmed === 'false') return false;
  if (/^-?\d+(\.\d+)?$/.test(trimmed)) return Number(trimmed);
  return trimmed;
}

/** Strip a trailing `# comment` that is not inside a quoted value. */
function stripComment(line: string): string {
  const hash = line.indexOf('#');
  if (hash < 0) return line;
  // A comment marker must be preceded by whitespace or start the line.
  if (hash > 0 && !/\s/.test(line[hash - 1]!)) return line;
  return line.slice(0, hash);
}

export function extractBlock(markdown: string): ParseResult {
  BLOCK_RE.lastIndex = 0;
  const matches = [...markdown.matchAll(BLOCK_RE)];
  if (matches.length === 0) return fail('no_block', 'no fenced bound block found');
  if (matches.length > 1) {
    return fail('multiple_blocks', `${matches.length} bound blocks found, exactly one required`);
  }

  const raw = matches[0]![1]!;
  const block: BoundBlock = {};
  let listKey: string | null = null;

  const lines = raw.split(/\r?\n/);
  for (const original of lines) {
    if (original.includes('\t')) return fail('bad_syntax', 'tabs are not permitted');
    const line = stripComment(original).replace(/\s+$/, '');
    if (line.trim() === '') continue;

    const listItem = /^\s+-\s+(.+)$/.exec(line);
    if (listItem) {
      if (!listKey) return fail('bad_syntax', `list item without a parent key: ${line.trim()}`);
      (block[listKey] as string[]).push(listItem[1]!.trim());
      continue;
    }

    const pair = /^([A-Za-z][A-Za-z0-9_]*):[ \t]*(.*)$/.exec(line);
    if (!pair) return fail('bad_syntax', `unparsable line: ${line.trim()}`);

    const key = pair[1]!;
    const value = pair[2]!;
    if (Object.hasOwn(block, key)) return fail('duplicate_key', `duplicate key: ${key}`);

    if (value.trim() === '') {
      block[key] = [];
      listKey = key;
    } else {
      block[key] = coerce(value);
      listKey = null;
    }
  }

  if (block.version !== 1) return fail('bad_version', `unsupported version: ${String(block.version)}`);
  for (const field of REQUIRED_FIELDS) {
    if (!Object.hasOwn(block, field)) return fail('missing_field', `missing required field: ${field}`);
  }

  return { ok: true, block, raw };
}

/** Stable JSON: keys sorted, arrays order-preserving, no insignificant whitespace. */
export function canonicalJson(block: BoundBlock): string {
  const keys = Object.keys(block).sort();
  const parts = keys.map((k) => `${JSON.stringify(k)}:${JSON.stringify(block[k])}`);
  return `{${parts.join(',')}}`;
}

/**
 * Bytes covered by a Val signature. Binding the role prevents a familiar
 * override from being replayed as the coven-wide policy.
 */
export function canonicalBytes(block: BoundBlock, role: string): Buffer {
  const payload = `bound:v1\n${role}\n${String(block.issued_at)}\n${canonicalJson(block)}`;
  return Buffer.from(payload, 'utf8');
}
