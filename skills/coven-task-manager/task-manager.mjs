#!/usr/bin/env node

import { lstat, mkdir, readFile, symlink, unlink, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const DEFAULT_COVEN_HOME = join(homedir(), ".coven");
const DEFAULT_CODEX_HOME = join(homedir(), ".codex");

const DEFAULT_AUTOMATIONS = [
  {
    id: "coven-task-freshness-daily",
    name: "Coven Task Freshness Daily",
    rrule: "RRULE:FREQ=WEEKLY;BYHOUR=8;BYMINUTE=30;BYDAY=SU,MO,TU,WE,TH,FR,SA",
    prompt: `Use the \`coven-task-manager\` skill to run the daily Coven task freshness sweep.

Goals:
- Load the current Cave task board from the local Coven home.
- Identify stale running tasks, blocked tasks that need human attention, review-ready work, and stale backlog/inbox items.
- Build the Thread Coordination map before resuming or spawning work.
- Update the task freshness report.
- Only edit task state when there is concrete evidence from the board, linked sessions, git, CI, or explicit user instructions.

Deliverable:
- A concise task freshness summary with next actions and any task cards that need human attention.`,
  },
  {
    id: "coven-task-blocked-escalation",
    name: "Coven Blocked Task Escalation",
    rrule: "RRULE:FREQ=WEEKLY;BYHOUR=10;BYMINUTE=0;BYDAY=MO,TU,WE,TH,FR",
    prompt: `Use the \`coven-task-manager\` skill to review blocked Coven tasks.

Goals:
- Find blocked cards, cards marked needsHuman, and stale running cards.
- Check the Thread Coordination map for duplicated sessions, overloaded familiars, or repo/branch conflicts.
- Separate real blockers from stale bookkeeping.
- Prepare a short escalation list with owners, evidence, and the smallest next unblock action.
- Do not close, delete, or mark tasks done without concrete evidence.`,
  },
  {
    id: "coven-task-weekly-cleanup",
    name: "Coven Weekly Task Cleanup",
    rrule: "RRULE:FREQ=WEEKLY;BYHOUR=17;BYMINUTE=0;BYDAY=FR",
    prompt: `Use the \`coven-task-manager\` skill for the weekly Coven task-board cleanup.

Goals:
- Summarize done, review, blocked, active, inbox, and backlog cards.
- Summarize simultaneous active/review/blocked threads by session, familiar, repo, and branch.
- Detect duplicate or stale cards.
- Suggest archive/delete candidates, but do not perform destructive cleanup unless explicitly approved.
- Update the task freshness report with changes since the last run.`,
  },
];

function expandHome(input) {
  if (!input || input === "~") return homedir();
  if (input.startsWith("~/")) return join(homedir(), input.slice(2));
  return input;
}

function ageHours(iso, now) {
  if (!iso) return 0;
  const time = new Date(iso).getTime();
  if (!Number.isFinite(time)) return 0;
  return Math.max(0, (now.getTime() - time) / 36e5);
}

function priorityRank(card) {
  return { urgent: 0, high: 1, medium: 2, low: 3 }[card.priority] ?? 4;
}

function compareCards(a, b) {
  return priorityRank(a) - priorityRank(b) || (a.updatedAt ?? "").localeCompare(b.updatedAt ?? "");
}

function isOpenCoordinationCard(card) {
  if (card.status === "done" || card.lifecycle === "completed") return false;
  return ["running", "blocked", "review"].includes(card.status)
    || ["running", "dispatched", "review", "failed"].includes(card.lifecycle);
}

function titleList(cards, now) {
  return cards.slice(0, 4).map((card) => cardLine(card, now)).join("\n");
}

function addGroup(groups, key, title, cards, action) {
  if (cards.length < 2) return;
  groups.push({ key, title, cards: [...cards].sort(compareCards), action });
}

function groupBy(items, keyFn) {
  const grouped = new Map();
  for (const item of items) {
    const key = keyFn(item);
    const bucket = grouped.get(key);
    if (bucket) bucket.push(item);
    else grouped.set(key, [item]);
  }
  return grouped;
}

function extractBranch(card) {
  const haystack = `${card.title ?? ""}\n${card.notes ?? ""}`;
  const match = haystack.match(/\b(?:branch|head)\s*[:=]\s*([A-Za-z0-9._/-]+)/i)
    ?? haystack.match(/\b(?:on|from)\s+([A-Za-z0-9._/-]+)\s+branch\b/i);
  return match?.[1]?.replace(/[),.;]+$/, "") ?? null;
}

