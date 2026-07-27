//! Deterministic, offline evidence for Ward Gate-3 review.
//!
//! Probes are advisory: their status is serialized beside a staged proposal,
//! but no result applies or approves a write. Runtime/configuration failures
//! become `unscored` evidence rather than an implicit pass.

use std::path::Path;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::threads_gate;
use crate::ward::{
    Authorization, FileEdit, ProbeConfig, ProbeFormat, ProbeId, Proposal, Verdict, Ward, WardConfig,
};

const PROTECTED_START: &str = "<!-- ward:protected -->";
const PROTECTED_END: &str = "<!-- /ward:protected -->";
type CompiledProbeMatcher<'a> = (&'a ProbeConfig, globset::GlobMatcher);

/// One surface's immutable staging-time probe evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SurfaceProbeReport {
    /// Target exactly as stored in the pending proposal.
    pub target: String,
    /// Gate-2-resolved familiar-home-relative surface.
    pub surface: String,
    /// SHA-256 of the current file, or `null` when the file does not exist.
    pub baseline_sha256: Option<String>,
    /// SHA-256 of the full proposed end state.
    pub proposed_sha256: String,
    /// Aggregate status for this surface.
    pub status: ProbeStatus,
    /// A baseline read error. Individual matching probes are also unscored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Results in `ward.toml` declaration order.
    pub results: Vec<ProbeResult>,
}

/// One configured deterministic check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeResult {
    pub id: ProbeId,
    pub configured_surface: String,
    /// SHA-256 of the full typed `[[probe]]` declaration used for this result.
    pub configuration_sha256: String,
    pub status: ProbeStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub detail: Value,
}

/// A probe's deterministic outcome. No variant carries approval authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProbeStatus {
    Passed,
    Failed,
    Unscored,
}

/// Compact evidence shown by pending-proposal list surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeSummary {
    pub status: ProbeStatus,
    pub passed: usize,
    pub failed: usize,
    pub unscored: usize,
    pub targets: usize,
}

impl ProbeSummary {
    pub(crate) fn unscored_targets(targets: usize) -> Self {
        Self {
            status: ProbeStatus::Unscored,
            passed: 0,
            failed: 0,
            unscored: targets,
            targets,
        }
    }

    pub(crate) fn from_reports(reports: &[SurfaceProbeReport]) -> Self {
        let mut passed = 0;
        let mut failed = 0;
        let mut unscored = 0;
        for report in reports {
            if report.results.is_empty() {
                unscored += 1;
                continue;
            }
            for result in &report.results {
                match result.status {
                    ProbeStatus::Passed => passed += 1,
                    ProbeStatus::Failed => failed += 1,
                    ProbeStatus::Unscored => unscored += 1,
                }
            }
        }
        let status = if failed > 0 {
            ProbeStatus::Failed
        } else if unscored > 0 || passed == 0 {
            ProbeStatus::Unscored
        } else {
            ProbeStatus::Passed
        };
        Self {
            status,
            passed,
            failed,
            unscored,
            targets: reports.len(),
        }
    }
}

/// Run the configured probe set against the exact end-state edits being
/// staged. Gate 1-2 are re-evaluated here solely to obtain confined, resolved
/// surface paths; a newly blocked target aborts staging rather than recording
/// evidence for an unsafe path.
pub(crate) fn run_at_staging(
    workspace: &Path,
    config: &WardConfig,
    edits: &[FileEdit],
    authorization: &Authorization,
) -> Result<Vec<SurfaceProbeReport>> {
    let ward = Ward::new(workspace, config.clone())?;
    let outcome = ward.evaluate(&Proposal {
        targets: edits.iter().map(|edit| edit.target.clone()).collect(),
        authorization: authorization.clone(),
    });
    if outcome.decisions.len() != edits.len() {
        bail!("Ward returned an incomplete probe adjudication");
    }
    let compiled_probes = compile_probe_matchers(config)?;
    let mut targets = std::collections::BTreeSet::new();
    let mut surfaces = std::collections::BTreeSet::new();
    for (edit, decision) in edits.iter().zip(&outcome.decisions) {
        if matches!(decision.verdict, Verdict::Blocked { .. }) {
            bail!(
                "probe target `{}` became blocked during staging",
                edit.target
            );
        }
        if !targets.insert(edit.target.as_str()) {
            bail!("probe staging requires unique edit targets");
        }
        if !surfaces.insert(crate::ward::portable_surface_key(&decision.resolved)) {
            bail!("probe staging requires unique resolved surfaces");
        }
    }

    edits
        .iter()
        .zip(outcome.decisions)
        .map(|(edit, decision)| {
            run_surface(
                workspace,
                &compiled_probes,
                &edit.target,
                &decision.resolved,
                &edit.new_contents,
            )
        })
        .collect()
}

