// Redaction for everything the conformance plane publishes.
//
// Structured values are redacted before serialization (redactDeep walks the
// object and replaces sensitive keys' values), then the serialized text is
// scrubbed again (redactText) so anything that reached a string form — log
// lines, receipts, diagnostics, error messages — is covered too. The review
// (#882, finding 5) names the holes in the previous implementation: prompts
// shorter than four characters, multiline and quoted prompts, credentials,
// Windows paths, and /private/... macOS paths all leaked. Every form below
// is covered and pinned by tests.

export const REDACTION_RULES = [
  'sensitive structured values (prompts, credentials, user paths) replaced before serialization',
  'definition prompts of any length, including multiline and quoted occurrences, replaced with [redacted]',
  'credential-shaped strings (cloud keys, OAuth/CI tokens, API keys, bearer tokens, passwords, PEM blocks, userinfo URLs) replaced with [redacted]',
  'POSIX user paths (/Users, /home, /root, /private/...) and Windows paths (drive-letter, %USERPROFILE%, UNC) replaced with [redacted-path]'
];

const PATH_PLACEHOLDER = '[redacted-path]';
const SECRET_PLACEHOLDER = '[redacted]';

// Keys whose value is a prompt or free-text intent — replaced wholesale.
const PROMPT_KEYS = new Set([
  'prompt',
  'intent',
  'intentStatement',
  'statement',
  'instruction',
  'userMessage'
]);

// Keys whose value is a path — replaced with the path placeholder.
const PATH_KEYS = new Set(['cwd', 'outputTarget', 'outputPath', 'filePath', 'logPath']);

// Keys whose value is a credential or other secret — replaced wholesale.
const SECRET_KEYS = new Set([
  'password',
  'passwd',
  'pwd',
  'secret',
  'token',
  'accessToken',
  'refreshToken',
  'sessionToken',
  'apiKey',
  'api_key',
  'authorization',
  'privateKey',
  'private_key',
  'credentials',
  'cookie',
  'setCookie'
]);

