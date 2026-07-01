#!/usr/bin/env node
import { cp, mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const ROOT = path.resolve(new URL("..", import.meta.url).pathname);
const TARGET_ROOT = path.join(ROOT, "skills");
const DEFAULT_SOURCE = path.join(process.env.HOME ?? "", ".openclaw", "workspace", "skills");
const SOURCE_ROOT = path.resolve(process.env.OPENCLAW_SKILLS_DIR ?? DEFAULT_SOURCE);
const MANIFEST_PATH = path.join(TARGET_ROOT, "openclaw-skills-manifest.json");
const IGNORED_NAMES = new Set([
  ".DS_Store",
  ".clawhub",
  ".git",
  ".scan-reports",
  "CHANGELOG.md",
  "_meta.json",
  "dist",
  "node_modules",
  "official-prompt-guide.md",
]);
const EXISTING_CANONICAL_SKILLS = new Set(["opencoven-design"]);

function parseArgs(argv) {
  const args = new Set(argv);
  return {
    check: args.has("--check"),
    help: args.has("--help") || args.has("-h"),
  };
}

async function sourceSkillNames() {
  const entries = await readdir(SOURCE_ROOT, { withFileTypes: true });
  const names = [];
  for (const entry of entries) {
    if (IGNORED_NAMES.has(entry.name)) continue;
    const skillPath = path.join(SOURCE_ROOT, entry.name);
    if (!entry.isDirectory() && !entry.isSymbolicLink()) continue;
    if (existsSync(path.join(skillPath, "SKILL.md"))) names.push(entry.name);
  }
  names.sort((a, b) => a.localeCompare(b));
  return names;
}

function descriptionFromFrontmatter(markdown) {
  const match = markdown.match(/^---\n([\s\S]*?)\n---/);
  if (!match) return "";
  const lines = match[1].split("\n");
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const simple = line.match(/^description:\s*(["']?)(.*?)\1\s*$/);
    if (simple && simple[2] !== ">" && simple[2] !== "|") return simple[2].trim();
    if (/^description:\s*[>|]\s*$/.test(line)) {
      const block = [];
      for (const next of lines.slice(i + 1)) {
        if (!/^\s+/.test(next)) break;
        block.push(next.trim());
      }
      return block.join(" ").replace(/\s+/g, " ").trim();
    }
  }
  return "";
}

async function manifestFor(names) {
  const skills = [];
  for (const name of names) {
    const skillMarkdown = await readFile(path.join(SOURCE_ROOT, name, "SKILL.md"), "utf8");
    const canonical = EXISTING_CANONICAL_SKILLS.has(name) && existsSync(path.join(TARGET_ROOT, name, "SKILL.md"));
    skills.push({
      name,
      description: descriptionFromFrontmatter(skillMarkdown),
      source: "openclaw-workspace",
      sourcePath: `\${OPENCLAW_SKILLS_DIR:-~/.openclaw/workspace/skills}/${name}`,
      targetPath: `skills/${name}`,
      migrationStatus: canonical ? "represented-by-existing-coven-skill" : "copied-from-openclaw-workspace",
      managedBySync: !canonical,
    });
  }
  return {
    schemaVersion: "opencoven.openclaw-skills.v1",
    generatedBy: "scripts/sync-openclaw-skills.mjs",
    sourceEnvVar: "OPENCLAW_SKILLS_DIR",
    defaultSource: "~/.openclaw/workspace/skills",
    skillCount: skills.length,
    skills,
  };
}

async function listFiles(root) {
  const files = [];
  async function walk(dir, relPrefix = "") {
    const entries = await readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      if (IGNORED_NAMES.has(entry.name)) continue;
      const abs = path.join(dir, entry.name);
      const rel = path.join(relPrefix, entry.name);
      if (entry.isDirectory()) {
        await walk(abs, rel);
      } else if (entry.isSymbolicLink()) {
        const real = await stat(abs);
        if (real.isDirectory()) await walk(abs, rel);
        else files.push(rel);
      } else if (entry.isFile()) {
        files.push(rel);
      }
    }
  }
  await walk(root);
  return files.sort((a, b) => a.localeCompare(b));
}

async function checkSkill(name) {
  const sourceDir = path.join(SOURCE_ROOT, name);
  const targetDir = path.join(TARGET_ROOT, name);
  if (!existsSync(targetDir)) return [`missing skills/${name}`];
  if (EXISTING_CANONICAL_SKILLS.has(name)) return [];
  const sourceFiles = await listFiles(sourceDir);
  const targetFiles = await listFiles(targetDir);
  const problems = [];
  if (sourceFiles.join("\n") !== targetFiles.join("\n")) {
    problems.push(`file list drift for skills/${name}`);
    return problems;
  }
  for (const rel of sourceFiles) {
    const [sourceBytes, targetBytes] = await Promise.all([
      readFile(path.join(sourceDir, rel)),
      readFile(path.join(targetDir, rel)),
    ]);
    if (!sourceBytes.equals(targetBytes)) problems.push(`content drift for skills/${name}/${rel}`);
  }
  return problems;
}

async function syncSkill(name) {
  if (EXISTING_CANONICAL_SKILLS.has(name) && existsSync(path.join(TARGET_ROOT, name, "SKILL.md"))) return;
  const sourceDir = path.join(SOURCE_ROOT, name);
  const targetDir = path.join(TARGET_ROOT, name);
  await rm(targetDir, { recursive: true, force: true });
  await cp(sourceDir, targetDir, {
    recursive: true,
    dereference: true,
    filter: (src) => !IGNORED_NAMES.has(path.basename(src)),
  });
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    console.log("Usage: node scripts/sync-openclaw-skills.mjs [--check]");
    console.log("Set OPENCLAW_SKILLS_DIR to override ~/.openclaw/workspace/skills.");
    return 0;
  }

  if (!existsSync(SOURCE_ROOT)) {
    console.error(`OpenClaw skills source not found: ${SOURCE_ROOT}`);
    return 1;
  }

  const names = await sourceSkillNames();
  if (names.length === 0) {
    console.error(`No OpenClaw skills found in ${SOURCE_ROOT}`);
    return 1;
  }

  const manifest = await manifestFor(names);
  const manifestJson = `${JSON.stringify(manifest, null, 2)}\n`;

  if (opts.check) {
    const manifestActual = existsSync(MANIFEST_PATH) ? await readFile(MANIFEST_PATH, "utf8") : "";
    const problems = [];
    if (manifestActual !== manifestJson) problems.push("stale skills/openclaw-skills-manifest.json");
    for (const name of names) problems.push(...(await checkSkill(name)));
    if (problems.length > 0) {
      for (const problem of problems) console.error(problem);
      return 1;
    }
    console.log(`openclaw_skills_ok count=${names.length}`);
    return 0;
  }

  await mkdir(TARGET_ROOT, { recursive: true });
  for (const name of names) await syncSkill(name);
  await writeFile(MANIFEST_PATH, manifestJson, "utf8");
  console.log(`openclaw_skills_synced count=${names.length}`);
  return 0;
}

main().then((code) => {
  process.exitCode = code;
}).catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
