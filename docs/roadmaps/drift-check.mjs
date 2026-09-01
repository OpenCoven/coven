#!/usr/bin/env node
// Drift check for the Coven Automations v1 tracker mapping (OpenCoven/coven#859).
//
// Verifies that the machine-readable Bead <-> GitHub mapping
// (docs/roadmaps/coven-automations-v1.mapping.json), the generated mapping table
// inside docs/roadmaps/coven-automations-v1.md, and an optional Beads public
// export (.beads/issues.jsonl from OpenCoven/coven-cave) agree.
//
// Design constraints (from OpenCoven/coven#859):
//   - runnable locally and in CI without ambient production credentials;
//   - no network access; the optional Beads export is a local file;
//   - reports identifiers, statuses, priorities, links, and evidence references only;
//   - tracker data is never treated as production automation state.
//
// Usage:
//   node docs/roadmaps/drift-check.mjs                       # verify committed state (exit 1 on error-severity drift)
//   node docs/roadmaps/drift-check.mjs --strict              # pending-provisioning warnings also fail
//   node docs/roadmaps/drift-check.mjs --beads-export PATH   # cross-check a Beads issues.jsonl export
//   node docs/roadmaps/drift-check.mjs --render              # regenerate the roadmap mapping table in place
//   node docs/roadmaps/drift-check.mjs --selftest            # run built-in detection fixtures

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROADMAPS_DIR = path.resolve(SCRIPT_DIR);
const MAPPING_PATH = path.join(ROADMAPS_DIR, "coven-automations-v1.mapping.json");
const ROADMAP_PATH = path.join(ROADMAPS_DIR, "coven-automations-v1.md");

const BLOCK_BEGIN =
  "<!-- BEGIN GENERATED:MAPPING-TABLE v1 -- regenerate with: node docs/roadmaps/drift-check.mjs --render (do not edit by hand) -->";
const BLOCK_END = "<!-- END GENERATED:MAPPING-TABLE -->";

const PRIORITIES = new Set(["P0", "P1", "P2"]);
const BEAD_PRIORITY_BY_NUMBER = { 0: "P0", 1: "P1", 2: "P2" };

// ---------------------------------------------------------------------------
// Program membership contract (E011).
//
// These are the outcomes that OpenCoven/coven#854 and OpenCoven/coven#859
// declare as the Automations v1 P0/P1 set. They are pinned here, in the checker,
// on purpose: a coverage rule that reads its own expectations out of the file it
// is checking can always be satisfied by deleting a row. Every ref below must
// appear in the section named here, so the mapping cannot pass -- in default or
// --strict mode -- by omitting the #859 program-control binding or any of the
// seven cross-repository children.
//
// Adding or retiring a program outcome is a deliberate two-file change: update
// the mapping and update this contract in the same reviewed PR.
// ---------------------------------------------------------------------------

const REQUIRED_OUTCOME_REFS = [
  "OpenCoven/coven#854", // program / release rollup
  "OpenCoven/coven#859", // program control / tracker operationalization
  "OpenCoven/coven#816", // foundation
  "OpenCoven/coven#855", // protocol
  "OpenCoven/coven#856", // scheduler
  "OpenCoven/coven#857", // authority
  "OpenCoven/coven#858", // certification
];

const REQUIRED_CROSS_REPOSITORY_CHILD_REFS = [
  "OpenCoven/familiar-contract#17", // familiar embodiment profile
  "OpenCoven/coven-threads#29", // automation authority profile
  "OpenCoven/sdk#80", // SDK surface
  "OpenCoven/coven-cave#5217", // Cave oversight
  "OpenCoven/psyche#18", // Psyche adapter
  "OpenCoven/coven-docs#76", // documentation
  "OpenCoven/.github#2", // organization canaries
];

// ---------------------------------------------------------------------------
// Sensitive-payload detection. Patterns are assembled from fragments so that
// this source file never itself contains a string matching the repo privacy
// guard or the detector below.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Analysis core (pure; exercised by --selftest)
// ---------------------------------------------------------------------------

function outcomeGithubRef(outcome) {
  return `${outcome.github.repo}#${outcome.github.issue}`;
}

// Every mapped row, whichever section it lives in. Cross-repository children are
// first-class program members: they carry the same shape, the same one-to-one
// invariant, and the same evidence semantics as in-repository outcomes.
function allEntries(mapping) {
  return [...(mapping.outcomes ?? []), ...(mapping.cross_repository_children ?? [])];
}

