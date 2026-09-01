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

function buildSlugIndex(mapping) {
  const bySlug = new Map();
  for (const outcome of mapping.outcomes ?? []) {
    bySlug.set(outcome.slug, outcome);
  }
  return bySlug;
}

function findDependencyErrors(mapping) {
  const findings = [];
  const bySlug = buildSlugIndex(mapping);
  const edges = new Map();

  for (const outcome of mapping.outcomes ?? []) {
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

  for (const outcome of mapping.outcomes ?? []) {
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

  findings.push(...findDependencyErrors(mapping));
  return findings;
}

function findPendingProvisioning(mapping) {
  const findings = [];
  for (const outcome of mapping.outcomes ?? []) {
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
  const lines = [
    "| Outcome | GitHub | Priority | Bead label | Bead ID | Dependencies | Disposition |",
    "| --- | --- | --- | --- | --- | --- | --- |",
  ];
  const order = { program: 0, "p0-foundation": 1, "p0-workstream": 2 };
  const outcomes = [...(mapping.outcomes ?? [])].sort(
    (a, b) => (order[a.role] ?? 9) - (order[b.role] ?? 9) || a.slug.localeCompare(b.slug),
  );
  for (const outcome of outcomes) {
    const ref = outcomeGithubRef(outcome);
    const deps = (outcome.depends_on ?? []).join(", ") || "(none)";
    lines.push(
      `| ${outcome.slug} | [${ref}](${outcome.github.url}) | ${outcome.github.priority} | \`${outcome.bead.label}\` | ${
        outcome.bead.id ?? "(pending provisioning)"
      } | ${deps} | ${outcome.bead.disposition} |`,
    );
  }
  if ((mapping.cross_repository_children ?? []).length === 0) {
    lines.push("");
    lines.push(
      "_Cross-repository child outcomes: none created yet. One Bead per SDK, Cave, Psyche, docs, organization-canary, Familiar Contract, and Threads outcome under the program is mapped here one-to-one as each is created._",
    );
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
  for (const outcome of mapping.outcomes ?? []) {
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

function analyze(mapping, roadmapText, exportText) {
  const findings = [
    ...findMappingErrors(mapping),
    ...findGeneratedBlockDrift(mapping, roadmapText),
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
  for (const outcome of pendingProvisioning.outcomes) {
    outcome.bead.id = null;
    outcome.bead.provisioning = "selftest";
  }
  if (!analyze(pendingProvisioning, baseRoadmap, undefined).some((finding) => finding.code === "W010")) {
    failures.push("selftest: expected W010 detection on a synthetic pending-provisioning mapping");
  }

  const duplicateRef = clone(baseMapping);
  duplicateRef.outcomes[1].github.issue = duplicateRef.outcomes[2].github.issue;
  expectFinding(analyze(duplicateRef, baseRoadmap, undefined), "E001", "duplicate GitHub mapping");

  const unknownDep = clone(baseMapping);
  unknownDep.outcomes[2].depends_on.push("does-not-exist");
  expectFinding(analyze(unknownDep, baseRoadmap, undefined), "E003", "unknown dependency");

  const cycle = clone(baseMapping);
  cycle.outcomes[0].depends_on.push("certification");
  cycle.outcomes[5].depends_on.push("program");
  expectFinding(analyze(cycle, baseRoadmap, undefined), "E004", "dependency cycle");

  const badPriority = clone(baseMapping);
  badPriority.outcomes[2].github.priority = "P9";
  expectFinding(analyze(badPriority, baseRoadmap, undefined), "E005", "invalid priority");

  const noOwner = clone(baseMapping);
  noOwner.outcomes[2].github.owner = null;
  expectFinding(analyze(noOwner, baseRoadmap, undefined), "E006", "P0 without owner");

  const closedNoEvidence = clone(baseMapping);
  closedNoEvidence.outcomes[2].github.state = "closed";
  expectFinding(analyze(closedNoEvidence, baseRoadmap, undefined), "E007", "closed without evidence");

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
  for (const outcome of emptyOutcomeMapping.outcomes) {
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
    const declared = clone(baseMapping);
    declared.outcomes[2].bead.id = "automations-v1.1";
    const declaredFindings = analyze(declared, baseRoadmap, duplicateBeadExport);
    expectFinding(declaredFindings, "E101", "duplicate bead references for one outcome");
  }

  if (failures.length > 0) {
    console.error(failures.join("\n"));
    return false;
  }
  console.log(`drift-check: selftest passed (${SENSITIVE_PATTERNS.length} sensitive-payload rules, 11 drift fixtures)`);
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
