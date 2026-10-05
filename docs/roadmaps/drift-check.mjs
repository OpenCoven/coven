#!/usr/bin/env node
// Validate the reviewed GitHub issue graph and generated roadmap without credentials.
// --issues-export PATH compares a local GitHub REST issue-array snapshot.
// Tracker state never authorizes runtime actions or certifies a release.
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
const ROADMAPS_DIR = path.dirname(fileURLToPath(import.meta.url));
const MAPPING_PATH = path.join(ROADMAPS_DIR, "coven-automations-v1.mapping.json");
const ROADMAP_PATH = path.join(ROADMAPS_DIR, "coven-automations-v1.md");
const BLOCK_BEGIN = "<!-- BEGIN GENERATED:MAPPING-TABLE v1 -- regenerate with: node docs/roadmaps/drift-check.mjs --render (do not edit by hand) -->";
const BLOCK_END = "<!-- END GENERATED:MAPPING-TABLE -->";
const PRIORITIES = new Set(["P0", "P1", "P2"]);
const REQUIRED_OUTCOME_REFS = ["OpenCoven/coven#854", "OpenCoven/coven#859", "OpenCoven/coven#816", "OpenCoven/coven#855", "OpenCoven/coven#856", "OpenCoven/coven#857", "OpenCoven/coven#858"];
const REQUIRED_CROSS_REPOSITORY_CHILD_REFS = ["OpenCoven/familiar-contract#17", "OpenCoven/coven-threads#29", "OpenCoven/sdk#80", "OpenCoven/coven-cave#5217", "OpenCoven/psyche#18", "OpenCoven/coven-docs#76", "OpenCoven/.github#2"];
function frag(...parts) {
  return parts.join("");
}

const SENSITIVE_PATTERNS = [
  {
    name: "coven_session_key",
    pattern: new RegExp(
      frag("agent:[A-Za-z0-9_-]+:(?:telegram|imessage|discord|whatsapp|", "signal|webchat):[a-z]+:\\S"),
    ),
  },
  {
    name: "messenger_chat_id",
    pattern: new RegExp(
      frag("(?:telegram|imessage|discord|whatsapp|", "signal):(?:direct:)?\\d{6,}"),
    ),
  },
  {
    name: "absolute_personal_path",
    pattern: new RegExp(frag("/", "(?:Users|home)/[A-Za-z0-9._-]+/")),
  },
  {
    name: "runtime_internal_path",
    pattern: new RegExp(frag("~/", "\\.(?:openclaw|coven)/(?:agents|workspaces|credentials|sessions)")),
  },
  {
    name: "phone_number",
    pattern: new RegExp(frag("\\+[1-9]\\d{1,14}", "(?!\\d)")),
  },
  {
    name: "credential_bearing_url",
    pattern: new RegExp(frag("ht", "tps?://\\S*(?:invite|handoff|ts\\.net)\\S*to", "ken\\S*")),
  },
];

function findSensitivePayloads(text) {
  const hits = [];
  for (const { name, pattern } of SENSITIVE_PATTERNS) {
    const match = pattern.exec(text);
    if (match !== null) {
      hits.push({ rule: name, excerpt: "<redacted: sensitive pattern matched>" });
    }
  }
  return hits;
}


