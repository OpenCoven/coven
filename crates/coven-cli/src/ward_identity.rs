use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use coven_threads_core::{
    CandidateIdentityContext, CandidateIdentityFact, CandidateIdentityFacts, IdentityFact,
};

use crate::cockpit_sources;
use crate::ward;

pub(crate) fn candidate_binding(
    config: &ward::WardConfig,
    context: Option<&CandidateIdentityContext>,
) -> anyhow::Result<Option<[u8; 32]>> {
    let Some(invariants) = config.identity_invariant_set()? else {
        return Ok(None);
    };
    let context =
        context.ok_or_else(|| anyhow::anyhow!("candidate identity evidence unavailable"))?;
    let declarations = serde_json::to_vec(&invariants)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"coven:ward-identity-evidence:v1");
    hasher.update(&(declarations.len() as u64).to_be_bytes());
    hasher.update(&declarations);
    hasher.update(&context.candidate_commitment);
    Ok(Some(*hasher.finalize().as_bytes()))
}

pub(crate) fn candidate_rejection(
    config: &ward::WardConfig,
    context: Option<&CandidateIdentityContext>,
) -> anyhow::Result<Option<coven_threads_core::Verdict>> {
    let Some(invariants) = config.identity_invariant_set()? else {
        return Ok(None);
    };
    let coherence = invariants.evaluate(
        context.map_or([0; 32], |context| context.candidate_commitment),
        context.map(|context| &context.facts),
    );
    Ok(match coherence {
        coven_threads_core::WeaveCoherence::Coherent => None,
        coven_threads_core::WeaveCoherence::Broken { reason }
        | coven_threads_core::WeaveCoherence::Degraded { reason, .. } => {
            Some(coven_threads_core::Verdict::Reject {
                reason: coven_threads_core::RejectReason::WeaveBroken { reason },
            })
        }
    })
}

pub(crate) fn candidate_identity_context(
    coven_home: &Path,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    edits: &[ward::FileEdit],
    authorization: &ward::Authorization,
    resolved_decisions: Option<&[ward::Decision]>,
) -> Option<CandidateIdentityContext> {
    let invariants = config.identity_invariant_set().ok().flatten()?;
    let required_facts: BTreeSet<_> = invariants
        .declarations()
        .iter()
        .map(|declaration| declaration.fact)
        .collect();
    let overrides = materialized_candidate_overrides(
        workspace,
        config,
        edits,
        authorization,
        resolved_decisions,
    )?;
    let source_ward = ward::Ward::new(workspace, config.clone()).ok()?;
    let soul_source = source_ward.materialize("SOUL.md").ok()?;
    let identity_source = source_ward.materialize("IDENTITY.md").ok()?;
    let roster = relevant_roster_source(coven_home, familiar_id, &required_facts);
    let soul = relevant_surface_source(
        workspace,
        "SOUL.md",
        &soul_source,
        &overrides,
        required_facts.contains(&IdentityFact::Name)
            || required_facts.contains(&IdentityFact::Purpose),
    );
    let identity = relevant_surface_source(
        workspace,
        "IDENTITY.md",
        &identity_source,
        &overrides,
        required_facts.contains(&IdentityFact::Name)
            || required_facts.contains(&IdentityFact::Pronouns),
    );

    let candidate_commitment = candidate_commitment([
        roster.commitment(),
        soul.commitment(),
        identity.commitment(),
    ]);
    let soul_facts = soul.parsed_text().map(parse_soul).unwrap_or_default();
    let identity_facts = identity
        .parsed_text()
        .map(parse_identity)
        .unwrap_or_default();
    let mut facts = Vec::with_capacity(required_facts.len());
    for fact in required_facts {
        let value = match fact {
            IdentityFact::Name => resolve_required_consistent([
                identity_facts.name.clone(),
                soul_facts.name.clone(),
                roster.name(),
            ]),
            IdentityFact::Person => roster.person(),
            IdentityFact::Pronouns => {
                resolve_required_consistent([identity_facts.pronouns.clone(), roster.pronouns()])
            }
            IdentityFact::Purpose => soul_facts.purpose.clone(),
            IdentityFact::Coven => roster.coven(),
        };
        if let Some(value) = value {
            facts.push(CandidateIdentityFact { fact, value });
        }
    }
    let facts = CandidateIdentityFacts::try_new(candidate_commitment, facts)
        .expect("candidate facts are canonical");
    Some(CandidateIdentityContext {
        candidate_commitment,
        facts,
    })
}