function buildSlugIndex(mapping) {
  const bySlug = new Map();
  for (const outcome of allEntries(mapping)) {
    bySlug.set(outcome.slug, outcome);
  }
  return bySlug;
}

function findCoverageErrors(mapping) {
  const findings = [];
  const sectionOf = (entries) => new Set((entries ?? []).map(outcomeGithubRef));
  const outcomeRefs = sectionOf(mapping.outcomes);
  const childRefs = sectionOf(mapping.cross_repository_children);

  for (const ref of REQUIRED_OUTCOME_REFS) {
    if (outcomeRefs.has(ref)) continue;
    const misfiled = childRefs.has(ref) ? " (found under cross_repository_children instead)" : "";
    findings.push({
      code: "E011",
      severity: "error",
      slug: null,
      message: `required program outcome ${ref} is missing from mapping.outcomes${misfiled}`,
    });
  }

  for (const ref of REQUIRED_CROSS_REPOSITORY_CHILD_REFS) {
    if (childRefs.has(ref)) continue;
    const misfiled = outcomeRefs.has(ref) ? " (found under outcomes instead)" : "";
    findings.push({
      code: "E011",
      severity: "error",
      slug: null,
      message: `required cross-repository child ${ref} is missing from mapping.cross_repository_children${misfiled}`,
    });
  }

  return findings;
}

function findDependencyErrors(mapping) {
  const findings = [];
  const bySlug = buildSlugIndex(mapping);
  const edges = new Map();

  for (const outcome of allEntries(mapping)) {
    for (const dep of outcome.depends_on ?? []) {
      if (!bySlug.has(dep)) {
        findings.push({
          code: "E003",
          severity: "error",
          slug: outcome.slug,
          message: `unknown dependency mapping: ${outcomeGithubRef(outcome)} depends on unknown slug '${dep}'`,
        });
      }
    }
    edges.set(outcome.slug, [...(outcome.depends_on ?? [])]);
  }

  const state = new Map();
  const stack = new Map();
  const visit = (slug) => {
    if (state.get(slug) === "done") return true;
    if (state.get(slug) === "visiting") {
      findings.push({
        code: "E004",
        severity: "error",
        slug,
        message: `dependency cycle involving '${slug}'`,
      });
      return false;
    }
    state.set(slug, "visiting");
    for (const dep of edges.get(slug) ?? []) {
      if (!visit(dep)) return false;
    }
    state.set(slug, "done");
    return true;
  };
  for (const slug of edges.keys()) visit(slug);

  return findings;
}

function findMappingErrors(mapping) {
  const findings = [];
  const seenRefs = new Map();
  const seenSlugs = new Set();
  const seenLabels = new Set();
  const seenBeadIds = new Map();
  const seenCaveLabels = new Set();

  for (const outcome of allEntries(mapping)) {
    const ref = outcomeGithubRef(outcome);
    if (seenRefs.has(ref)) {
      findings.push({
        code: "E001",
        severity: "error",
        slug: outcome.slug,
        message: `GitHub outcome ${ref} maps to more than one bead ('${seenRefs.get(ref)}' and '${outcome.slug}')`,
      });
    } else {
      seenRefs.set(ref, outcome.slug);
    }

    if (seenSlugs.has(outcome.slug)) {
      findings.push({
        code: "E002",
        severity: "error",
        slug: outcome.slug,
        message: `duplicate outcome slug '${outcome.slug}'`,
      });
    }
    seenSlugs.add(outcome.slug);

    const label = outcome.bead?.label;
    if (label) {
      if (seenLabels.has(label)) {
        findings.push({
          code: "E002",
          severity: "error",
          slug: outcome.slug,
          message: `duplicate bead label '${label}'`,
        });
      }
      seenLabels.add(label);
    }

    const beadId = outcome.bead?.id;
    if (beadId) {
      if (seenBeadIds.has(beadId)) {
        findings.push({
          code: "E002",
          severity: "error",
          slug: outcome.slug,
          message: `bead ${beadId} maps to more than one outcome ('${seenBeadIds.get(beadId)}' and '${outcome.slug}')`,
        });
      } else {
        seenBeadIds.set(beadId, outcome.slug);
      }
    }

    const caveLabel = outcome.bead?.cave_label;
    if (caveLabel) {
      if (seenCaveLabels.has(caveLabel)) {
        findings.push({
          code: "E002",
          severity: "error",
          slug: outcome.slug,
          message: `duplicate canonical Cave bead label '${caveLabel}'`,
        });
      }
      seenCaveLabels.add(caveLabel);
    }

    const priority = outcome.github?.priority;
    if (!PRIORITIES.has(priority)) {
      findings.push({
        code: "E005",
        severity: "error",
        slug: outcome.slug,
        message: `invalid or missing priority '${priority}' for ${ref} (expected one of ${[...PRIORITIES].join(", ")})`,
      });
    }

    if (priority === "P0") {
      const missing = [];
      if (!outcome.github?.owner) missing.push("owner");
      if (!outcome.bead?.gate) missing.push("acceptance gate");
      if (!outcome.bead?.disposition) missing.push("disposition");
      if (missing.length > 0) {
        findings.push({
          code: "E006",
          severity: "error",
          slug: outcome.slug,
          message: `P0 outcome ${ref} is missing: ${missing.join(", ")}`,
        });
      }
    }

    const closedMirror =
      outcome.github?.state === "closed" ||
      /^(complete|done|closed)/i.test(outcome.bead?.disposition ?? "");
    if (closedMirror && (outcome.bead?.evidence ?? []).length === 0) {
      findings.push({
        code: "E007",
        severity: "error",
        slug: outcome.slug,
        message: `completed work for ${ref} lacks PR/test/release evidence references`,
      });
    }
  }

  findings.push(...findCoverageErrors(mapping));
  findings.push(...findDependencyErrors(mapping));
  return findings;
}

