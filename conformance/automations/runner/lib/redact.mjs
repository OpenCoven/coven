// Redaction for everything the conformance plane publishes: definition
// prompts, secrets, private memory markers, and irrelevant absolute user
// paths never reach a report, a receipt, or a doctor finding.

export const REDACTION_RULES = [
  'definition prompts replaced with [redacted]',
  'absolute user paths replaced with [redacted-path]'
];

export function redactText(text, prompts) {
  let redacted = text;
  for (const prompt of prompts) {
    if (prompt && prompt.length >= 4) {
      redacted = redacted.split(prompt).join('[redacted]');
    }
  }
  redacted = redacted.replace(/(\/Users\/|\/home\/)[A-Za-z0-9._-]+\//g, '[redacted-path]/');
  return redacted;
}

export function redactJson(value, prompts) {
  return JSON.parse(redactText(JSON.stringify(value), prompts));
}