const CREDENTIAL_PATTERNS = [
  { name: 'aws-access-key', pattern: /AKIA[0-9A-Z]{16}/g },
  { name: 'github-token', pattern: /gh[pousr]_[A-Za-z0-9]{36,255}/g },
  { name: 'github-fine-grained', pattern: /github_pat_[A-Za-z0-9_]{22,255}/g },
  { name: 'slack-token', pattern: /xox[baprs]-[A-Za-z0-9-]{10,}/g },
  { name: 'gitlab-pat', pattern: /glpat-[A-Za-z0-9_-]{20,}/g },
  { name: 'google-api-key', pattern: /AIza[0-9A-Za-z_-]{35}/g },
  { name: 'openai-style-key', pattern: /sk-(?:proj-|svcacct-|ant-)?[A-Za-z0-9_-]{20,}/g },
  { name: 'bearer-token', pattern: /[Bb]earer\s+[A-Za-z0-9._~+/=-]{8,}/g },
  { name: 'key-value-secret', pattern: /\b(?:password|passwd|pwd|secret|token|api[-_]?key|client[-_]?secret)\b\s*["']?\s*[:=]\s*["']?[^\s"',;})\\]+/gi },
  { name: 'pem-block', pattern: /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g },
  { name: 'userinfo-url', pattern: /\b[a-z][a-z0-9+.-]*:\/\/[^\s:/@"]+:[^\s/@"]+@[^\s"']+/g }
];

const PATH_PATTERNS = [
  // POSIX homes: /Users/alice/..., /home/bob/..., /root/...
  { name: 'posix-home', pattern: /(?:\/Users\/|\/home\/|\/root\/)[A-Za-z0-9._-]+(?:\/[^\s"',}\\)\]]*)?/g },
  // macOS /private/... (symlink-realpath form of /tmp, /var, and homes)
  { name: 'macos-private', pattern: /\/private\/(?:var|tmp|Users|home|root)[^\s"',}\\)\]]*/g },
  // Windows drive-letter home directories, raw form (C:\Users\dev\...)
  { name: 'windows-drive', pattern: /[A-Za-z]:\\(?:Users|Documents and Settings|Windows\\System32\\config\\systemprofile)(?:\\[^\s"',}\\)\]]+)*/g },
  // Windows drive-letter home directories, JSON-serialized form (C:\\Users\\...)
  { name: 'windows-drive-escaped', pattern: /[A-Za-z]:\\\\(?:Users|Documents and Settings|Windows\\\\System32\\\\config\\\\systemprofile)(?:\\\\[^\s"',}\\)\]]+)*/g },
  // Windows profile env vars, raw and JSON-serialized separators
  { name: 'windows-env', pattern: /%?(?:USERPROFILE|APPDATA|LOCALAPPDATA|HOMEPATH)%?(?:\\|\\\\)[^\s"',}\\)\]]+/g },
  // UNC shares, raw (\\server\share\...) and JSON-serialized (\\\\server\\share\\...)
  { name: 'unc-share', pattern: /\\\\{2,4}[A-Za-z0-9._-]+\\{1,2}[^\s"',}\\)\]]+(?:\\{1,2}[^\s"',}\\)\]]+)*/g }
];

// A prompt may be embedded with surrounding quote characters or inside a
// longer sentence; replace every exact occurrence regardless of length
// (whitespace-only prompts are skipped — they carry nothing to leak).
function applyPrompts(text, prompts) {
  let redacted = text;
  for (const prompt of prompts) {
    if (typeof prompt !== 'string') continue;
    const trimmed = prompt.trim();
    if (trimmed === '') continue;
    if (redacted.includes(prompt)) {
      redacted = redacted.split(prompt).join(SECRET_PLACEHOLDER);
    }
    // Quoted form: "...'prompt'..." or "prompt" inside prose with matching
    // quote characters that were not part of the collected prompt.
    const quoted = new RegExp(`(['"\`])${escapeRegExp(prompt)}\\1`, 'g');
    redacted = redacted.replace(quoted, `${SECRET_PLACEHOLDER}`);
  }
  return redacted;
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function scrubString(text, prompts = []) {
  let redacted = text;
  for (const { pattern } of PATH_PATTERNS) {
    redacted = redacted.replace(pattern, PATH_PLACEHOLDER);
  }
  for (const { pattern } of CREDENTIAL_PATTERNS) {
    redacted = redacted.replace(pattern, SECRET_PLACEHOLDER);
  }
  redacted = applyPrompts(redacted, prompts);
  return redacted;
}

// Replaces sensitive values in a structure BEFORE it is serialized, so no
// sensitive string ever reaches a JSON.stringify call in the first place.
export function redactDeep(value, prompts = [], key = null) {
  if (Array.isArray(value)) {
    return value.map((entry) => redactDeep(entry, prompts, null));
  }
  if (value !== null && typeof value === 'object') {
    const redacted = {};
    for (const [childKey, childValue] of Object.entries(value)) {
      if (PROMPT_KEYS.has(childKey) && typeof childValue === 'string') {
        redacted[childKey] = SECRET_PLACEHOLDER;
      } else if (PATH_KEYS.has(childKey) && typeof childValue === 'string') {
        redacted[childKey] = /^[A-Za-z]:\\|^\/|\\\\|^%/.test(childValue)
          ? PATH_PLACEHOLDER
          : childValue; // relative workspace paths carry no user identity
      } else if (SECRET_KEYS.has(childKey) && typeof childValue === 'string') {
        redacted[childKey] = SECRET_PLACEHOLDER;
      } else {
        redacted[childKey] = redactDeep(childValue, prompts, childKey);
      }
    }
    return redacted;
  }
  if (typeof value === 'string') return scrubString(value, prompts);
  return value;
}

export function redactText(text, prompts = []) {
  return scrubString(String(text), prompts);
}

// One-shot published form: deep-redact the structure, serialize, then scrub
// the serialized text. Both layers exist on purpose — the deep pass removes
// values before serialization, the text pass catches occurrences inside
// strings that were already serialized elsewhere (logs, error messages).
export function redactPublishedText(value, prompts = []) {
  return redactText(JSON.stringify(redactDeep(value, prompts), null, 2), prompts);
}

export function redactJson(value, prompts = []) {
  return JSON.parse(redactPublishedText(value, prompts));
}