function findPendingProvisioning(mapping) {
  const findings = [];
  for (const outcome of allEntries(mapping)) {
    if (outcome.bead?.id === null || outcome.bead?.id === undefined) {
      const ref = outcomeGithubRef(outcome);
      const provisioning = outcome.bead?.provisioning ?? "unrecorded";
      findings.push({
        code: "W010",
        severity: "warning",
        slug: outcome.slug,
        message: `${ref} has no provisioned bead id yet (provisioning: ${provisioning})`,
      });
    }
  }
  return findings;
}

function renderMappingTable(mapping) {
  const header = [
    "| Outcome | GitHub | Priority | Bead label | Bead ID | Dependencies | Disposition |",
    "| --- | --- | --- | --- | --- | --- | --- |",
  ];
  const order = {
    program: 0,
    "program-control": 1,
    "p0-foundation": 2,
    "p0-workstream": 3,
    "p0-cross-repository-child": 4,
    "p1-cross-repository-child": 5,
  };
  const sortEntries = (entries) =>
    [...entries].sort((a, b) => (order[a.role] ?? 9) - (order[b.role] ?? 9) || a.slug.localeCompare(b.slug));
  const row = (outcome) => {
    const ref = outcomeGithubRef(outcome);
    const deps = (outcome.depends_on ?? []).join(", ") || "(none)";
    // Cross-repository children are cited by fully-qualified `owner/repo#number`
    // reference rather than absolute URL: that is this tracker's citation
    // convention for cross-repo work, and several of those absolute URLs also
    // trip the repository secret guard's high-entropy heuristic
    // (scripts/check-secrets.py). Re-adding a `github.url` to a child therefore
    // fails the secret scan, not this checker.
    const cell = outcome.github.url ? `[${ref}](${outcome.github.url})` : `\`${ref}\``;
    return `| ${outcome.slug} | ${cell} | ${outcome.github.priority} | \`${
      outcome.bead.label
    }\` | ${outcome.bead.id ?? "(pending provisioning)"} | ${deps} | ${outcome.bead.disposition} |`;
  };

  const lines = [...header];
  for (const outcome of sortEntries(mapping.outcomes ?? [])) lines.push(row(outcome));

  const children = mapping.cross_repository_children ?? [];
  lines.push("");
  if (children.length === 0) {
    lines.push(
      "_Cross-repository child outcomes: **none mapped**. The program declares one Bead per SDK, Cave, Psyche, docs, organization-canary, Familiar Contract, and Threads outcome, so an empty section is a coverage violation (`E011`), not a statement that none exist._",
    );
  } else {
    lines.push(
      `_Cross-repository child outcomes (${children.length}), mapped one-to-one in the same canonical Cave Beads graph. Each carries a live bead id, an acceptance gate, a disposition, and an evidence list that stays empty until exact PR/test/release references exist._`,
    );
    lines.push("");
    lines.push(...header);
    for (const child of sortEntries(children)) lines.push(row(child));
  }
  return lines.join("\n");
}