function extractRepo(card) {
  const labels = Array.isArray(card.labels) ? card.labels.map((label) => String(label).toLowerCase()) : [];
  const projectLabel = labels.find((label) =>
    ["cave", "coven", "openclaw", "openmeow", "opentrust", "covencave"].includes(label),
  );
  if (projectLabel) return projectLabel === "covencave" ? "cave" : projectLabel;

  const haystack = `${card.title ?? ""}\n${card.notes ?? ""}`;
  const gh = haystack.match(/github\.com\/(?:OpenCoven|BunsDev)\/([A-Za-z0-9._-]+)/i);
  if (gh) return gh[1].toLowerCase();
  const path = haystack.match(/\/OpenCoven\/([A-Za-z0-9._-]+)/i);
  return path?.[1]?.toLowerCase() ?? null;
}

export function analyzeThreadCoordination(cards) {
  const open = cards.filter(isOpenCoordinationCard);
  const groups = [];

  const bySession = groupBy(open.filter((card) => card.sessionId), (card) => card.sessionId);
  for (const [sessionId, group] of bySession) {
    addGroup(
      groups,
      `session:${sessionId}`,
      `Shared session ${sessionId}`,
      group,
      "Resume the existing session once, then update or park every linked card from that evidence.",
    );
  }

  const byFamiliar = groupBy(open.filter((card) => card.familiarId), (card) => card.familiarId);
  for (const [familiarId, group] of byFamiliar) {
    addGroup(
      groups,
      `familiar:${familiarId}`,
      `Concurrent lanes for @${familiarId}`,
      group,
      "Pick one primary lane for the next action; leave the others with explicit wait/review/blocker notes.",
    );
  }

  const withRepos = open
    .map((card) => [extractRepo(card), card])
    .filter(([repo]) => repo);
  const byRepo = groupBy(withRepos, ([repo]) => repo);
  for (const [repo, pairs] of byRepo) {
    addGroup(
      groups,
      `repo:${repo}`,
      `Repo collision: ${repo}`,
      pairs.map(([, card]) => card),
      "Verify branch/worktree ownership before allowing simultaneous writes in this repo.",
    );
  }

  const withBranches = open
    .map((card) => [extractBranch(card), card])
    .filter(([branch]) => branch);
  const byBranch = groupBy(withBranches, ([branch]) => branch);
  for (const [branch, pairs] of byBranch) {
    addGroup(
      groups,
      `branch:${branch}`,
      `Branch collision: ${branch}`,
      pairs.map(([, card]) => card),
      "Do not run these in parallel; choose the freshest thread and reconcile the rest into it.",
    );
  }

  groups.sort((a, b) => b.cards.length - a.cards.length || a.title.localeCompare(b.title));
  return groups;
}

export async function loadBoard({ covenHome = DEFAULT_COVEN_HOME } = {}) {
  const boardPath = join(expandHome(covenHome), "cave-board.json");
  try {
    const raw = await readFile(boardPath, "utf8");
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed.cards) ? parsed.cards : [];
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
}

