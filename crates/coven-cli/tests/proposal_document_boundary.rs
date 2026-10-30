//! Architectural guard for the persisted proposal envelope boundary, run by
//! workspace tests. It detects dependency/visibility drift; it is not a sandbox.
use syn::{visit::Visit, Item, UseTree, Visibility};

const IMPORTS: &[&str] = &[
    "anyhow::Context",
    "anyhow::Result",
    "serde::Deserialize",
    "serde::Serialize",
    "sha2::Digest",
    "sha2::Sha256",
    "super::proposal_preflight::PROPOSAL_ENVELOPE_MAX_BYTES",
    "super::proposal_revision::hash_canonical_json",
    "super::ProposalApplyingState",
    "super::ProposalDecisionRequest",
    "crate::ward",
];

/// The coordinator reads and writes the decoded document; nothing else leaves.
const EXPOSED_TYPES: &[&str] = &["ProposalAuthority", "ProposalEnvelopeDocument"];
const EXPOSED_METHODS: &[&str] = &[
    "parse_preflighted",
    "pending",
    "scheduled",
    "serialize_with_decision",
    "authority_bytes",
    "revision",
];

#[derive(Default)]
struct Boundary {
    violations: Vec<String>,
}

fn is_super(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Restricted(v) if v.path.is_ident("super"))
}

impl Boundary {
    fn check_path(&mut self, names: &[String]) {
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        let allowed = match names.as_slice() {
            ["crate", "proposal_scheduler", "ScheduledProposal" | "ProposalLifecycle", ..] => true,
            ["ward", "WardEditBudgetFailure", ..] => true,
            ["std", "result", "Result"] => true,
            ["crate" | "super" | "self" | "std" | "core" | "ward", ..] => false,
            [root, _, ..] => matches!(
                *root,
                "anyhow"
                    | "serde"
                    | "serde_json"
                    | "coven_threads_core"
                    | "time"
                    | "Self"
                    | "D"
                    | "T"
                    | "ProposalAuthority"
                    | "Sha256"
                    | "Option"
                    | "Box"
                    | "u64"
            ),
            _ => true,
        };
        if !allowed {
            self.violations
                .push(format!("dependency {}", names.join("::")));
        }
    }

    fn check_use(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.check_use(&path.tree, prefix);
                prefix.pop();
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.check_use(item, prefix);
                }
            }
            UseTree::Name(name) => {
                let mut full = prefix.clone();
                full.push(name.ident.to_string());
                let path = full.join("::");
                if !IMPORTS.contains(&path.as_str()) {
                    self.violations.push(format!("import {path}"));
                }
            }
            UseTree::Rename(_) | UseTree::Glob(_) => {
                self.violations.push("renamed or glob import".into())
            }
        }
    }
}

