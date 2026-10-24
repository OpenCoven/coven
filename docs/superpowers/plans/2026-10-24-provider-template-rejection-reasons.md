---
title: "Bounded provider-template rejection reasons"
summary: "Name the closed rejection branch classes behind each unsupported Kernel-File template."
read_when:
  - Changing the Windows reliability provider-template projector or its receipts
source_adjacent_reason: "Defines implementation and verification of native issue 1413."
---

# Bounded provider-template rejection reasons implementation plan

> **For agentic workers:** Use superpowers:executing-plans inline, followed by
> one independent whole-branch review. Checkboxes are intended steps; commits
> and issue evidence establish completion.

**Goal:** Each projected Kernel-File template carries a bounded, canonical union
of closed rejection-reason codes, so a receipt says which existing rejection
branches fired. The 259 unsupported fields in the #1388 receipt were reported
only as "unsupported"; 41 had unknown names and the other 218 had no visible
cause.

**Architecture:** `CovenKernelTemplate.Project` records the closed code of each
rejection branch it already executes, without changing a decision, count,
field projection, limit or reset. The probe and metadata envelopes move to an
explicit v2 that adds one `reasons` array per template. The Python validators
select v1 or v2 from the exact `schemaVersion` string and keep v1 exactly as
strict as it is today.

**Spec:** https://github.com/OpenCoven/coven/issues/1413 under
https://github.com/OpenCoven/coven/issues/1227.

## Frozen reason enum

This list is frozen before implementation. Adding, renaming or splitting a
code requires a new envelope version.

A code names a branch class the projector executed. It does not attribute a
reason to a field, count occurrences, enumerate every defect in a template, or
say anything about capture readiness. Where the legacy projector
short-circuits, only the branch it evaluated is named.

### Non-terminal codes (status `unsupported`)

Each `UnsupportedCount` increment, and each predicate that marks a field
unsupported, maps to exactly one of these codes.

| Code | Legacy branch |
|---|---|
| `root-name` | The root element's local name is not `template`. |
| `root-namespace` | The root is `template` in a namespace other than none or the event namespace. Not evaluated when `root-name` fired (the legacy `&&` short-circuits). |
| `root-attribute` | The root carries a namespaced attribute or any attribute other than `tid` (namespace declarations excluded). |
| `root-content` | An element below an unsupported root. |
| `child-element` | A direct child of a supported root that is not a `data` element. |
| `data-namespace` | A direct `data` child of a supported root in an unsupported namespace. |
| `nested-element` | An element at depth two or deeper below a supported root, including one inside a `data` field. |
| `text` | Non-whitespace text or CDATA anywhere in the template. |
| `field-name` | A `data` field whose `name` is not in the closed name set. |
| `field-type` | A `data` field whose `inType` is not in the closed type set. |
| `field-duplicate` | A field with a known name and type whose name was already seen. The duplicate check runs only when name and type are both known, exactly as the legacy `\|\|` chain does; it is never evaluated for a field that `field-name` or `field-type` already rejected. |
| `win-namespace` | The `win` prefix is bound, in scope of the field, to a namespace other than the Windows events manifest namespace. |
| `field-count` | A `count` attribute that is not a decimal integer in 0..65535. |
| `field-length` | A `length` attribute that is not a decimal integer in 0..65535. |
| `field-attribute` | A field attribute that is namespaced or is not `name`, `inType`, `count` or `length` (namespace declarations excluded). |

Branches that the legacy code reaches through one `else` are classified by a
fixed predicate order: unsupported root, then depth two or more, then a `data`
name, then anything else.

### Terminal codes (status `invalid` or `limit`)

A terminal branch clears every accumulated reason, together with the existing
reset of fields and `UnsupportedCount`, and keeps exactly one code.

| Code | Status | Legacy branch |
|---|---|---|
| `size-limit` | `limit` | More than 32768 UTF-16 code units. |
| `utf16-invalid` | `invalid` | The text cannot be encoded as UTF-8 for its digest (an unpaired surrogate). |
| `xml-invalid` | `invalid` | `XmlException`, including malformed XML and a prohibited DTD. |
| `node-limit` | `limit` | More than 2048 nodes. Checked before depth, as in the legacy `\|\|`. |
| `depth-limit` | `limit` | A node deeper than 16. |
| `field-limit` | `limit` | A 129th field in one template. Checked before the collector budget. |
| `field-budget` | `limit` | A field beyond the collector-wide remaining field budget. |

### Statuses without reasons

`missing`, `empty` and `supported-flat` always carry an empty `reasons` array.

### Cardinality

- The enum has 22 codes: 15 non-terminal and 7 terminal.
- A terminal template carries exactly one code.
- An unsupported template carries at least one and at most **12** codes.
  `root-name` and `root-namespace` exclude each other. Under an unsupported
  root no field, child or nested element is classified, so at most four codes
  apply there (`root-name` or `root-namespace`, `root-attribute`,
  `root-content`, `text`). Under a supported root the remaining 12
  non-terminal codes can all apply at once, and one control template does so.
- `reasons` is the ordinal-sorted set of codes, with no duplicates.
- `UnsupportedCount` is not reason cardinality and is never derived from it.

## Wire versions

- `coven.kernel-file-probe.v2` and `coven.kernel-file-metadata.v2` add one
  required `reasons` array to every template object. Nothing else changes.
- The export-failure object (`coven.kernel-file-collector-export-failure.v1`)
  carries no template data and keeps its kind.
- Validators select v1 or v2 only from the exact `schemaVersion` string, never
  from which keys are present. v1 rejects a `reasons` key exactly as it rejects
  any other unknown key today, and is otherwise unchanged. v2 rejects a missing
  `reasons`, an unknown, duplicate or out-of-order code, more than 12 codes, and
  any status/reason combination the table above rules out.
- Collector-only decoding (`decode_collector`, `decode_output`,
  `validate_collector`, `validate_exit`) takes the version as an explicit
  argument. Existing callers keep v1.
- The finalizer builds a metadata envelope whose version matches the probe
  version it decoded; a v1 bundle still finalizes to a v1 receipt.
- Caps stay at 524288 bytes for a normal payload and 8192 bytes for an
  export-failure payload. A larger augmented result fails capped export; caps
  are not raised to force publication.
- No raw XML, node record, name, attribute value, namespace, exception text,
  arbitrary hash or account/process inventory is exported.

## Steps

- [ ] Characterize complete legacy projections before instrumentation: known
  and unknown fields, unknown-type then known-type with the same name,
  conflicting `win` binding then a duplicate, multiple defects, root, nested,
  text and CDATA cases, invalid XML and DTD, and every limit and reset branch.
- [ ] Instrument the projector, add the v2 probe wire, and update the Python
  validators and finalizer together.
- [ ] Add discriminating reason tests for every code, and mutants for an
  unconditional `seen.Add` and a reason-derived `UnsupportedCount` that the
  characterization must kill.
- [ ] Feed actual typed C# serialization into the Python validator for the v2
  probe wire, the v2 metadata envelope, the collector-only output and the
  oversize/fallback paths, and keep the v1 fixtures accepted unchanged.
- [ ] One bounded read-only native metadata attempt, only after #1382 is
  accepted and this change is published.
