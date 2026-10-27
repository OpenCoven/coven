//! Architectural guard for the pure preflight boundary, run by workspace tests.
//! This detects dependency/visibility drift; it is not a sandbox for Rust code.
use syn::{visit::Visit, Item, UseTree, Visibility};

#[derive(Default)]
struct Boundary {
    violations: Vec<String>,
    local_types: Vec<String>,
}

impl Boundary {
    fn check_path(&mut self, segments: &[String]) {
        let names: Vec<_> = segments.iter().map(String::as_str).collect();
        let allowed = match names.as_slice() {
            ["crate", "ward"] => true,
            ["crate", _, ..] | ["super", _, ..] => false,
            ["ward", member, ..] => matches!(
                *member,
                "WardEditBudget"
                    | "WardEditBudgetFailure"
                    | "WARD_EDIT_MAX_COUNT"
                    | "WARD_RETAINED_CONTENT_MAX_BYTES"
                    | "staged_contents_decoded_len"
            ),
            ["std", module, ..] => {
                matches!(*module, "fmt" | "result")
                    || matches!(
                        names.as_slice(),
                        ["std", "io", "Error", "new"] | ["std", "io", "ErrorKind", "InvalidData"]
                    )
            }
            [root, _, ..] => {
                self.local_types.iter().any(|name| name == root)
                    || matches!(
                        *root,
                        "anyhow" | "serde" | "serde_json" | "Self" | "A" | "D" | "u64" | "usize"
                    )
            }
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
                if !matches!(
                    path.as_str(),
                    "crate::ward" | "anyhow::Context" | "anyhow::Result" | "serde::Deserialize"
                ) {
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
        self.check_path(
            &path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>(),
        );
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Tokens are not recursively visited by syn. The qualified internal paths
        // are still visible in their token rendering (literals are conservatively
        // included); catch accidental store/API calls inside formatting macros.
        let path = regex::Regex::new(r"\b(crate|super)\s*::\s*([A-Za-z_][A-Za-z_0-9]*)").unwrap();
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
                if f.sig.ident != "validate_proposal_envelope_preflight"
                    || !matches!(&f.vis, Visibility::Restricted(v) if v.path.is_ident("crate"))
                {
                    self.violations
                        .push(format!("exposed function {}", f.sig.ident));
                }
            }
            Item::Const(c) if !matches!(c.vis, Visibility::Inherited) => {
                if !matches!(
                    c.ident.to_string().as_str(),
                    "PROPOSAL_ENVELOPE_MAX_BYTES"
                        | "PROPOSAL_ENVELOPE_SYNTAX_MAX_BYTES"
                        | "PROPOSAL_JSON_MAX_DEPTH"
                ) || !matches!(&c.vis, Visibility::Restricted(v) if v.path.is_ident("super"))
                {
                    self.violations
                        .push(format!("exposed constant {}", c.ident));
                }
            }
            Item::Struct(s) if !matches!(s.vis, Visibility::Inherited) => {
                self.violations.push("exposed structure".into())
            }
            Item::Enum(e) if !matches!(e.vis, Visibility::Inherited) => {
                self.violations.push("exposed enum".into())
            }
            Item::Mod(m)
                if m.ident != "tests"
                    || m.content.is_some()
                    || !matches!(m.vis, Visibility::Inherited)
                    || m.attrs.len() != 1
                    || !m.attrs[0].path().is_ident("cfg")
                    || !m.attrs[0]
                        .parse_args::<syn::Path>()
                        .is_ok_and(|p| p.is_ident("test")) =>
            {
                self.violations.push("nested production module".into());
            }
            _ => {}
        }
        syn::visit::visit_item(self, item);
    }
}

fn violations(source: &str) -> Vec<String> {
    let file = syn::parse_file(source).expect("valid Rust fixture");
    let local_types = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(item) => Some(item.ident.to_string()),
            Item::Enum(item) => Some(item.ident.to_string()),
            _ => None,
        })
        .collect();
    let mut guard = Boundary {
        local_types,
        ..Boundary::default()
    };
    guard.visit_file(&file);
    guard.violations
}

#[test]
fn preflight_keeps_policy_persistence_and_io_outside_its_boundary() {
    let failures = violations(include_str!("../src/api/proposal_preflight.rs"));
    assert!(
        failures.is_empty(),
        "preflight boundary violations: {failures:?}"
    );
}

#[test]
fn guard_rejects_authority_bypasses_and_dependency_drift() {
    for source in [
        "use crate::store;",
        "use super::*;",
        "use crate::ward as policy;",
        "fn f() { crate::store::open_store(); }",
        "fn f() { ward::Ward::apply(); }",
        "fn f() { std::fs::read(\"proposal\"); }",
        "fn f() { rusqlite::Connection::open(\"store\"); }",
        "pub(crate) fn decode_without_preflight() {}",
        "pub(crate) struct ProposalEnvelopeLimits;",
        "mod alternate;",
        "fn f() { format!(\"{:?}\", crate::store::open_store()); }",
    ] {
        assert!(!violations(source).is_empty(), "guard missed {source}");
    }
    assert!(
        violations("use crate::ward; fn f() { ward::WardEditBudget::for_edit_count(1); }")
            .is_empty()
    );
}