fn compile_probe_matchers(config: &WardConfig) -> Result<Vec<CompiledProbeMatcher<'_>>> {
    config
        .probe
        .iter()
        .map(|probe| {
            probe
                .surface_matcher()
                .with_context(|| format!("invalid probe surface glob `{}`", probe.surface))
                .map(|matcher| (probe, matcher))
        })
        .collect()
}

fn run_surface(
    workspace: &Path,
    compiled_probes: &[CompiledProbeMatcher<'_>],
    target: &str,
    surface: &str,
    proposed: &[u8],
) -> Result<SurfaceProbeReport> {
    let matching = compiled_probes
        .iter()
        .filter_map(|(probe, matcher)| matcher.is_match(surface).then_some(*probe))
        .collect::<Vec<_>>();
    let proposed_sha256 = sha256_hex(proposed);
    let baseline = match threads_gate::read_surface_if_exists(workspace, surface) {
        Ok(baseline) => baseline,
        Err(error) => {
            let message = format!("current surface could not be read: {error:#}");
            let results = matching
                .iter()
                .map(|probe| ProbeResult {
                    id: probe.id,
                    configured_surface: probe.surface.clone(),
                    configuration_sha256: probe_config_sha256(probe),
                    status: ProbeStatus::Unscored,
                    summary: "Baseline unavailable; probe was not scored.".to_string(),
                    detail: json!({ "error": "baseline-unavailable" }),
                })
                .collect();
            return Ok(SurfaceProbeReport {
                target: target.to_string(),
                surface: surface.to_string(),
                baseline_sha256: None,
                proposed_sha256,
                status: ProbeStatus::Unscored,
                error: Some(message),
                results,
            });
        }
    };
    let baseline_bytes = baseline.as_deref().unwrap_or_default();
    let results: Vec<ProbeResult> = matching
        .into_iter()
        .map(|probe| run_probe(probe, baseline_bytes, proposed))
        .collect();
    let status = aggregate_results(&results);
    Ok(SurfaceProbeReport {
        target: target.to_string(),
        surface: surface.to_string(),
        baseline_sha256: baseline.as_deref().map(sha256_hex),
        proposed_sha256,
        status,
        error: None,
        results,
    })
}

/// Result of reconciling persisted evidence with current deterministic output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeEvidenceValidation {
    Valid,
    Stale,
    Inconsistent,
}

/// Current deterministic evidence plus its relationship to the staged sidecar.
pub(crate) struct RevalidatedProbeEvidence {
    pub validation: ProbeEvidenceValidation,
    pub current: Option<Vec<SurfaceProbeReport>>,
}

/// Recompute a persisted sidecar from the staged contents and current baseline.
/// Callers must summarize only `Valid` evidence; stale or inconsistent evidence
/// is explicitly unscored.
pub(crate) fn validate_staged_reports(
    workspace: &Path,
    config: &WardConfig,
    proposal: &coven_threads_core::PendingProposal,
    reports: &[SurfaceProbeReport],
) -> ProbeEvidenceValidation {
    revalidate_staged_reports(workspace, config, proposal, reports).validation
}

