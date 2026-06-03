#!/usr/bin/env node
/**
 * coven-board-entry — Create a task on the Coven task board
 * 
 * Usage:
 *   node create-task.mjs --title "Task name" [options]
 *   import { createCovenTask } from './create-task.mjs';
 */

import { randomUUID } from "crypto";
import { readFileSync, writeFileSync, existsSync } from "fs";
import { homedir } from "os";
import { join } from "path";

const GATEWAY_URL = process.env.OPENCLAW_GATEWAY_URL?.trim() ?? "http://localhost:3000";
const PENDING_FILE = join(homedir(), ".openclaw", "coven-tasks-pending.json");

/**
 * Build a CovenTask from partial input.
 */
function buildTask(input) {
  const {
    title,
    description,
    status = "inbox",
    priority = "medium",
    familiar,
    project,
    tags,
    createdBy,
  } = input;

  if (!title) throw new Error("title is required");

  const now = new Date().toISOString();
  const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 32);
  const prefix = familiar ?? "task";
  const id = `${prefix}-${slug}-${Date.now()}`;

  return {
    id,
    title,
    description: description ?? undefined,
    status,
    priority,
    familiar: familiar ?? undefined,
    project: project ?? undefined,
    tags: Array.isArray(tags) ? tags : tags ? [tags] : undefined,
    createdBy: createdBy ?? familiar ?? "unknown",
    createdAt: now,
    updatedAt: now,
  };
}

/**
 * Write task to the local pending file as fallback.
 */
function writeToPending(task) {
  let existing = [];
  if (existsSync(PENDING_FILE)) {
    try {
      existing = JSON.parse(readFileSync(PENDING_FILE, "utf8"));
    } catch {
      existing = [];
    }
  }
  existing.push(task);
  writeFileSync(PENDING_FILE, JSON.stringify(existing, null, 2));
  return task;
}

/**
 * Main entry: create a task.
 * Tries gateway API first, falls back to local pending file.
 */
export async function createCovenTask(input) {
  const task = buildTask(input);

  // Try gateway
  try {
    const res = await fetch(`${GATEWAY_URL}/api/gateway/tasks`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(process.env.OPENCLAW_GATEWAY_TOKEN
          ? { Authorization: `Bearer ${process.env.OPENCLAW_GATEWAY_TOKEN}` }
          : {}),
      },
      body: JSON.stringify({ tasks: [task], userId: task.createdBy }),
      signal: AbortSignal.timeout(5000),
    });

    if (res.ok) {
      const data = await res.json();
      console.log(`[coven-board-entry] ✓ Task synced to gateway (${data.synced ?? 1} tasks)`);
      return task;
    } else {
      console.warn(`[coven-board-entry] Gateway returned ${res.status}, falling back to pending file`);
    }
  } catch (err) {
    console.warn(`[coven-board-entry] Gateway unreachable (${err.message}), falling back to pending file`);
  }

  // Fallback
  writeToPending(task);
  console.log(`[coven-board-entry] ✓ Task written to ${PENDING_FILE}`);
  return task;
}

// ── CLI mode ─────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith("--")) {
      const key = argv[i].slice(2);
      const val = argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[++i] : "true";
      if (key === "tags") {
        args.tags = val.split(",").map((t) => t.trim());
      } else {
        args[key] = val;
      }
    }
  }
  return args;
}

const isMain = process.argv[1] === new URL(import.meta.url).pathname;
if (isMain) {
  const args = parseArgs(process.argv.slice(2));
  if (!args.title) {
    console.error("Usage: node create-task.mjs --title <title> [--description ...] [--priority high|medium|low|critical] [--familiar echo] [--project CovenCave] [--tags tag1,tag2]");
    process.exit(1);
  }
  createCovenTask(args).then((task) => {
    console.log("\nCreated task:");
    console.log(JSON.stringify(task, null, 2));
  }).catch((err) => {
    console.error("Error:", err.message);
    process.exit(1);
  });
}
