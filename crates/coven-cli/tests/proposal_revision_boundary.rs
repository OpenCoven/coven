//! Architectural guard for the pure revision-digest boundary, run by workspace
//! tests. This detects dependency/visibility drift; it is not a Rust sandbox.
use syn::{visit::Visit, Item, UseTree, Visibility};

const IMPORTS: &[&str] = &[
    "std::collections::BTreeMap",
    "anyhow::Context",
    "anyhow::Result",
    "sha2::Digest",
    "sha2::Sha256",
    "super::proposal_preflight::PROPOSAL_JSON_MAX_DEPTH",
];

#[derive(Default)]
struct Boundary {
    violations: Vec<String>,
}

impl Boundary {
    fn check_path(&mut self, names: &[String]) {
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        let allowed = match names.as_slice() {
            ["super", "proposal_preflight", "PROPOSAL_JSON_MAX_DEPTH"] => true,
            ["crate" | "super" | "self" | "std" | "core", ..] => false,
            [root, _, ..] => matches!(
                *root,
                "anyhow" | "serde_json" | "Sha256" | "BTreeMap" | "Self"
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

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Fn(f) if !matches!(f.vis, Visibility::Inherited) => {
                if f.sig.ident != "hash_canonical_json"
                    || !matches!(&f.vis, Visibility::Restricted(v) if v.path.is_ident("super"))
                {
                    self.violations
                        .push(format!("exposed function {}", f.sig.ident));
                }
            }
            Item::Fn(_) | Item::Use(_) => {}
            Item::Mod(m)
                if m.ident == "tests"
                    && m.content.is_none()
                    && matches!(m.vis, Visibility::Inherited)
                    && m.attrs.len() == 1
                    && m.attrs[0].path().is_ident("cfg")
                    && m.attrs[0]
                        .parse_args::<syn::Path>()
                        .is_ok_and(|p| p.is_ident("test")) => {}
            // State, types and other modules belong to the coordinator.
            _ => self.violations.push("non-function item".into()),
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
fn revision_digest_keeps_policy_persistence_and_io_outside_its_boundary() {
    let failures = violations(include_str!("../src/api/proposal_revision.rs"));
    assert!(
        failures.is_empty(),
        "revision boundary violations: {failures:?}"
    );
}

#[test]
fn guard_rejects_authority_bypasses_and_dependency_drift() {
    for source in [
        "use crate::store;",
        "use super::*;",
        "use super::ProposalEnvelopeDocument;",
        "use sha2::Sha256 as Digest;",
        "use std::fs;",
        "fn f() { std::fs::read(\"proposal\"); }",
        "fn f() { crate::store::open_store(); }",
        "fn f() { super::decide_threads_proposal(); }",
        "fn f() { rusqlite::Connection::open(\"store\"); }",
        "fn f() { format!(\"{:?}\", crate::store::open_store()); }",
        "pub(crate) fn hash_canonical_json() {}",
        "pub(super) fn json_value_end() {}",
        "struct Revision;",
        "static CACHE: () = ();",
        "mod alternate;",
    ] {
        assert!(!violations(source).is_empty(), "guard missed {source}");
    }
    assert!(violations(
        "use anyhow::Result; pub(super) fn hash_canonical_json() -> Result<()> { \
         serde_json::to_vec(&1)?; Ok(()) }"
    )
    .is_empty());
}