function entries(mapping) {
  return [...(mapping.outcomes ?? []), ...(mapping.cross_repository_children ?? [])];
}
function ref(row) { return `${row.github?.repo}#${row.github?.issue}`; }
function finding(code, message, slug = null) { return { code, severity: "error", message, slug }; }
function validate(mapping) {
  const findings = [];
  if (mapping.schema !== "coven.automations-v1.tracker-mapping" || mapping.schema_version !== 2)
    findings.push(finding("E000", "unsupported issue-graph schema"));
  for (const [section, required] of [["outcomes", REQUIRED_OUTCOME_REFS], ["cross_repository_children", REQUIRED_CROSS_REPOSITORY_CHILD_REFS]]) {
    const actual = new Set((mapping[section] ?? []).map(ref));
    for (const expected of required) if (!actual.has(expected))
      findings.push(finding("E011", `required ${section} member ${expected} is missing or misfiled`));
  }
  const seenRefs = new Set(), bySlug = new Map();
  for (const row of entries(mapping)) {
    const id = ref(row);
    if (!/^[\w.-]+\/[\w.-]+$/.test(row.github?.repo ?? "") || !Number.isSafeInteger(row.github?.issue) || row.github.issue <= 0)
      findings.push(finding("E002", `invalid GitHub issue reference ${id}`, row.slug));
    if (seenRefs.has(id)) findings.push(finding("E001", `duplicate issue ${id}`, row.slug));
    seenRefs.add(id);
    if (!row.slug || bySlug.has(row.slug)) findings.push(finding("E002", `invalid or duplicate slug ${row.slug}`, row.slug));
    bySlug.set(row.slug, row);
    if (!PRIORITIES.has(row.github?.priority)) findings.push(finding("E005", `invalid priority for ${id}`, row.slug));
    if (!["open", "closed"].includes(row.github?.state)) findings.push(finding("E002", `invalid issue state for ${id}`, row.slug));
    if (row.github?.priority === "P0" && (!row.github.owner || !row.acceptance_gate || !row.disposition))
      findings.push(finding("E006", `P0 ${id} lacks owner, acceptance gate, or disposition`, row.slug));
    const closed = row.github?.state === "closed";
    if (closed !== /^(closed|complete|done)/i.test(row.disposition ?? ""))
      findings.push(finding("E012", `issue state and disposition disagree for ${id}`, row.slug));
    if (closed && !(row.evidence?.length > 0)) findings.push(finding("E007", `closed ${id} lacks delivery evidence`, row.slug));
    if (!Array.isArray(row.depends_on)) findings.push(finding("E003", `dependencies for ${id} must be an array`, row.slug));
  }
  const state = new Map();
  function visit(slug) {
    if (state.get(slug) === "visiting") { findings.push(finding("E004", `dependency cycle at ${slug}`, slug)); return; }
    if (state.get(slug) === "done") return;
    state.set(slug, "visiting");
    for (const dep of bySlug.get(slug)?.depends_on ?? []) {
      if (!bySlug.has(dep)) findings.push(finding("E003", `unknown prerequisite ${dep}`, slug));
      else visit(dep);
    }
    state.set(slug, "done");
  }
  for (const slug of bySlug.keys()) visit(slug);
  for (const hit of findSensitivePayloads(JSON.stringify(mapping))) findings.push(finding("E009", `sensitive mapping payload: ${hit.rule}`));
  return findings;
}
function renderMappingTable(mapping) {
  const lines = ["| Outcome | GitHub | Priority | Dependencies | Disposition |", "| --- | --- | --- | --- | --- |"];
  for (const row of entries(mapping)) {
    const issue = row.github.url ? `[${ref(row)}](${row.github.url})` : `\`${ref(row)}\``;
    lines.push(`| ${row.slug} | ${issue} | ${row.github.priority} | ${row.depends_on.join(", ") || "(none)"} | ${row.disposition} |`);
  }
  return lines.join("\n");
}
function extractBlock(text) {
  const begin = text.indexOf(BLOCK_BEGIN), end = text.indexOf(BLOCK_END);
  return begin < 0 || end < begin ? null : text.slice(begin + BLOCK_BEGIN.length, end).trim();
}
function compareIssues(mapping, issueText) {
  let issues;
  try { issues = JSON.parse(issueText); } catch { return [finding("E100", "issue export is not valid JSON")]; }
  if (!Array.isArray(issues)) return [finding("E100", "issue export must be a GitHub REST JSON array")];
  const findings = [], indexed = new Map();
  for (const issue of issues) {
    if (issue.pull_request) continue;
    const match = /^https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/issues\/(\d+)$/.exec(issue.html_url ?? "");
    if (!match || Number(match[2]) !== issue.number || !["open", "closed"].includes(issue.state)) {
      findings.push(finding("E100", "issue export contains an invalid issue identity or state")); continue;
    }
    const id = `${match[1]}#${issue.number}`;
    if (indexed.has(id)) findings.push(finding("E101", `duplicate issue snapshot ${id}`));
    indexed.set(id, issue);
  }
  for (const row of entries(mapping)) {
    const actual = indexed.get(ref(row));
    if (!actual) findings.push(finding("E101", `missing issue snapshot for ${ref(row)}`, row.slug));
    else if (actual.state !== row.github.state) findings.push(finding("E102", `state drift for ${ref(row)}`, row.slug));
  }
  return findings;
}
function analyze(mapping, text, issueText) {
  const findings = validate(mapping);
  if (extractBlock(text) !== renderMappingTable(mapping)) findings.push(finding("E008", "generated issue table drift; run --render"));
  for (const hit of findSensitivePayloads(text)) findings.push(finding("E009", `sensitive roadmap payload: ${hit.rule}`));
  if (issueText !== undefined) findings.push(...compareIssues(mapping, issueText));
  return findings;
}
function selftest(mapping, text) {
  const clone = () => structuredClone(mapping);
  const assertCode = (m, code) => { if (!validate(m).some(f => f.code === code)) throw new Error(`missing detection ${code}`); };
  if (analyze(mapping, text).length) throw new Error("committed issue graph is not clean");
  let m = clone(); m.outcomes = m.outcomes.filter(r => r.github.issue !== 859); assertCode(m, "E011");
  m = clone(); m.cross_repository_children = []; assertCode(m, "E011");
  m = clone(); m.outcomes.push(m.cross_repository_children.pop()); assertCode(m, "E011");
  m = clone(); m.outcomes[1].github = structuredClone(m.outcomes[0].github); assertCode(m, "E001");
  m = clone(); m.outcomes[1].slug = m.outcomes[0].slug; assertCode(m, "E002");
  m = clone(); m.outcomes[0].github.issue = 0; assertCode(m, "E002");
  m = clone(); m.outcomes[0].depends_on.push("missing"); assertCode(m, "E003");
  m = clone(); m.outcomes[0].depends_on.push(m.outcomes[0].slug); assertCode(m, "E004");
  m = clone(); m.outcomes[0].github.priority = "P9"; assertCode(m, "E005");
  m = clone(); m.outcomes[0].github.owner = null; assertCode(m, "E006");
  m = clone(); m.outcomes.find(r => r.github.state === "closed").evidence = []; assertCode(m, "E007");
  m = clone(); m.cross_repository_children.find(r => r.github.state === "closed").evidence = []; assertCode(m, "E007");
  m = clone(); m.outcomes[0].disposition = "closed"; assertCode(m, "E012");
  m = clone(); m.outcomes[0].notes = frag("agent:x", ":telegram:direct:", "PRIVATE"); assertCode(m, "E009");
  if (!analyze(mapping, "missing markers").some(f => f.code === "E008")) throw new Error("missing table-drift detection");
  const issues = entries(mapping).map(r => ({number:r.github.issue, html_url:`https://github.com/${r.github.repo}/issues/${r.github.issue}`, state:r.github.state}));
  if (compareIssues(mapping, JSON.stringify(issues)).length) throw new Error("valid GitHub snapshot rejected");
  for (const [value, code] of [["{", "E100"], ["{}", "E100"], ["[]", "E101"], [JSON.stringify([...issues, issues[0]]), "E101"], [JSON.stringify(issues.map((r,i) => i === 0 ? {...r,state:"closed"} : r)), "E102"]])
    if (!compareIssues(mapping, value).some(f => f.code === code)) throw new Error(`missing snapshot detection ${code}`);
  console.log("drift-check: issue graph and GitHub snapshot selftests passed");
}
function main(args) {
  const allowed = new Set(["--strict", "--render", "--selftest", "--issues-export"]);
  let issueText;
  for (let i = 0; i < args.length; i++) {
    if (!allowed.has(args[i])) throw new Error(`unknown option ${args[i]}`);
    if (args[i] === "--issues-export") {
      const file = args[++i]; if (!file || file.startsWith("--")) throw new Error("--issues-export requires a path");
      issueText = fs.readFileSync(path.resolve(file), "utf8");
    }
  }
  const mapping = JSON.parse(fs.readFileSync(MAPPING_PATH, "utf8"));
  const text = fs.readFileSync(ROADMAP_PATH, "utf8");
  if (args.includes("--selftest")) { selftest(mapping, text); return; }
  if (args.includes("--render")) {
    const findings = validate(mapping); if (findings.length) throw new Error(JSON.stringify(findings));
    const begin = text.indexOf(BLOCK_BEGIN), end = text.indexOf(BLOCK_END);
    if (begin < 0 || end < begin) throw new Error("generated table markers missing");
    fs.writeFileSync(ROADMAP_PATH, text.slice(0,begin) + BLOCK_BEGIN + "\n" + renderMappingTable(mapping) + "\n" + text.slice(end));
    console.log("drift-check: regenerated issue table"); return;
  }
  const findings = analyze(mapping, text, issueText);
  for (const f of findings) console.error(`${f.code}: ${f.message}`);
  if (findings.length) process.exitCode = 1;
  else console.log("drift-check: no findings");
}
try { main(process.argv.slice(2)); } catch (error) { console.error(`drift-check: ${error.message}`); process.exitCode = 2; }