pub(crate) fn revalidate_staged_reports(
    workspace: &Path,
    config: &WardConfig,
    proposal: &coven_threads_core::PendingProposal,
    reports: &[SurfaceProbeReport],
) -> RevalidatedProbeEvidence {
    let Some(edits) = proposal
        .edits
        .iter()
        .map(|edit| {
            edit.contents
                .to_bytes()
                .ok()
                .map(|contents| FileEdit::new(edit.surface.as_str(), contents))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return RevalidatedProbeEvidence {
            validation: ProbeEvidenceValidation::Inconsistent,
            current: None,
        };
    };
    let authorization = proposal
        .writer
        .as_str()
        .strip_prefix("principal:")
        .map(|fingerprint| Authorization::signed_by(fingerprint.to_string()))
        .unwrap_or_else(Authorization::unsigned);
    let Ok(current) = run_at_staging(workspace, config, &edits, &authorization) else {
        return RevalidatedProbeEvidence {
            validation: ProbeEvidenceValidation::Inconsistent,
            current: None,
        };
    };

    let validation = if reports == current {
        ProbeEvidenceValidation::Valid
    } else {
        let same_binding = reports.len() == current.len()
            && reports
                .iter()
                .zip(&current)
                .all(|(stored, current)| same_evidence_binding(stored, current));
        let baseline_changed = reports.iter().zip(&current).any(|(stored, current)| {
            stored.baseline_sha256 != current.baseline_sha256 || stored.error != current.error
        });
        if same_binding && baseline_changed {
            ProbeEvidenceValidation::Stale
        } else {
            ProbeEvidenceValidation::Inconsistent
        }
    };
    RevalidatedProbeEvidence {
        validation,
        current: Some(current),
    }
}

fn same_evidence_binding(stored: &SurfaceProbeReport, current: &SurfaceProbeReport) -> bool {
    stored.target == current.target
        && stored.surface == current.surface
        && stored.proposed_sha256 == current.proposed_sha256
        && stored.results.len() == current.results.len()
        && stored
            .results
            .iter()
            .zip(&current.results)
            .all(|(stored, current)| {
                stored.id == current.id
                    && stored.configured_surface == current.configured_surface
                    && stored.configuration_sha256 == current.configuration_sha256
            })
}

fn aggregate_results(results: &[ProbeResult]) -> ProbeStatus {
    if results
        .iter()
        .any(|result| result.status == ProbeStatus::Failed)
    {
        ProbeStatus::Failed
    } else if results.is_empty()
        || results
            .iter()
            .any(|result| result.status == ProbeStatus::Unscored)
    {
        ProbeStatus::Unscored
    } else {
        ProbeStatus::Passed
    }
}

fn run_probe(probe: &ProbeConfig, baseline: &[u8], proposed: &[u8]) -> ProbeResult {
    let (status, summary, detail) = match probe.id {
        ProbeId::Parse => run_parse(probe.format, proposed),
        ProbeId::SizeDelta => run_size_delta(baseline, proposed),
        ProbeId::ProtectedRegion => run_protected_region(baseline, proposed),
        ProbeId::PatternLint => run_pattern_lint(&probe.forbidden, &probe.required, proposed),
    };
    ProbeResult {
        id: probe.id,
        configured_surface: probe.surface.clone(),
        configuration_sha256: probe_config_sha256(probe),
        status,
        summary,
        detail,
    }
}

fn run_parse(format: Option<ProbeFormat>, proposed: &[u8]) -> (ProbeStatus, String, Value) {
    let Some(format) = format else {
        return (
            ProbeStatus::Unscored,
            "No format was declared; parse probe was not scored.".to_string(),
            json!({ "error": "format-not-declared" }),
        );
    };
    let result = match format {
        ProbeFormat::Toml => std::str::from_utf8(proposed)
            .context("TOML is not UTF-8")
            .and_then(|text| {
                toml::from_str::<toml::Value>(text)
                    .context("invalid TOML")
                    .map(|_| ())
            }),
        ProbeFormat::Json => serde_json::from_slice::<Value>(proposed)
            .context("invalid JSON")
            .map(|_| ()),
        ProbeFormat::MarkdownFrontMatter => parse_markdown_front_matter(proposed),
    };
    let format_value = serde_json::to_value(format).unwrap_or(Value::Null);
    match result {
        Ok(()) => (
            ProbeStatus::Passed,
            "Proposed contents parse as the declared format.".to_string(),
            json!({ "format": format_value }),
        ),
        Err(error) => (
            ProbeStatus::Failed,
            "Proposed contents do not parse as the declared format.".to_string(),
            json!({ "format": format_value, "error": error.to_string() }),
        ),
    }
}

fn parse_markdown_front_matter(proposed: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(proposed).context("Markdown is not UTF-8")?;
    let Some(after_open) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        bail!("Markdown front matter must start with a `---` fence");
    };
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        let candidate = line.strip_suffix('\n').unwrap_or(line);
        let candidate = candidate.strip_suffix('\r').unwrap_or(candidate);
        if candidate == "---" {
            let metadata = &after_open[..offset];
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(metadata)
                .context("invalid YAML front matter")?;
            return Ok(());
        }
        offset += line.len();
    }
    bail!("Markdown front matter has no closing `---` fence")
}

