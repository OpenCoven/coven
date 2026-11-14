//! Architectural guard for the proposal decision-origin authority, run by
//! workspace tests. It detects dependency/visibility drift; it is not a sandbox.
use syn::{visit::Visit, Item, UseTree, Visibility};

const IMPORTS: &[&str] = &[
    "anyhow::Context",
    "anyhow::Result",
    "sha2::Digest",
    "sha2::Sha256",
    "super::proposal_apply_intent::invalid_proposal_apply_intent",
    "super::proposal_apply_intent::load_proposal_apply_intent",
    "super::HistoricalDecisionReviewRequired",
    "super::ProposalDecisionRequest",
    "super::OWNER_LOCAL_DECISION_ACTOR",
    // Function-local trait imports.
    "rusqlite::OptionalExtension",
];

/// Claims, recovery and what each origin may do stay with the coordinator.
const EXPOSED: &[&str] = &[
    "TrustedProposalDecisionOrigin",
    "HISTORICAL_UNKNOWN_DECISION_ACTOR",
    "invalid_proposal_decision_origin",
    "trusted_decision_approver",
    "trusted_decision_approver_for_purpose",
    "bound_proposal_decision_purpose",
    "approver",
    "verify_apply_intent_actor",
];

#[derive(Default)]
struct Boundary {
    violations: Vec<String>,
}

impl Boundary {
    fn exposed(&mut self, vis: &Visibility, name: &syn::Ident) {
        let exposed = !matches!(vis, Visibility::Inherited);
        let restricted = matches!(vis, Visibility::Restricted(v) if v.path.is_ident("super"));
        if exposed && (!restricted || !EXPOSED.contains(&name.to_string().as_str())) {
            self.violations.push(format!("exposed {name}"));
        }
    }

    fn check_path(&mut self, names: &[String]) {
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        let allowed = match names.as_slice() {
            ["std", "fmt", ..] | ["std", "error", "Error"] => true,
            ["crate" | "super" | "self" | "std" | "core", ..] => false,
            [root, _, ..] => matches!(
                *root,
                "anyhow"
                    | "rusqlite"
                    | "time"
                    | "coven_threads_core"
                    | "serde_json"
                    | "Self"
                    | "Sha256"
                    | "String"
                    | "TrustedProposalDecisionOrigin"
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
            // `use std::fmt::Write as _` brings a trait into scope for `write!`.
            UseTree::Rename(rename)
                if prefix.join("::") == "std::fmt"
                    && rename.ident == "Write"
                    && rename.rename == "_" => {}
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
        self.exposed(&item.vis, &item.sig.ident);
        syn::visit::visit_impl_item_fn(self, item);
    }

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Fn(f) => self.exposed(&f.vis, &f.sig.ident),
            Item::Enum(e) => self.exposed(&e.vis, &e.ident),
            Item::Const(c) => self.exposed(&c.vis, &c.ident),
            Item::Struct(s) => {
                self.exposed(&s.vis, &s.ident);
                if s.fields
                    .iter()
                    .any(|field| !matches!(field.vis, Visibility::Inherited))
                {
                    self.violations
                        .push(format!("exposed field in {}", s.ident));
                }
            }
            Item::Impl(_) | Item::Use(_) => {}
            Item::Mod(m)
                if m.ident == "tests"
                    && m.content.is_none()
                    && matches!(m.vis, Visibility::Inherited)
                    && m.attrs.len() == 1
                    && m.attrs[0].path().is_ident("cfg")
                    && m.attrs[0]
                        .parse_args::<syn::Path>()
                        .is_ok_and(|p| p.is_ident("test")) => {}
            // Statics and other modules belong elsewhere.
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
fn decision_origin_keeps_claims_recovery_and_io_outside_its_boundary() {
    let failures = violations(include_str!("../src/api/proposal_decision_origin.rs"));
    assert!(
        failures.is_empty(),
        "decision-origin boundary violations: {failures:?}"
    );
}

#[test]
fn guard_rejects_authority_bypasses_and_dependency_drift() {
    for source in [
        "use crate::ward;",
        "use super::*;",
        "use super::decide_threads_proposal;",
        "use super::proposal_apply_intent::append_proposal_apply_intent;",
        "use std::fs;",
        "use sha2::Sha256 as Hash;",
        "fn f() { std::fs::read(\"claim\"); }",
        "fn f() { crate::store::open_store(); }",
        "fn f() { super::find_pending_proposal(); }",
        "fn f() { format!(\"{:?}\", crate::proposal_store::remove_existing()); }",
        "pub(crate) fn trusted_decision_approver() {}",
        "pub(super) fn trust_without_reservation() {}",
        "pub(super) struct InvalidProposalDecisionOrigin(String);",
        "struct InvalidProposalDecisionOrigin(pub(super) String);",
        "pub(super) const OWNER: &str = \"owner\";",
        "impl TrustedProposalDecisionOrigin { pub(super) fn force_owner() {} }",
        "static CACHE: () = ();",
        "mod alternate;",
    ] {
        assert!(!violations(source).is_empty(), "guard missed {source}");
    }
    assert!(violations(
        "use super::ProposalDecisionRequest; pub(super) fn bound_proposal_decision_purpose() { \
         use std::fmt::Write as _; let _ = serde_json::to_vec(&1); }"
    )
    .is_empty());
}
