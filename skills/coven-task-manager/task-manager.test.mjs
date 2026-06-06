import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

import {
  analyzeThreadCoordination,
  buildTaskFreshnessReport,
  classifyTasks,
  installDefaultAutomations,
  installSkillSymlink,
  loadBoard,
  normalizeAutomationStatus,
  parseStaleRunningHours,
} from "./task-manager.mjs";

test("classifyTasks separates stale running, blocked, review, active, and done cards", () => {
  const now = new Date("2026-06-06T12:00:00.000Z");
  const cards = [
    {
      id: "stale-1",
      title: "Long-running implementation",
      status: "running",
      priority: "urgent",
      familiarId: "cody",
      updatedAt: "2026-06-06T00:00:00.000Z",
      runningSince: "2026-06-06T00:00:00.000Z",
    },
    {
      id: "blocked-1",
      title: "Needs API decision",
      status: "blocked",
      priority: "high",
      familiarId: "sage",
      updatedAt: "2026-06-06T10:00:00.000Z",
      needsHuman: true,
    },
    {
      id: "review-1",
      title: "Check PR fallout",
      status: "review",
      priority: "medium",
      updatedAt: "2026-06-06T11:00:00.000Z",
    },
    {
      id: "active-1",
      title: "Fresh run",
      status: "running",
      priority: "medium",
      updatedAt: "2026-06-06T11:30:00.000Z",
      runningSince: "2026-06-06T11:30:00.000Z",
    },
    {
      id: "done-1",
      title: "Merged fix",
      status: "done",
      priority: "low",
      updatedAt: "2026-06-06T09:00:00.000Z",
    },
  ];

  const result = classifyTasks(cards, { now, staleRunningHours: 4 });

  assert.deepEqual(result.counts, {
    total: 5,
    staleRunning: 1,
    blocked: 1,
    review: 1,
    active: 1,
    done: 1,
  });
  assert.equal(result.staleRunning[0].id, "stale-1");
  assert.equal(result.blocked[0].id, "blocked-1");
  assert.equal(result.review[0].id, "review-1");
});

test("analyzeThreadCoordination groups simultaneous sessions and repo lanes", () => {
  const groups = analyzeThreadCoordination([
    {
      id: "one",
      title: "Cave editor",
      status: "running",
      familiarId: "cody",
      sessionId: "sess-1",
      labels: ["cave"],
      notes: "branch: cody/editor",
    },
    {
      id: "two",
      title: "Cave API",
      status: "review",
      familiarId: "cody",
      sessionId: "sess-1",
      labels: ["cave"],
      notes: "branch: cody/editor",
    },
    {
      id: "three",
      title: "Finished",
      status: "done",
      familiarId: "cody",
      sessionId: "sess-1",
      labels: ["cave"],
    },
  ]);

  assert.ok(groups.some((group) => group.key === "session:sess-1"));
  assert.ok(groups.some((group) => group.key === "familiar:cody"));
  assert.ok(groups.some((group) => group.key === "repo:cave"));
  assert.ok(groups.some((group) => group.key === "branch:cody/editor"));
  assert.equal(groups.find((group) => group.key === "session:sess-1").cards.length, 2);
});

test("buildTaskFreshnessReport includes prioritized sections and concrete cards", () => {
  const report = buildTaskFreshnessReport(
    [
      {
        id: "blocked-1",
        title: "Needs API decision",
        status: "blocked",
        priority: "urgent",
        familiarId: "sage",
        updatedAt: "2026-06-06T10:00:00.000Z",
        needsHuman: true,
      },
    ],
    { now: new Date("2026-06-06T12:00:00.000Z") },
  );

  assert.match(report, /^# Coven Task Freshness - 2026-06-06/m);
  assert.match(report, /## Needs Human/);
  assert.match(report, /## Thread Coordination/);
  assert.match(report, /Needs API decision/);
  assert.match(report, /sage/);
  assert.match(report, /## Next Actions/);
});

test("loadBoard reads Cave task cards from a Coven home", async () => {
  const root = await mkdtemp(join(tmpdir(), "coven-task-manager-"));
  await writeFile(
    join(root, "cave-board.json"),
    JSON.stringify({ version: 1, cards: [{ id: "one", title: "Card", status: "inbox" }] }),
    "utf8",
  );

  const cards = await loadBoard({ covenHome: root });

  assert.equal(cards.length, 1);
  assert.equal(cards[0].id, "one");
});

test("parseStaleRunningHours falls back for invalid CLI values", () => {
  assert.equal(parseStaleRunningHours(undefined), 4);
  assert.equal(parseStaleRunningHours("true"), 4);
  assert.equal(parseStaleRunningHours("nope"), 4);
  assert.equal(parseStaleRunningHours("0"), 4);
  assert.equal(parseStaleRunningHours("6"), 6);
});

test("normalizeAutomationStatus accepts only supported automation states", () => {
  assert.equal(normalizeAutomationStatus(undefined), "PAUSED");
  assert.equal(normalizeAutomationStatus("active"), "ACTIVE");
  assert.equal(normalizeAutomationStatus("PAUSED"), "PAUSED");
  assert.equal(normalizeAutomationStatus("delete-everything"), "PAUSED");
});

test("installDefaultAutomations writes paused Codex automation TOMLs by default", async () => {
  const root = await mkdtemp(join(tmpdir(), "coven-task-manager-"));
  const codexHome = join(root, ".codex");
  const skillPath = join(root, "skill");
  await mkdir(skillPath, { recursive: true });

  const installed = await installDefaultAutomations({
    codexHome,
    skillPath,
    status: "PAUSED",
  });

  assert.deepEqual(
    installed.map((item) => item.id),
    [
      "coven-task-freshness-daily",
      "coven-task-blocked-escalation",
      "coven-task-weekly-cleanup",
    ],
  );

  const first = join(codexHome, "automations", "coven-task-freshness-daily", "automation.toml");
  const contents = await readFile(first, "utf8");
  assert.match(contents, /status = "PAUSED"/);
  assert.match(contents, /Use the `coven-task-manager` skill/);
  assert.match(contents, /rrule = "RRULE:FREQ=WEEKLY;BYHOUR=8;BYMINUTE=30;BYDAY=SU,MO,TU,WE,TH,FR,SA"/);
  assert.doesNotMatch(contents, /^model = /m);
  await stat(first);
});

test("installSkillSymlink refuses to replace a real skill directory", async () => {
  const root = await mkdtemp(join(tmpdir(), "coven-task-manager-"));
  const existing = join(root, ".coven", "skills", "coven-task-manager");
  await mkdir(existing, { recursive: true });

  await assert.rejects(
    () => installSkillSymlink({ covenHome: join(root, ".coven"), replace: true }),
    /refusing to replace non-symlink/,
  );
});