fn run_size_delta(baseline: &[u8], proposed: &[u8]) -> (ProbeStatus, String, Value) {
    let before_bytes = baseline.len() as i64;
    let after_bytes = proposed.len() as i64;
    let before_lines = logical_line_count(baseline) as i64;
    let after_lines = logical_line_count(proposed) as i64;
    (
        ProbeStatus::Passed,
        "Size delta calculated.".to_string(),
        json!({
            "beforeBytes": before_bytes,
            "afterBytes": after_bytes,
            "bytesDelta": after_bytes - before_bytes,
            "beforeLines": before_lines,
            "afterLines": after_lines,
            "linesDelta": after_lines - before_lines,
        }),
    )
}

fn logical_line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    bytes.iter().filter(|byte| **byte == b'\n').count() + usize::from(bytes.last() != Some(&b'\n'))
}

fn run_protected_region(baseline: &[u8], proposed: &[u8]) -> (ProbeStatus, String, Value) {
    let baseline_regions = match protected_regions(baseline) {
        Ok(regions) => regions,
        Err(error) => {
            return (
                ProbeStatus::Unscored,
                "Current protected regions could not be parsed.".to_string(),
                json!({ "error": error.to_string() }),
            )
        }
    };
    let proposed_regions = match protected_regions(proposed) {
        Ok(regions) => regions,
        Err(error) => {
            return (
                ProbeStatus::Failed,
                "Proposed protected-region fences are malformed.".to_string(),
                json!({ "error": error.to_string() }),
            )
        }
    };
    let region_count = baseline_regions.len();
    if baseline_regions == proposed_regions {
        (
            ProbeStatus::Passed,
            if region_count == 0 {
                "No protected regions are present.".to_string()
            } else {
                "Protected regions are unchanged.".to_string()
            },
            json!({ "regions": region_count }),
        )
    } else {
        (
            ProbeStatus::Failed,
            "A protected region was added, removed, relocated, reordered, or changed.".to_string(),
            json!({
                "baselineRegions": region_count,
                "proposedRegions": proposed_regions.len(),
            }),
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ProtectedRegion {
    start_line: usize,
    bytes: String,
}

fn protected_regions(bytes: &[u8]) -> Result<Vec<ProtectedRegion>> {
    let text = std::str::from_utf8(bytes).context("protected-region surface is not UTF-8")?;
    let mut regions = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let remaining = &text[cursor..];
        let next_start = remaining.find(PROTECTED_START);
        let next_end = remaining.find(PROTECTED_END);
        match (next_start, next_end) {
            (None, None) => break,
            (None, Some(_)) => bail!("closing protected marker has no opening marker"),
            (Some(start), Some(end)) if end < start => {
                bail!("closing protected marker appears before its opening marker")
            }
            (Some(start), _) => {
                let start = cursor + start;
                let content_start = start + PROTECTED_START.len();
                let tail = &text[content_start..];
                let Some(end_offset) = tail.find(PROTECTED_END) else {
                    bail!("opening protected marker has no closing marker");
                };
                if tail[..end_offset].contains(PROTECTED_START) {
                    bail!("nested protected regions are not supported");
                }
                let end = content_start + end_offset + PROTECTED_END.len();
                let line_start = text[..start]
                    .rfind('\n')
                    .map(|index| index + 1)
                    .unwrap_or(0);
                regions.push(ProtectedRegion {
                    start_line: text[..line_start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count(),
                    bytes: text[line_start..end].to_string(),
                });
                cursor = end;
            }
        }
    }
    Ok(regions)
}

fn run_pattern_lint(
    forbidden: &[String],
    required: &[String],
    proposed: &[u8],
) -> (ProbeStatus, String, Value) {
    if forbidden.is_empty() && required.is_empty() {
        return (
            ProbeStatus::Unscored,
            "No forbidden or required patterns were configured.".to_string(),
            json!({ "error": "patterns-not-declared" }),
        );
    }
    let text = match std::str::from_utf8(proposed) {
        Ok(text) => text,
        Err(_) => {
            return (
                ProbeStatus::Unscored,
                "Pattern lint requires UTF-8 proposed contents.".to_string(),
                json!({ "error": "proposed-contents-not-utf8" }),
            )
        }
    };
    let forbidden = match compile_patterns("forbidden", forbidden) {
        Ok(patterns) => patterns,
        Err(detail) => {
            return (
                ProbeStatus::Unscored,
                "A forbidden regex is invalid; pattern lint was not scored.".to_string(),
                detail,
            )
        }
    };
    let required = match compile_patterns("required", required) {
        Ok(patterns) => patterns,
        Err(detail) => {
            return (
                ProbeStatus::Unscored,
                "A required regex is invalid; pattern lint was not scored.".to_string(),
                detail,
            )
        }
    };
    let forbidden_matches: Vec<usize> = forbidden
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| pattern.is_match(text).then_some(index))
        .collect();
    let missing_required: Vec<usize> = required
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| (!pattern.is_match(text)).then_some(index))
        .collect();
    let detail = json!({
        "forbiddenPatterns": forbidden.len(),
        "requiredPatterns": required.len(),
        "forbiddenMatches": forbidden_matches,
        "missingRequired": missing_required,
    });
    if detail["forbiddenMatches"]
        .as_array()
        .is_some_and(|matches| !matches.is_empty())
        || detail["missingRequired"]
            .as_array()
            .is_some_and(|missing| !missing.is_empty())
    {
        (
            ProbeStatus::Failed,
            "Pattern lint found forbidden or missing required content.".to_string(),
            detail,
        )
    } else {
        (
            ProbeStatus::Passed,
            "Pattern lint passed.".to_string(),
            detail,
        )
    }
}

fn compile_patterns(kind: &str, patterns: &[String]) -> std::result::Result<Vec<Regex>, Value> {
    patterns
        .iter()
        .enumerate()
        .map(|(index, pattern)| {
            Regex::new(pattern)
                .map_err(|error| json!({ "error": "invalid-regex", "kind": kind, "index": index, "detail": error.to_string() }))
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn probe_config_sha256(probe: &ProbeConfig) -> String {
    let serialized =
        serde_json::to_vec(probe).expect("typed Ward probe configuration always serializes");
    sha256_hex(&serialized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ward::{Authorization, FileEdit, WardConfig};

    fn config_with_all_probes() -> WardConfig {
        WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "reviewed/**"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "parse"
format = "markdown-front-matter"

[[probe]]
surface = "reviewed/**"
id = "size-delta"

[[probe]]
surface = "reviewed/**"
id = "protected-region"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
required = ["(?m)^name: sage$"]
"#,
        )
        .expect("probe config parses")
    }

    #[test]
    fn probe_matchers_compile_in_declaration_order_for_reuse() {
        let config = config_with_all_probes();

        let compiled = compile_probe_matchers(&config).unwrap();

        assert_eq!(compiled.len(), config.probe.len());
        assert!(compiled
            .iter()
            .all(|(_, matcher)| matcher.is_match("reviewed/SKILL.md")));
        assert!(compiled
            .iter()
            .all(|(_, matcher)| !matcher.is_match("notes/SKILL.md")));
        assert_eq!(
            compiled
                .iter()
                .map(|(probe, _)| probe.id)
                .collect::<Vec<_>>(),
            config
                .probe
                .iter()
                .map(|probe| probe.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn all_four_deterministic_probes_pass_and_capture_snapshot_hashes() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path();
        std::fs::create_dir_all(workspace.join("reviewed")).unwrap();
        std::fs::write(
            workspace.join("reviewed/SKILL.md"),
            "---\nname: sage\n---\nBefore\n<!-- ward:protected -->\nfixed\n<!-- /ward:protected -->\n",
        )
        .unwrap();
        let edits = vec![FileEdit::new(
            "reviewed/SKILL.md",
            "---\nname: sage\n---\nAfter\n<!-- ward:protected -->\nfixed\n<!-- /ward:protected -->\n",
        )];

        let reports = run_at_staging(
            workspace,
            &config_with_all_probes(),
            &edits,
            &Authorization::unsigned(),
        )
        .unwrap();

        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(report.surface, "reviewed/SKILL.md");
        assert_eq!(report.status, ProbeStatus::Passed);
        assert_eq!(report.baseline_sha256.as_deref().map(str::len), Some(64));
        assert_eq!(report.proposed_sha256.len(), 64);
        assert_eq!(report.results.len(), 4);
        assert!(report
            .results
            .iter()
            .all(|result| result.status == ProbeStatus::Passed));
        assert_eq!(ProbeSummary::from_reports(&reports).passed, 4);
    }

    #[test]
    fn deterministic_failures_and_probe_errors_remain_advisory_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path();
        std::fs::write(
            workspace.join("reviewed.json"),
            "<!-- ward:protected -->\nfixed\n<!-- /ward:protected -->\n",
        )
        .unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "reviewed.json"
tier = 1

[[probe]]
surface = "reviewed.json"
id = "parse"
format = "json"

[[probe]]
surface = "reviewed.json"
id = "protected-region"

[[probe]]
surface = "reviewed.json"
id = "pattern-lint"
forbidden = ["["]
"#,
        )
        .unwrap();
        let edits = vec![FileEdit::new(
            "reviewed.json",
            b"{not json}\n<!-- ward:protected -->\nchanged\n<!-- /ward:protected -->\n".to_vec(),
        )];

        let reports =
            run_at_staging(workspace, &config, &edits, &Authorization::unsigned()).unwrap();
        let results = &reports[0].results;

        assert_eq!(reports[0].status, ProbeStatus::Failed);
        assert_eq!(results[0].status, ProbeStatus::Failed);
        assert_eq!(results[1].status, ProbeStatus::Failed);
        assert_eq!(results[2].status, ProbeStatus::Unscored);
        let summary = ProbeSummary::from_reports(&reports);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.unscored, 1);
        assert_eq!(summary.status, ProbeStatus::Failed);
    }

    #[test]
    fn parse_probe_accepts_each_declared_v1_format() {
        let temp = tempfile::tempdir().unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "*.toml"
tier = 1

[[surface]]
path = "*.json"
tier = 1

[[surface]]
path = "*.md"
tier = 1

[[probe]]
surface = "*.toml"
id = "parse"
format = "toml"

[[probe]]
surface = "*.json"
id = "parse"
format = "json"

[[probe]]
surface = "*.md"
id = "parse"
format = "markdown-front-matter"
"#,
        )
        .unwrap();
        let edits = vec![
            FileEdit::new("identity.toml", "name = \"sage\"\n"),
            FileEdit::new("identity.json", r#"{"name":"sage"}"#),
            FileEdit::new("identity.md", "---\nname: sage\n---\n# Sage\n"),
        ];

        let reports =
            run_at_staging(temp.path(), &config, &edits, &Authorization::unsigned()).unwrap();

        assert_eq!(reports.len(), 3);
        assert!(reports
            .iter()
            .all(|report| report.status == ProbeStatus::Passed));
    }

    #[test]
    fn markdown_front_matter_requires_opening_and_closing_fences() {
        let temp = tempfile::tempdir().unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "*.md"
tier = 1

[[probe]]
surface = "*.md"
id = "parse"
format = "markdown-front-matter"
"#,
        )
        .unwrap();
        let edits = vec![
            FileEdit::new("missing-open.md", "name: sage\n---\n# Sage\n"),
            FileEdit::new("missing-close.md", "---\nname: sage\n# Sage\n"),
        ];

        let reports =
            run_at_staging(temp.path(), &config, &edits, &Authorization::unsigned()).unwrap();

        assert_eq!(reports.len(), 2);
        assert!(reports
            .iter()
            .all(|report| report.status == ProbeStatus::Failed));
    }

    #[test]
    fn markdown_front_matter_rejects_invalid_yaml_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "*.md"
tier = 1

[[probe]]
surface = "*.md"
id = "parse"
format = "markdown-front-matter"
"#,
        )
        .unwrap();
        let edits = vec![FileEdit::new(
            "invalid-front-matter.md",
            "---\nname: [\n---\n# Sage\n",
        )];

        let reports =
            run_at_staging(temp.path(), &config, &edits, &Authorization::unsigned()).unwrap();

        assert_eq!(reports[0].status, ProbeStatus::Failed);
        assert_eq!(reports[0].results[0].status, ProbeStatus::Failed);
    }

    #[test]
    fn protected_region_rejects_relocation_or_marker_reindentation() {
        let baseline = b"intro\n<!-- ward:protected -->\nfixed\n<!-- /ward:protected -->\noutro\n";
        let relocated = b"intro\noutro\n<!-- ward:protected -->\nfixed\n<!-- /ward:protected -->\n";
        let reindented =
            b"intro\n  <!-- ward:protected -->\nfixed\n<!-- /ward:protected -->\noutro\n";

        assert_eq!(
            run_protected_region(baseline, relocated).0,
            ProbeStatus::Failed
        );
        assert_eq!(
            run_protected_region(baseline, reindented).0,
            ProbeStatus::Failed
        );
    }

    #[test]
    fn pattern_lint_reports_forbidden_matches_and_missing_requirements() {
        let temp = tempfile::tempdir().unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "MEMORY.md"
tier = 1

[[probe]]
surface = "MEMORY.md"
id = "pattern-lint"
forbidden = ["(?i)secret"]
required = ["(?m)^name:"]
"#,
        )
        .unwrap();
        let edits = vec![FileEdit::new("MEMORY.md", "contains a SECRET")];

        let reports =
            run_at_staging(temp.path(), &config, &edits, &Authorization::unsigned()).unwrap();
        let result = &reports[0].results[0];

        assert_eq!(result.status, ProbeStatus::Failed);
        assert_eq!(result.detail["forbiddenMatches"], json!([0]));
        assert_eq!(result.detail["missingRequired"], json!([0]));
    }

    #[test]
    fn staging_refuses_nonportable_resolved_surface_aliases() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("reviewed")).unwrap();
        for (left, right) in [
            ("reviewed/skill.md", "reviewed/SKILL.md"),
            ("reviewed/caf\u{e9}.md", "reviewed/cafe\u{301}.md"),
        ] {
            let edits = vec![
                FileEdit::new(left, "---\nname: sage\n---\n"),
                FileEdit::new(right, "---\nname: sage\n---\n"),
            ];

            let error = run_at_staging(
                temp.path(),
                &config_with_all_probes(),
                &edits,
                &Authorization::unsigned(),
            )
            .expect_err("nonportable aliases must not produce conflicting probe reports");

            assert!(
                error.to_string().contains("unique resolved surfaces"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn absent_probe_config_is_explicitly_unscored() {
        let temp = tempfile::tempdir().unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "reviewed/**"
tier = 1
"#,
        )
        .unwrap();
        let edits = vec![FileEdit::new("reviewed/new.md", "new")];

        let reports =
            run_at_staging(temp.path(), &config, &edits, &Authorization::unsigned()).unwrap();

        assert_eq!(reports[0].status, ProbeStatus::Unscored);
        assert!(reports[0].results.is_empty());
        assert_eq!(reports[0].baseline_sha256, None);
        let summary = ProbeSummary::from_reports(&reports);
        assert_eq!(summary.unscored, 1);
        assert_eq!(summary.status, ProbeStatus::Unscored);
    }

    #[test]
    fn unreadable_baseline_demotes_matching_probes_to_unscored() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("reviewed")).unwrap();
        let config = WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val"
protected_surface = []

[[surface]]
path = "reviewed"
tier = 1

[[probe]]
surface = "reviewed"
id = "size-delta"
"#,
        )
        .unwrap();
        let edits = vec![FileEdit::new("reviewed", "replacement")];

        let reports =
            run_at_staging(temp.path(), &config, &edits, &Authorization::unsigned()).unwrap();

        assert_eq!(reports[0].status, ProbeStatus::Unscored);
        assert_eq!(reports[0].results[0].status, ProbeStatus::Unscored);
        assert!(reports[0].error.is_some());
    }
}