export function classifyTasks(cards, { now = new Date(), staleRunningHours = 4 } = {}) {
  const staleRunning = [];
  const blocked = [];
  const review = [];
  const active = [];
  const done = [];

  for (const card of cards) {
    if (card.status === "done" || card.lifecycle === "completed") {
      done.push(card);
      continue;
    }
    if (card.status === "blocked" || card.needsHuman || card.lifecycle === "failed") {
      blocked.push(card);
      continue;
    }
    if (card.status === "review" || card.lifecycle === "review") {
      review.push(card);
      continue;
    }
    if (card.status === "running" || card.lifecycle === "running" || card.lifecycle === "dispatched") {
      const basis = card.runningSince ?? card.lifecycleAt ?? card.updatedAt;
      if (ageHours(basis, now) >= staleRunningHours) staleRunning.push(card);
      else active.push(card);
      continue;
    }
    if (card.status === "inbox" || card.status === "backlog" || card.lifecycle === "queued") {
      active.push(card);
    }
  }

  staleRunning.sort(compareCards);
  blocked.sort(compareCards);
  review.sort(compareCards);
  active.sort(compareCards);
  done.sort(compareCards);

  return {
    staleRunning,
    blocked,
    review,
    active,
    done,
    counts: {
      total: cards.length,
      staleRunning: staleRunning.length,
      blocked: blocked.length,
      review: review.length,
      active: active.length,
      done: done.length,
    },
  };
}

function cardLine(card, now) {
  const owner = card.familiarId ? ` @${card.familiarId}` : "";
  const priority = card.priority ? ` [${card.priority}]` : "";
  const age = card.updatedAt ? ` updated ${Math.round(ageHours(card.updatedAt, now))}h ago` : "";
  return `- ${card.title}${priority}${owner}${age} (${card.id})`;
}

function section(title, cards, now, empty) {
  const lines = [`## ${title}`];
  if (cards.length === 0) lines.push(empty);
  else lines.push(...cards.map((card) => cardLine(card, now)));
  return lines.join("\n");
}

function threadCoordinationSection(cards, now) {
  const groups = analyzeThreadCoordination(cards);
  const lines = ["## Thread Coordination"];
  if (groups.length === 0) {
    lines.push("No obvious simultaneous-thread conflicts.");
    return lines.join("\n");
  }

  for (const group of groups.slice(0, 8)) {
    lines.push(`### ${group.title}`);
    lines.push(group.action);
    lines.push(titleList(group.cards, now));
    lines.push("");
  }
  return lines.join("\n").trimEnd();
}

export function buildTaskFreshnessReport(cards, { now = new Date(), staleRunningHours = 4 } = {}) {
  const classified = classifyTasks(cards, { now, staleRunningHours });
  const coordinationGroups = analyzeThreadCoordination(cards);
  const date = now.toISOString().slice(0, 10);
  const lines = [
    `# Coven Task Freshness - ${date}`,
    "",
    "## Summary",
    `- Total: ${classified.counts.total}`,
    `- Stale running: ${classified.counts.staleRunning}`,
    `- Needs human: ${classified.counts.blocked}`,
    `- Review: ${classified.counts.review}`,
    `- Active: ${classified.counts.active}`,
    `- Done: ${classified.counts.done}`,
    `- Thread coordination groups: ${coordinationGroups.length}`,
    "",
    threadCoordinationSection(cards, now),
    "",
    section("Stale Running", classified.staleRunning, now, "None."),
    "",
    section("Needs Human", classified.blocked, now, "None."),
    "",
    section("Ready For Review", classified.review, now, "None."),
    "",
    section("Next Actions", [...classified.staleRunning, ...classified.blocked, ...classified.review].slice(0, 10), now, "No immediate task-management action needed."),
    "",
  ];
  return `${lines.join("\n")}\n`;
}

export async function writeTaskFreshnessReport({
  covenHome = DEFAULT_COVEN_HOME,
  out,
  now = new Date(),
  staleRunningHours = 4,
} = {}) {
  const cards = await loadBoard({ covenHome });
  const report = buildTaskFreshnessReport(cards, { now, staleRunningHours });
  const outPath = resolve(
    expandHome(out ?? join(covenHome, "task-manager", "freshness-report.md")),
  );
  await mkdir(dirname(outPath), { recursive: true });
  await writeFile(outPath, report, "utf8");
  return { path: outPath, report };
}

function tomlString(value) {
  return JSON.stringify(value);
}

function tomlMultiline(value) {
  return `'''${value.replace(/'''/g, "'''\"'''\"'''")}'''`;
}