function extractGeneratedBlock(roadmapText) {
  const begin = roadmapText.indexOf(BLOCK_BEGIN);
  const end = roadmapText.indexOf(BLOCK_END);
  if (begin === -1 || end === -1 || end < begin) return null;
  const start = begin + BLOCK_BEGIN.length;
  return roadmapText.slice(start, end).replace(/^\n/, "").replace(/\n\s*$/, "\n");
}

function findGeneratedBlockDrift(mapping, roadmapText) {
  const committed = extractGeneratedBlock(roadmapText);
  if (committed === null) {
    return [
      {
        code: "E008",
        severity: "error",
        slug: null,
        message: "generated mapping table block missing from docs/roadmaps/coven-automations-v1.md",
      },
    ];
  }
  const expected = renderMappingTable(mapping);
  if (committed.trimEnd() !== expected.trimEnd()) {
    return [
      {
        code: "E008",
        severity: "error",
        slug: null,
        message:
          "generated mapping table in docs/roadmaps/coven-automations-v1.md was edited outside the generator contract (run: node docs/roadmaps/drift-check.mjs --render)",
      },
    ];
  }
  return [];
}

function beadPriorityLabel(priority) {
  if (typeof priority === "number") return BEAD_PRIORITY_BY_NUMBER[priority] ?? `P${priority}`;
  return String(priority ?? "unknown");
}

function collectBeadGithubRefs(bead) {
  const haystack = [
    bead.external_ref,
    bead.notes,
    bead.design,
    bead.acceptance_criteria,
    ...(bead.comments ?? []).map((comment) => comment?.text ?? ""),
  ]
    .filter((value) => typeof value === "string")
    .join("\n");
  const refs = new Set();
  for (const match of haystack.matchAll(/https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/issues\/(\d+)/g)) {
    refs.add(`${match[1]}#${match[2]}`);
  }
  for (const match of haystack.matchAll(/\b([\w.-]+\/[\w.-]+)#(\d+)\b/g)) {
    refs.add(`${match[1]}#${match[2]}`);
  }
  return refs;
}

function findExportDrift(mapping, exportText) {
  const findings = [];
  const beads = [];
  for (const [index, line] of exportText.split("\n").entries()) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    let bead;
    try {
      bead = JSON.parse(trimmed);
    } catch {
      findings.push({
        code: "E100",
        severity: "error",
        slug: null,
        message: `beads export line ${index + 1} is not valid JSON`,
      });
      continue;
    }
    beads.push({ line: index + 1, bead });
  }

  for (const { line, bead } of beads) {
    for (const hit of findSensitivePayloads(JSON.stringify(bead))) {
      findings.push({
        code: "E009",
        severity: "error",
        slug: bead.id ?? null,
        message: `tracker output contains sensitive payload (rule: ${hit.rule}) at export line ${line}`,
      });
    }
  }

  const byRef = new Map();
  for (const outcome of allEntries(mapping)) {
    byRef.set(outcomeGithubRef(outcome), outcome);
  }

  const beadRefs = new Map();
  for (const { line, bead } of beads) {
    for (const ref of collectBeadGithubRefs(bead)) {
      if (!byRef.has(ref)) continue;
      if (!beadRefs.has(ref)) beadRefs.set(ref, []);
      beadRefs.get(ref).push({ line, bead });
    }
  }

  for (const [ref, outcome] of byRef) {
    if (outcome.bead?.id !== null && outcome.bead?.id !== undefined) {
      const linked = beadRefs.get(ref) ?? [];
      if (linked.length === 0) {
        findings.push({
          code: "E101",
          severity: "error",
          slug: outcome.slug,
          message: `GitHub outcome ${ref} has no bead referencing it in the export (expected exactly one)`,
        });
      } else if (linked.length > 1) {
        findings.push({
          code: "E101",
          severity: "error",
          slug: outcome.slug,
          message: `GitHub outcome ${ref} is referenced by ${linked.length} beads in the export (expected exactly one)`,
        });
      }
    }
  }

  for (const [ref, entries] of beadRefs) {
    const outcome = byRef.get(ref);
    for (const { bead } of entries) {
      const beadOpen = bead.status !== undefined && !["closed", "done"].includes(bead.status);
      const githubOpen = outcome.github?.state === "open";
      if (beadOpen !== githubOpen) {
        findings.push({
          code: "E102",
          severity: "error",
          slug: outcome.slug,
          message: `state drift for ${ref}: bead '${bead.id}' status '${bead.status}' vs GitHub state '${outcome.github?.state}'`,
        });
      }
      const expectedPriority = outcome.github?.priority;
      const actualPriority = beadPriorityLabel(bead.priority);
      if (PRIORITIES.has(expectedPriority) && actualPriority !== expectedPriority) {
        findings.push({
          code: "E103",
          severity: "error",
          slug: outcome.slug,
          message: `priority drift for ${ref}: bead '${bead.id}' is ${actualPriority}, mapping says ${expectedPriority}`,
        });
      }
      const labels = bead.labels ?? [];
      if (!labels.includes("surface:shared")) {
        findings.push({
          code: "E104",
          severity: "error",
          slug: outcome.slug,
          message: `bead '${bead.id}' mapped to ${ref} lacks the surface:shared label (has: ${
            labels.length > 0 ? labels.join(", ") : "(none)"
          })`,
        });
      }
    }
  }

  return findings;
}

