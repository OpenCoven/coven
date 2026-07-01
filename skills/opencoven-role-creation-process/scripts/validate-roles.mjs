#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const home = os.homedir();
const covenHome = process.env.COVEN_HOME || path.join(home, ".coven");
const familiarIds = process.argv.slice(2);

function expandHome(value) {
  if (value === "~") return home;
  if (value.startsWith("~/")) return path.join(home, value.slice(2));
  return value;
}

function readTomlString(block, key) {
  const quoted = block.match(new RegExp(`^\\s*${key}\\s*=\\s*(['"])(.*?)\\1\\s*(?:#.*)?$`, "m"));
  if (quoted) return quoted[2];
  const bare = block.match(new RegExp(`^\\s*${key}\\s*=\\s*([^\\s#]+)\\s*(?:#.*)?$`, "m"));
  return bare?.[1] ?? null;
}

function familiarWorkspaces() {
  const file = path.join(covenHome, "familiars.toml");
  const raw = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "";
  const workspaces = new Map();
  const ids = [];
  for (const block of raw.split(/^\s*\[\[familiar\]\]\s*$/m).slice(1)) {
    const id = readTomlString(block, "id");
    const workspace = readTomlString(block, "workspace");
    if (!id) continue;
    ids.push(id);
    if (workspace) workspaces.set(id, path.resolve(expandHome(workspace)));
  }
  return { ids, workspaces };
}

function frontmatter(text) {
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---/);
  if (!match) return {};
  const result = {};
  for (const line of match[1].split("\n")) {
    const m = line.match(/^(\w[\w-]*):\s+"?([^"]*)"?\s*$/);
    if (m) result[m[1]] = m[2];
  }
  return result;
}

function listField(text, field) {
  const match = text.match(new RegExp(`\\n${field}:\\s*\\n((?:\\s*-[^\\n]*\\n?)*)`));
  if (!match) return [];
  return match[1].match(/- (.+)/g)?.map((m) => m.slice(2).trim()) ?? [];
}

const { ids, workspaces } = familiarWorkspaces();
const selected = familiarIds.length ? familiarIds : ids;
const errors = [];
const roles = [];

for (const familiar of selected) {
  const workspace = workspaces.get(familiar) || path.join(covenHome, "familiars", familiar);
  const rolesDir = path.join(workspace, "roles");
  if (!fs.existsSync(rolesDir)) continue;
  for (const entry of fs.readdirSync(rolesDir, { withFileTypes: true })) {
    const roleId = entry.name;
    const roleDir = path.join(rolesDir, roleId);
    if (!fs.existsSync(roleDir) || !fs.statSync(roleDir).isDirectory()) continue;
    const roleFile = path.join(roleDir, "ROLE.md");
    if (!fs.existsSync(roleFile)) continue;
    const text = fs.readFileSync(roleFile, "utf8");
    const fm = frontmatter(text);
    for (const field of ["name", "id", "version", "description", "familiar"]) {
      if (!fm[field]) errors.push(`${familiar}:${roleId} missing ${field}`);
    }
    if (!/Relationship To SOUL\.md|Relationship to SOUL\.md/.test(text)) {
      errors.push(`${familiar}:${roleId} missing SOUL.md relationship section`);
    }
    for (const workflow of listField(text, "workflows")) {
      const workflowFile = path.join(roleDir, "workflows", `${workflow}.md`);
      if (!fs.existsSync(workflowFile)) errors.push(`${familiar}:${roleId} missing workflow ${workflow}`);
    }
    roles.push(`${familiar}:${roleId}`);
  }
}

if (errors.length) {
  console.error(`roles_invalid count=${roles.length}`);
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}

console.log(`roles_ok count=${roles.length}`);
for (const role of roles.sort()) console.log(role);