function automationToml(template, { skillPath, status }) {
  const normalizedStatus = normalizeAutomationStatus(status);
  return [
    "version = 1",
    `id = ${tomlString(template.id)}`,
    'kind = "cron"',
    `name = ${tomlString(template.name)}`,
    `prompt = ${tomlMultiline(template.prompt)}`,
    `status = ${tomlString(normalizedStatus)}`,
    `rrule = ${tomlString(template.rrule)}`,
    'reasoning_effort = "high"',
    'execution_environment = "worktree"',
    "cwds = []",
    'tags = ["coven", "tasks", "freshness"]',
    `skill_path = ${tomlString(skillPath)}`,
    "",
  ].join("\n");
}

export async function installDefaultAutomations({
  codexHome = DEFAULT_CODEX_HOME,
  skillPath = HERE,
  status = "PAUSED",
} = {}) {
  const root = join(expandHome(codexHome), "automations");
  const normalizedStatus = normalizeAutomationStatus(status);
  const installed = [];
  for (const template of DEFAULT_AUTOMATIONS) {
    const dir = join(root, template.id);
    await mkdir(dir, { recursive: true });
    const toml = automationToml(template, { skillPath, status: normalizedStatus });
    await writeFile(join(dir, "automation.toml"), toml, "utf8");
    installed.push({ id: template.id, path: join(dir, "automation.toml") });
  }
  return installed;
}

export async function installSkillSymlink({
  covenHome = DEFAULT_COVEN_HOME,
  skillPath = HERE,
  replace = false,
} = {}) {
  const target = join(expandHome(covenHome), "skills", "coven-task-manager");
  await mkdir(dirname(target), { recursive: true });
  if (existsSync(target)) {
    if (!replace) return { path: target, changed: false };
    const current = await lstat(target);
    if (!current.isSymbolicLink()) {
      throw new Error(`refusing to replace non-symlink skill install at ${target}`);
    }
    await unlink(target);
  }
  await symlink(resolve(skillPath), target, "dir");
  return { path: target, changed: true };
}

function parseArgs(argv) {
  const args = { _: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const item = argv[i];
    if (!item.startsWith("--")) {
      args._.push(item);
      continue;
    }
    const key = item.slice(2);
    const next = argv[i + 1];
    args[key] = next && !next.startsWith("--") ? argv[++i] : "true";
  }
  return args;
}

export function parseStaleRunningHours(value, fallback = 4) {
  const parsed = Number(value ?? fallback);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

export function normalizeAutomationStatus(value, fallback = "PAUSED") {
  const status = String(value ?? fallback).trim().toUpperCase();
  return status === "PAUSED" || status === "ACTIVE" ? status : fallback;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const command = args._[0];
  if (command === "report") {
    const result = await writeTaskFreshnessReport({
      covenHome: args["coven-home"] ?? DEFAULT_COVEN_HOME,
      out: args.out,
      staleRunningHours: parseStaleRunningHours(args["stale-running-hours"]),
    });
    process.stdout.write(`${result.report}\nWrote ${result.path}\n`);
    return;
  }
  if (command === "install-default-automations") {
    const installed = await installDefaultAutomations({
      codexHome: args["codex-home"] ?? DEFAULT_CODEX_HOME,
      skillPath: args["skill-path"] ?? HERE,
      status: args.status ?? "PAUSED",
    });
    process.stdout.write(JSON.stringify({ installed }, null, 2) + "\n");
    return;
  }
  if (command === "install-local") {
    const skill = await installSkillSymlink({
      covenHome: args["coven-home"] ?? DEFAULT_COVEN_HOME,
      skillPath: args["skill-path"] ?? HERE,
      replace: args.replace === "true",
    });
    const automations = await installDefaultAutomations({
      codexHome: args["codex-home"] ?? DEFAULT_CODEX_HOME,
      skillPath: skill.path,
      status: args.status ?? "PAUSED",
    });
    process.stdout.write(JSON.stringify({ skill, automations }, null, 2) + "\n");
    return;
  }

  process.stderr.write(
    "Usage: task-manager.mjs report|install-default-automations|install-local [--status PAUSED|ACTIVE]\n",
  );
  process.exit(1);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`${error?.stack ?? error}\n`);
    process.exit(1);
  });
}