function findSyncNarrativeDrift(mapping, roadmapText) {
  // Truth-critical invariant: the machine-readable remote sync status
  // (mapping.sync.beads_provisioning.remote_sync_status) and the roadmap's
  // Active blockers narrative must agree on whether remote Dolt propagation is
  // still an open blocker. This exists because a real successful sync once left
  // one artifact corrected while the other silently claimed propagation was
  // deferred. It pins no transient OID -- it only enforces cross-artifact
  // agreement, so either both are updated together or the check fails.
  if (typeof roadmapText !== "string") return [];
  const status = String(mapping.sync?.beads_provisioning?.remote_sync_status ?? "");
  const statusUnresolved = /^\s*(deferred|blocked)\b/i.test(status);
  const narrativeUnresolved = /remote dolt propagation[^\n]*\b(deferred|blocked)\b/i.test(roadmapText);
  if (statusUnresolved === narrativeUnresolved) return [];
  return [
    {
      code: "E012",
      severity: "error",
      slug: null,
      message:
        `remote sync state is inconsistent across artifacts: ` +
        `mapping.sync.remote_sync_status is ${statusUnresolved ? "unresolved (deferred/blocked)" : "resolved"} ` +
        `but the roadmap Active blockers narrative is ${narrativeUnresolved ? "unresolved (deferred/blocked)" : "resolved"}. ` +
        `Update docs/roadmaps/coven-automations-v1.mapping.json and coven-automations-v1.md together.`,
    },
  ];
}

function analyze(mapping, roadmapText, exportText) {
  const findings = [
    ...findMappingErrors(mapping),
    ...findGeneratedBlockDrift(mapping, roadmapText),
    ...findSyncNarrativeDrift(mapping, roadmapText),
    ...findPendingProvisioning(mapping),
  ];
  if (typeof roadmapText === "string") {
    for (const hit of findSensitivePayloads(roadmapText)) {
      findings.push({
        code: "E009",
        severity: "error",
        slug: null,
        message: `roadmap artifact contains sensitive payload (rule: ${hit.rule})`,
      });
    }
  }
  const mappingText = JSON.stringify(mapping, null, 2);
  for (const hit of findSensitivePayloads(mappingText)) {
    findings.push({
      code: "E009",
      severity: "error",
      slug: null,
      message: `mapping file contains sensitive payload (rule: ${hit.rule})`,
    });
  }
  if (exportText !== undefined) {
    findings.push(...findExportDrift(mapping, exportText));
  }
  return findings;
}

function printFindings(findings) {
  if (findings.length === 0) {
    console.log("drift-check: no findings");
    return;
  }
  for (const finding of findings) {
    const scope = finding.slug ? ` [${finding.slug}]` : "";
    console.log(`${finding.code}${scope} ${finding.severity}: ${finding.message}`);
  }
}

function loadMapping() {
  return JSON.parse(fs.readFileSync(MAPPING_PATH, "utf8"));
}

function writeRenderedBlock(mapping) {
  let roadmap = fs.readFileSync(ROADMAP_PATH, "utf8");
  const begin = roadmap.indexOf(BLOCK_BEGIN);
  const end = roadmap.indexOf(BLOCK_END);
  if (begin === -1 || end === -1 || end < begin) {
    console.error("drift-check: generated block markers missing from roadmap; cannot render");
    process.exitCode = 2;
    return false;
  }
  const replacement = `${BLOCK_BEGIN}\n${renderMappingTable(mapping)}\n${BLOCK_END}`;
  roadmap = roadmap.slice(0, begin) + replacement + roadmap.slice(end + BLOCK_END.length);
  fs.writeFileSync(ROADMAP_PATH, roadmap);
  return true;
}

