//! Architectural guard for the proposal apply-intent journal, run by workspace
//! tests. It detects dependency/visibility drift; it is not a sandbox.
use syn::{visit::Visit, Item, UseTree, Visibility};

const IMPORTS: &[&str] = &[
    "std::collections::BTreeMap",
    "anyhow::Context",
    "anyhow::Result",
    "serde::Deserialize",
    "serde::Serialize",
    "crate::store",
];

/// The coordinator decides when to append and what recovery does with a read.
const EXPOSED_TYPES: &[&str] = &[
    "ProposalApplyingState",
    "ProposalBeforeImage",
    "RecordedProposalApplyIntent",
];
const EXPOSED_FUNCTIONS: &[&str] = &[
    "append_proposal_apply_intent",
    "invalid_proposal_apply_intent",
    "load_proposal_apply_intent",
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
            ["crate", "ward_probes", "ProbeSummary"] => true,
            ["store", "durable_transaction"] => true,
            ["std", "fmt", ..] | ["std", "error", "Error"] => true,
            ["crate" | "super" | "self" | "std" | "core" | "store", ..] => false,
            [root, _, ..] => matches!(
                *root,
                "anyhow"
                    | "serde"
                    | "serde_json"
                    | "rusqlite"
                    | "coven_threads_core"
                    | "time"
                    | "Self"
                    | "Option"
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
                // A function-local import of rusqlite's optional-row extension.
                if !IMPORTS.contains(&path.as_str()) && path != "rusqlite::OptionalExtension" {
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

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Struct(s) => {
                let exposed = !matches!(s.vis, Visibility::Inherited);
                if exposed
                    && (!is_super(&s.vis) || !EXPOSED_TYPES.contains(&s.ident.to_string().as_str()))
                {
                    self.violations.push(format!("exposed struct {}", s.ident));
                }
                for field in &s.fields {
                    let field_exposed = !matches!(field.vis, Visibility::Inherited);
                    if field_exposed && (!exposed || !is_super(&field.vis)) {
                        self.violations
                            .push(format!("exposed field in {}", s.ident));
                    }
                }
            }
            Item::Fn(f) if !matches!(f.vis, Visibility::Inherited) => {
                if !is_super(&f.vis)
                    || !EXPOSED_FUNCTIONS.contains(&f.sig.ident.to_string().as_str())
                {
                    self.violations
                        .push(format!("exposed function {}", f.sig.ident));
                }
            }
            Item::Fn(_) | Item::Impl(_) | Item::Use(_) => {}
            Item::Mod(m)
                if m.ident == "tests"
                    && m.content.is_none()
                    && matches!(m.vis, Visibility::Inherited)
                    && m.attrs.len() == 1
                    && m.attrs[0].path().is_ident("cfg")
                    && m.attrs[0]
                        .parse_args::<syn::Path>()
                        .is_ok_and(|p| p.is_ident("test")) => {}
            // Enums, statics, constants and other modules belong elsewhere.
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
fn apply_intent_journal_keeps_policy_and_file_io_outside_its_boundary() {
    let failures = violations(include_str!("../src/api/proposal_apply_intent.rs"));
    assert!(
        failures.is_empty(),
        "apply-intent boundary violations: {failures:?}"
    );
}

#[test]
fn guard_rejects_authority_bypasses_and_dependency_drift() {
    for source in [
        "use crate::ward;",
        "use super::*;",
        "use super::ProposalEnvelopeDocument;",
        "use std::fs;",
        "use crate::store as persistence;",
        "fn f() { std::fs::write(\"intent\", b\"\"); }",
        "fn f() { crate::ward::Ward::apply(); }",
        "fn f() { store::open_store(); }",
        "fn f() { super::decide_threads_proposal(); }",
        "fn f() { format!(\"{:?}\", crate::proposal_store::remove_existing()); }",
        "pub(crate) fn load_proposal_apply_intent() {}",
        "pub(super) fn delete_proposal_apply_intent() {}",
        "pub(super) struct InvalidProposalApplyIntent(String);",
        "struct InvalidProposalApplyIntent(pub(super) String);",
        "pub(super) struct ProposalApplyingState { pub(crate) decision: String }",
        "enum Mode { A }",
        "static CACHE: () = ();",
        "mod alternate;",
    ] {
        assert!(!violations(source).is_empty(), "guard missed {source}");
    }
    assert!(violations(
        "use crate::store; pub(super) fn load_proposal_apply_intent() -> anyhow::Result<()> { \
         use rusqlite::OptionalExtension; store::durable_transaction(); Ok(()) }"
    )
    .is_empty());
}