impl<'ast> Visit<'ast> for Boundary {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.len() > 1 {
            self.check_path(
                &path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>(),
            );
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // syn does not visit macro tokens; catch qualified internal paths there.
        let path =
            regex::Regex::new(r"\b(crate|super|std)\s*::\s*([A-Za-z_][A-Za-z_0-9]*)").unwrap();
        for capture in path.captures_iter(&node.tokens.to_string()) {
            self.check_path(&[capture[1].to_owned(), capture[2].to_owned()]);
        }
        syn::visit::visit_macro(self, node);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if !matches!(item.vis, Visibility::Inherited) {
            self.violations.push("public import".into());
        }
        self.check_use(&item.tree, &mut Vec::new());
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let exposed = !matches!(item.vis, Visibility::Inherited);
        if exposed
            && (!is_super(&item.vis)
                || !EXPOSED_METHODS.contains(&item.sig.ident.to_string().as_str()))
        {
            self.violations
                .push(format!("exposed method {}", item.sig.ident));
        }
        syn::visit::visit_impl_item_fn(self, item);
    }

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Struct(s) if !matches!(s.vis, Visibility::Inherited) => {
                if !is_super(&s.vis) || !EXPOSED_TYPES.contains(&s.ident.to_string().as_str()) {
                    self.violations.push(format!("exposed struct {}", s.ident));
                }
                for field in &s.fields {
                    if !matches!(field.vis, Visibility::Inherited) && !is_super(&field.vis) {
                        self.violations.push(format!("wider field in {}", s.ident));
                    }
                }
            }
            Item::Struct(s) => {
                if s.fields
                    .iter()
                    .any(|field| !matches!(field.vis, Visibility::Inherited))
                {
                    self.violations
                        .push(format!("exposed field in {}", s.ident));
                }
            }
            Item::Enum(e) if !matches!(e.vis, Visibility::Inherited) => {
                if !is_super(&e.vis) || !EXPOSED_TYPES.contains(&e.ident.to_string().as_str()) {
                    self.violations.push(format!("exposed enum {}", e.ident));
                }
            }
            Item::Fn(f) if !matches!(f.vis, Visibility::Inherited) => {
                self.violations
                    .push(format!("exposed function {}", f.sig.ident));
            }
            Item::Enum(_) | Item::Fn(_) | Item::Impl(_) | Item::Use(_) => {}
            Item::Mod(m)
                if m.ident == "tests"
                    && m.content.is_none()
                    && matches!(m.vis, Visibility::Inherited)
                    && m.attrs.len() == 1
                    && m.attrs[0].path().is_ident("cfg")
                    && m.attrs[0]
                        .parse_args::<syn::Path>()
                        .is_ok_and(|p| p.is_ident("test")) => {}
            // Statics, constants and other modules belong elsewhere.
            _ => self.violations.push("unexpected item".into()),
        }
        syn::visit::visit_item(self, item);
    }
}

fn violations(source: &str) -> Vec<String> {
    let mut guard = Boundary::default();
    guard.visit_file(&syn::parse_file(source).expect("valid Rust fixture"));
    guard.violations
}

#[test]
fn proposal_document_keeps_claims_policy_and_io_outside_its_boundary() {
    let failures = violations(include_str!("../src/api/proposal_document.rs"));
    assert!(
        failures.is_empty(),
        "proposal document boundary violations: {failures:?}"
    );
}

#[test]
fn guard_rejects_authority_bypasses_and_dependency_drift() {
    for source in [
        "use crate::store;",
        "use super::*;",
        "use super::decide_threads_proposal;",
        "use std::fs;",
        "use crate::ward as policy;",
        "fn f() { std::fs::read(\"proposal\"); }",
        "fn f() { crate::store::open_store(); }",
        "fn f() { ward::Ward::apply(); }",
        "fn f() { super::read_bounded_proposal_file(); }",
        "fn f() { rusqlite::Connection::open(\"store\"); }",
        "fn f() { format!(\"{:?}\", crate::store::open_store()); }",
        "pub(super) fn parse_unchecked() {}",
        "pub(crate) struct ProposalEnvelopeDocument;",
        "pub(super) struct ProposalEnvelopeWire;",
        "struct ProposalEnvelopeWire { pub(super) schema: u8 }",
        "pub(super) struct ProposalEnvelopeDocument { pub(crate) authority: u8 }",
        "impl ProposalEnvelopeDocument { pub(super) fn bypass(&self) {} }",
        "impl ProposalEnvelopeDocument { pub(crate) fn revision(&self) {} }",
        "static CACHE: () = ();",
        "mod alternate;",
    ] {
        assert!(!violations(source).is_empty(), "guard missed {source}");
    }
    assert!(violations(
        "use crate::ward; pub(super) struct ProposalEnvelopeDocument { pub(super) probes: u8 } \
         impl ProposalEnvelopeDocument { pub(super) fn revision(&self) -> anyhow::Result<()> { \
         Err(ward::WardEditBudgetFailure::Other.into()) } }"
    )
    .is_empty());
}