fn materialized_candidate_overrides(
    workspace: &Path,
    config: &ward::WardConfig,
    edits: &[ward::FileEdit],
    authorization: &ward::Authorization,
    resolved_decisions: Option<&[ward::Decision]>,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let resolved_by_target = resolved_decisions
        .map(|decisions| {
            decisions
                .iter()
                .filter(|decision| !decision.verdict.is_blocked())
                .map(|decision| (decision.target.clone(), decision.resolved.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .or_else(|| {
            let ward = ward::Ward::new(workspace, config.clone()).ok()?;
            let proposal = ward::Proposal {
                targets: edits.iter().map(|edit| edit.target.clone()).collect(),
                authorization: authorization.clone(),
            };
            Some(
                ward.evaluate(&proposal)
                    .decisions
                    .into_iter()
                    .filter(|decision| !decision.verdict.is_blocked())
                    .map(|decision| (decision.target, decision.resolved))
                    .collect::<BTreeMap<_, _>>(),
            )
        })?;
    let mut overrides = BTreeMap::new();
    for edit in edits {
        let resolved = resolved_by_target.get(edit.target.as_str())?;
        if overrides
            .insert(resolved.clone(), edit.new_contents.clone())
            .is_some()
        {
            return None;
        }
    }
    Some(overrides)
}

#[derive(Debug, Default, Clone)]
struct RosterFacts {
    name: Option<String>,
    person: Option<String>,
    pronouns: Option<String>,
    coven: Option<String>,
}

#[derive(Debug, Clone)]
struct RosterSource {
    facts: Option<RosterFacts>,
    serialized: Option<Vec<u8>>,
}

impl RosterSource {
    fn commitment(&self) -> CandidateSource {
        CandidateSource {
            name: "familiars.toml:familiar",
            bytes: self.serialized.clone(),
        }
    }

    fn name(&self) -> Option<String> {
        self.facts.as_ref().and_then(|facts| facts.name.clone())
    }

    fn person(&self) -> Option<String> {
        self.facts.as_ref().and_then(|facts| facts.person.clone())
    }

    fn pronouns(&self) -> Option<String> {
        self.facts.as_ref().and_then(|facts| facts.pronouns.clone())
    }

    fn coven(&self) -> Option<String> {
        self.facts.as_ref().and_then(|facts| facts.coven.clone())
    }
}

fn relevant_roster_source(
    coven_home: &Path,
    familiar_id: &str,
    required_facts: &BTreeSet<IdentityFact>,
) -> RosterSource {
    if !required_facts.iter().any(|fact| {
        matches!(
            fact,
            IdentityFact::Name
                | IdentityFact::Person
                | IdentityFact::Pronouns
                | IdentityFact::Coven
        )
    }) {
        return RosterSource {
            facts: None,
            serialized: None,
        };
    }
    let matching_entries = cockpit_sources::read_familiar_entries(coven_home)
        .ok()
        .map(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.id == familiar_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let [entry] = matching_entries.as_slice() else {
        return RosterSource {
            facts: None,
            serialized: None,
        };
    };
    let facts = RosterFacts {
        name: normalize_inline(entry.name.as_deref().unwrap_or(&entry.display_name)),
        person: entry.person.as_deref().and_then(normalize_inline),
        pronouns: entry.pronouns.as_deref().and_then(normalize_inline),
        coven: entry.coven.as_deref().and_then(normalize_inline),
    };
    match serde_json::to_vec(entry) {
        Ok(serialized) => RosterSource {
            facts: Some(facts),
            serialized: Some(serialized),
        },
        Err(_) => RosterSource {
            facts: None,
            serialized: None,
        },
    }
}

#[derive(Debug, Clone)]
struct SurfaceSource {
    name: &'static str,
    bytes: Option<Vec<u8>>,
}

impl SurfaceSource {
    fn commitment(&self) -> CandidateSource {
        CandidateSource {
            name: self.name,
            bytes: self.bytes.clone(),
        }
    }

    fn parsed_text(&self) -> Option<&str> {
        std::str::from_utf8(self.bytes.as_deref()?).ok()
    }
}

fn relevant_surface_source(
    workspace: &Path,
    surface: &'static str,
    resolved: &str,
    overrides: &BTreeMap<String, Vec<u8>>,
    needed: bool,
) -> SurfaceSource {
    if !needed {
        return SurfaceSource {
            name: surface,
            bytes: None,
        };
    }
    let bytes = overrides.get(resolved).cloned().or_else(|| {
        crate::threads_gate::read_surface_if_exists(workspace, resolved)
            .ok()
            .flatten()
    });
    SurfaceSource {
        name: surface,
        bytes,
    }
}

#[derive(Debug, Default, Clone)]
struct ParsedSoul {
    name: Option<String>,
    purpose: Option<String>,
}

fn parse_soul(text: &str) -> ParsedSoul {
    let mut name = ParsedValue::default();
    let mut purpose = ParsedValue::default();
    let lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if let Some(rest) = line.strip_prefix("## I am ") {
            name.consider(normalize_inline(rest));
        } else if let Some(rest) = line.strip_prefix("# I am ") {
            name.consider(normalize_inline(rest));
        } else if matches!(line, "## I am" | "# I am") {
            name.consider(None);
        }
        if let Some(rest) = line.strip_prefix("My purpose is ") {
            purpose.consider(normalize_inline(rest));
        } else if line.eq_ignore_ascii_case("## Purpose") {
            purpose.consider(collect_markdown_section(&lines, index + 1));
        } else if line == "My purpose is" {
            purpose.consider(None);
        }
        index += 1;
    }
    ParsedSoul {
        name: name.into_option(),
        purpose: purpose.into_option(),
    }
}

#[derive(Debug, Default, Clone)]
struct ParsedIdentity {
    name: Option<String>,
    pronouns: Option<String>,
}

fn parse_identity(text: &str) -> ParsedIdentity {
    let mut name = ParsedValue::default();
    let mut pronouns = ParsedValue::default();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("# IDENTITY.md -") {
            name.consider(normalize_inline(rest));
        } else if let Some(rest) = line.strip_prefix("- **Name:**") {
            name.consider(normalize_inline(rest));
        }
        if let Some(rest) = line.strip_prefix("- **Pronouns:**") {
            pronouns.consider(normalize_inline(rest));
        }
    }
    ParsedIdentity {
        name: name.into_option(),
        pronouns: pronouns.into_option(),
    }
}

fn collect_markdown_section(lines: &[&str], start: usize) -> Option<String> {
    let mut collected = Vec::new();
    for line in &lines[start..] {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            break;
        }
        if !trimmed.is_empty() {
            collected.push(trimmed);
        }
    }
    normalize_inline(&collected.join(" "))
}

fn normalize_inline(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn resolve_required_consistent<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    if values.iter().any(Option::is_none) {
        return None;
    }
    let mut present = values.into_iter().flatten();
    let first = present.next()?;
    if present.all(|candidate| candidate == first) {
        Some(first)
    } else {
        None
    }
}

#[derive(Debug, Default, Clone)]
struct ParsedValue {
    value: Option<String>,
    ambiguous: bool,
}

impl ParsedValue {
    fn consider(&mut self, candidate: Option<String>) {
        let Some(candidate) = candidate else {
            self.value = None;
            self.ambiguous = true;
            return;
        };
        if self.ambiguous {
            return;
        }
        match self.value.as_ref() {
            None => self.value = Some(candidate),
            Some(existing) if existing == &candidate => {}
            Some(_) => {
                self.value = None;
                self.ambiguous = true;
            }
        }
    }

    fn into_option(self) -> Option<String> {
        (!self.ambiguous).then_some(self.value).flatten()
    }
}

struct CandidateSource {
    name: &'static str,
    bytes: Option<Vec<u8>>,
}

fn candidate_commitment<const N: usize>(sources: [CandidateSource; N]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"coven:ward-candidate-identity:v1");
    for source in sources {
        hasher.update(&(source.name.len() as u64).to_be_bytes());
        hasher.update(source.name.as_bytes());
        match source.bytes {
            Some(bytes) => {
                hasher.update(&[1]);
                hasher.update(&(bytes.len() as u64).to_be_bytes());
                hasher.update(&bytes);
            }
            None => {
                hasher.update(&[0]);
            }
        }
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_declared_name_makes_identity_extraction_ambiguous() {
        let identity = parse_identity("# IDENTITY.md - Sage\n- **Name:**\n");
        assert!(identity.name.is_none());
    }
}
