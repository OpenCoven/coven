# Psyche O2 Documentation Integration Design

**Status:** Approved for documentation-only implementation  
**Issue:** #689  
**Scope:** Reconcile the proposed O2 execution-binding contract with the
canonical Psyche documentation. No production API, schema, or runtime behavior
is implemented by this work.

## Decision

`psyche.execution_binding.v1` is a staged delivery sequence, not one complete
runtime capability:

| Phase | Responsibility | Status |
| --- | --- | --- |
| O2 | Immutable opaque binding, syntax validation, exact-match correlation, and expiry handling | Proposed contract |
| O3 | Adoption key, uniqueness, and replay/duplicate-adoption handling | Planned |
| O4 | Adoption lookup, return-or-fence behavior, and binding queries | Planned |
| O5 | Cancellation acknowledgement or unresolved disposition | Planned |
| O6 | Content-addressed result and artifact binding | Planned |
| O7 | Crash-safe persistence and recovery across O2-O6 | Planned |

The O2 contract remains explicit that exact comparison detects
cross-binding substitution only. It does not prevent replay or duplicate
adoption. Coven persists and compares opaque data; Psyche retains ownership of
identity, graph semantics, delegation policy, and orchestration.

## Documentation changes

1. Keep `O2_CONTRACT_DESIGN.md` as the detailed, proposed source for O2's
   request/response shape, persistence, validation, error outcomes, and test
   map.
2. Update `RUNTIME_DESIGN.md` to replace its composite execution-binding
   wording with the staged O2-O7 boundary and link to the O2 proposal.
3. Update `TECH.md` to describe the same phased capability in its contract
   table and schema/persistence narrative; it must no longer imply adoption,
   cursor, cancellation, or terminal correlation are O2 fields.
4. Update `PLAN.md` and `INTEGRATION_REVIEW.md` with an O2 companion link and
   a concise delivery-boundary summary. These documents remain program and
   review aids, not competing contract specifications.
5. Change the local-artifact ignore rule from `.psyche*` to `/.psyche*` so it
   affects only repository-root Psyche artifacts.

No public API reference is changed because O2 is not implemented. The detailed
O2 file is marked proposed everywhere it is linked.

## Source precedence

`O2_CONTRACT_DESIGN.md` is authoritative only for the proposed O2 contract.
`RUNTIME_DESIGN.md` remains authoritative for the product and ownership
boundary, and is amended to point to the staged delivery model. The W1 audit
continues to define the evidence-based O2-O8 ordering. If a document conflicts
with the proposed O2 scope, it must describe the deferred behavior as O3-O7
rather than adding it to O2.

## Validation

- Confirm all affected documents use the same O2-O7 allocation and describe
  O2 as proposed.
- Run whitespace, secret, and staged privacy checks.
- Run the repository's documentation-relevant checks; this documentation-only
  change does not alter Rust or TypeScript behavior.
- Review the final diff to ensure no O3-O7 implementation or release claim is
  introduced.