function runSelftest() {
  const failures = [];
  const expectFinding = (findings, code, label) => {
    if (!findings.some((finding) => finding.code === code)) {
      failures.push(`selftest: expected ${code} (${label}) to be detected`);
    }
  };
  const clone = (value) => JSON.parse(JSON.stringify(value));

  const baseMapping = loadMapping();
  const baseRoadmap = fs.readFileSync(ROADMAP_PATH, "utf8");
  const pristine = analyze(baseMapping, baseRoadmap, undefined);
  const pristineErrors = pristine.filter((finding) => finding.severity === "error");
  if (pristineErrors.length > 0) {
    failures.push(`selftest: committed state has error-severity findings: ${JSON.stringify(pristineErrors)}`);
  }
  // The committed mapping is now fully provisioned (every outcome carries a live
  // bead id), so it must be clean under --strict, i.e. no W010 pending-provisioning
  // warnings. The W010 detection rule itself is still exercised below against a
  // synthetic pending fixture.
  if (pristine.some((finding) => finding.code === "W010")) {
    failures.push(
      `selftest: committed mapping still has pending-provisioning warnings (W010): ${JSON.stringify(
        pristine.filter((finding) => finding.code === "W010"),
      )}`,
    );
  }
  const pendingProvisioning = clone(baseMapping);
  for (const outcome of [...pendingProvisioning.outcomes, ...pendingProvisioning.cross_repository_children]) {
    outcome.bead.id = null;
    outcome.bead.provisioning = "selftest";
  }
  if (!analyze(pendingProvisioning, baseRoadmap, undefined).some((finding) => finding.code === "W010")) {
    failures.push("selftest: expected W010 detection on a synthetic pending-provisioning mapping");
  }

  // A cross-repository child whose bead id is missing must be caught too: the
  // pending-provisioning rule covers the whole program, not just mapping.outcomes.
  const pendingChild = clone(baseMapping);
  pendingChild.cross_repository_children[2].bead.id = null;
  pendingChild.cross_repository_children[2].bead.provisioning = "selftest";
  expectFinding(
    analyze(pendingChild, baseRoadmap, undefined),
    "W010",
    "unprovisioned cross-repository child",
  );

  // Coverage contract (E011): --strict must not be satisfiable by deleting rows.
  const droppedProgramControl = clone(baseMapping);
  droppedProgramControl.outcomes = droppedProgramControl.outcomes.filter(
    (outcome) => outcome.github.issue !== 859,
  );
  expectFinding(
    analyze(droppedProgramControl, baseRoadmap, undefined),
    "E011",
    "omitted #859 program-control outcome",
  );

  const emptyChildren = clone(baseMapping);
  emptyChildren.cross_repository_children = [];
  const emptyChildFindings = analyze(emptyChildren, baseRoadmap, undefined);
  expectFinding(emptyChildFindings, "E011", "emptied cross_repository_children");
  if (emptyChildFindings.filter((finding) => finding.code === "E011").length !== 7) {
    failures.push(
      `selftest: expected one E011 per missing cross-repository child, got ${
        emptyChildFindings.filter((finding) => finding.code === "E011").length
      }`,
    );
  }

  const droppedOneChild = clone(baseMapping);
  droppedOneChild.cross_repository_children = droppedOneChild.cross_repository_children.filter(
    (child) => child.slug !== "sdk",
  );
  expectFinding(analyze(droppedOneChild, baseRoadmap, undefined), "E011", "omitted sdk#80 child");

  const misfiledChild = clone(baseMapping);
  const movedChild = misfiledChild.cross_repository_children.find((child) => child.slug === "documentation");
  misfiledChild.cross_repository_children = misfiledChild.cross_repository_children.filter(
    (child) => child.slug !== "documentation",
  );
  misfiledChild.outcomes.push(movedChild);
  expectFinding(analyze(misfiledChild, baseRoadmap, undefined), "E011", "cross-repository child filed as an outcome");

  const duplicateBeadId = clone(baseMapping);
  duplicateBeadId.cross_repository_children[0].bead.id = duplicateBeadId.outcomes[0].bead.id;
  expectFinding(analyze(duplicateBeadId, baseRoadmap, undefined), "E002", "one bead mapped to two outcomes");

  const bySlug = (mapping, slug) => {
    const entry = [...mapping.outcomes, ...mapping.cross_repository_children].find(
      (candidate) => candidate.slug === slug,
    );
    if (!entry) throw new Error(`selftest fixture: no entry with slug '${slug}'`);
    return entry;
  };

  const duplicateRef = clone(baseMapping);
  bySlug(duplicateRef, "protocol").github.issue = bySlug(duplicateRef, "scheduler").github.issue;
  expectFinding(analyze(duplicateRef, baseRoadmap, undefined), "E001", "duplicate GitHub mapping");

  const unknownDep = clone(baseMapping);
  bySlug(unknownDep, "protocol").depends_on.push("does-not-exist");
  expectFinding(analyze(unknownDep, baseRoadmap, undefined), "E003", "unknown dependency");

  const cycle = clone(baseMapping);
  bySlug(cycle, "foundation").depends_on.push("certification");
  bySlug(cycle, "certification").depends_on.push("foundation");
  expectFinding(analyze(cycle, baseRoadmap, undefined), "E004", "dependency cycle");

  const badPriority = clone(baseMapping);
  bySlug(badPriority, "protocol").github.priority = "P9";
  expectFinding(analyze(badPriority, baseRoadmap, undefined), "E005", "invalid priority");

  const noOwner = clone(baseMapping);
  bySlug(noOwner, "protocol").github.owner = null;
  expectFinding(analyze(noOwner, baseRoadmap, undefined), "E006", "P0 without owner");

  const closedNoEvidence = clone(baseMapping);
  bySlug(closedNoEvidence, "protocol").github.state = "closed";
  expectFinding(analyze(closedNoEvidence, baseRoadmap, undefined), "E007", "closed without evidence");

  // The same evidence rule must reach cross-repository children.
  const closedChildNoEvidence = clone(baseMapping);
  bySlug(closedChildNoEvidence, "sdk").github.state = "closed";
  expectFinding(
    analyze(closedChildNoEvidence, baseRoadmap, undefined),
    "E007",
    "closed cross-repository child without evidence",
  );

  const tamperedBlock = baseRoadmap.replace(
    /\| program \| \[/,
    "| program (edited outside the generator contract) | [",
  );
  expectFinding(analyze(baseMapping, tamperedBlock, undefined), "E008", "mirror edit");
  expectFinding(
    analyze(clone(baseMapping), "no markers here", undefined),
    "E008",
    "missing generated block",
  );

  // E012: the machine-readable remote sync status and the roadmap narrative must
  // agree. Both directions of disagreement must be caught.
  const staleStatus = clone(baseMapping);
  staleStatus.sync.beads_provisioning.remote_sync_status =
    "deferred: bd dolt push is non-fast-forward and pull did not complete.";
  expectFinding(
    analyze(staleStatus, baseRoadmap, undefined),
    "E012",
    "mapping says remote sync deferred while roadmap says resolved",
  );
  const staleNarrative = `${baseRoadmap}\n- Remote Dolt propagation blocked (synthetic selftest line)\n`;
  expectFinding(
    analyze(baseMapping, staleNarrative, undefined),
    "E012",
    "roadmap says remote propagation blocked while mapping says resolved",
  );

  const sensitiveMapping = clone(baseMapping);
  sensitiveMapping.outcomes[0].notes = [
    "operator note: ",
    frag("agent:", "demo", ":telegram:", "direct", ":SECRETVALUE"),
  ].join("");
  expectFinding(analyze(sensitiveMapping, baseRoadmap, undefined), "E009", "sensitive payload in mapping");

  const exportFixtures = [
    {
      label: "closed bead vs open outcome (E102)",
      line: JSON.stringify({
        _type: "issue",
        id: "automations-v1.1",
        title: "protocol",
        status: "closed",
        priority: 0,
        labels: ["surface:shared"],
        external_ref: "https://github.com/OpenCoven/coven/issues/855",
      }),
      codes: ["E102"],
    },
    {
      label: "priority drift (E103)",
      line: JSON.stringify({
        _type: "issue",
        id: "automations-v1.1",
        title: "protocol",
        status: "open",
        priority: 1,
        labels: ["surface:shared"],
        external_ref: "https://github.com/OpenCoven/coven/issues/855",
      }),
      codes: ["E103"],
    },
    {
      label: "missing surface:shared label (E104)",
      line: JSON.stringify({
        _type: "issue",
        id: "automations-v1.1",
        title: "protocol",
        status: "open",
        priority: 0,
        labels: ["surface:api"],
        external_ref: "https://github.com/OpenCoven/coven/issues/855",
      }),
      codes: ["E104"],
    },
    {
      label: "sensitive payload in export (E009)",
      line: JSON.stringify({
        _type: "issue",
        id: "automations-v1.9",
        title: "leaky",
        status: "open",
        priority: 0,
        labels: ["surface:shared"],
        notes: frag("session ", "agent:x", ":telegram:bot:", "SECRET"),
      }),
      codes: ["E009"],
    },
  ];

  const emptyOutcomeMapping = clone(baseMapping);
  for (const outcome of [...emptyOutcomeMapping.outcomes, ...emptyOutcomeMapping.cross_repository_children]) {
    outcome.bead.id = null;
    outcome.bead.provisioning = "selftest";
  }

  for (const fixture of exportFixtures) {
    const findings = analyze(emptyOutcomeMapping, baseRoadmap, fixture.line);
    for (const code of fixture.codes) {
      expectFinding(findings, code, fixture.label);
    }
  }

  const duplicateBeadExport = [
    JSON.stringify({
      _type: "issue",
      id: "automations-v1.1",
      status: "open",
      priority: 0,
      labels: ["surface:shared"],
      external_ref: "https://github.com/OpenCoven/coven/issues/855",
    }),
    JSON.stringify({
      _type: "issue",
      id: "automations-v1.2",
      status: "open",
      priority: 0,
      labels: ["surface:shared"],
      external_ref: "OpenCoven/coven#855",
    }),
  ].join("\n");
  const dupFindings = analyze(emptyOutcomeMapping, baseRoadmap, duplicateBeadExport);
  if (!dupFindings.some((finding) => finding.code === "E101")) {
    // Provisioning is pending, so E101 only fires for outcomes with declared ids.
    const declared = clone(emptyOutcomeMapping);
    bySlug(declared, "protocol").bead.id = "automations-v1.1";
    const declaredFindings = analyze(declared, baseRoadmap, duplicateBeadExport);
    expectFinding(declaredFindings, "E101", "duplicate bead references for one outcome");
  }

  if (failures.length > 0) {
    console.error(failures.join("\n"));
    return false;
  }
  console.log(
    `drift-check: selftest passed (${SENSITIVE_PATTERNS.length} sensitive-payload rules, 20 drift fixtures, ` +
      `${REQUIRED_OUTCOME_REFS.length} required outcomes and ${REQUIRED_CROSS_REPOSITORY_CHILD_REFS.length} required cross-repository children pinned)`,
  );
  return true;
}

function main(argv) {
  const args = argv.slice(2);
  if (args.includes("--selftest")) {
    process.exitCode = runSelftest() ? 0 : 1;
    return;
  }

  let mapping;
  try {
    mapping = loadMapping();
  } catch (error) {
    console.error(`drift-check: cannot parse mapping: ${error.message}`);
    process.exitCode = 2;
    return;
  }

  if (args.includes("--render")) {
    const ok = writeRenderedBlock(mapping);
    if (ok) console.log("drift-check: regenerated the roadmap mapping table");
    return;
  }

  let roadmapText;
  try {
    roadmapText = fs.readFileSync(ROADMAP_PATH, "utf8");
  } catch (error) {
    console.error(`drift-check: cannot read roadmap: ${error.message}`);
    process.exitCode = 2;
    return;
  }

  let exportText;
  const exportIndex = args.indexOf("--beads-export");
  if (exportIndex !== -1) {
    const exportPath = args[exportIndex + 1];
    if (!exportPath) {
      console.error("drift-check: --beads-export requires a path");
      process.exitCode = 2;
      return;
    }
    exportText = fs.readFileSync(path.resolve(exportPath), "utf8");
  }

  const findings = analyze(mapping, roadmapText, exportText);
  printFindings(findings);

  const strict = args.includes("--strict");
  const hasErrors = findings.some((finding) => finding.severity === "error");
  const hasWarnings = findings.some((finding) => finding.severity === "warning");
  if (hasErrors || (strict && hasWarnings)) {
    process.exitCode = 1;
  }
}

main(process.argv);
