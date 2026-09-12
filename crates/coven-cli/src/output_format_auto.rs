//! Opt-in, finite output-format regression authority; all other probes stay advisory.

use anyhow::{ensure, Context, Result};
use coven_threads_core::{MaterializedDiff, OutputFormatRegion, SurfaceRegionId};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::ward::{Decision, ProbeFormat, ProbeId, Tier, WardConfig};
use crate::ward_probes::{ProbeStatus, SurfaceProbeReport};

pub(crate) fn intercepts(
    workspace: &Path,
    config: &WardConfig,
    decisions: &[Decision],
) -> Result<bool> {
    if !config.editable.as_ref().is_some_and(|editable| {
        editable
            .harness_blocks
            .iter()
            .any(|region| region == "output_format")
    }) {
        return Ok(false);
    }
    let canonical = crate::ward::portable_surface_key(OutputFormatRegion::SURFACE);
    let mut intercepted = false;
    for decision in decisions {
        let declared = crate::ward::lexical_join(Path::new(""), &decision.target)
            .context("output-format routing requires a confined declared target")?;
        let declared_matches =
            crate::ward::portable_surface_key(&declared.to_string_lossy()) == canonical;
        let resolved_matches = crate::ward::portable_surface_key(&decision.resolved) == canonical;
        ensure!(
            !declared_matches || resolved_matches,
            "declared output-format target resolves to an unsupported surface"
        );
        intercepted |= declared_matches || resolved_matches;
    }
    if !intercepted {
        ensure!(
            !crate::ward::has_resolved_file_alias(
                workspace,
                OutputFormatRegion::SURFACE,
                decisions
            )?,
            "output-format redirection destinations and hardlink aliases are not supported proposal targets"
        );
    }
    Ok(intercepted)
}

pub(crate) fn validate_opt_in(config: &WardConfig, diff: &MaterializedDiff) -> Result<()> {
    OutputFormatRegion::validate(diff).map_err(anyhow::Error::msg)?;
    ensure!(
        config.surface.iter().any(|surface| {
            surface.path == OutputFormatRegion::SURFACE && surface.tier == Tier::Logged
        }) && config.classify_resolved_path(OutputFormatRegion::SURFACE)? == Tier::Logged,
        "output-format requires an explicit literal and effective tier 2 declaration"
    );
    ensure!(
        config
            .compiled_approval_tiers()?
            .and_then(|bindings| bindings
                .approval_path_for(&SurfaceRegionId::new("output_format"))
                .cloned())
            .is_some(),
        "output-format requires a compiled approval binding"
    );
    Ok(())
}

pub(crate) fn regression_evidence(
    config: &WardConfig,
    diff: &MaterializedDiff,
    evidence_hash: &[u8; 32],
    identity_evidence: Option<[u8; 32]>,
) -> Result<([u8; 32], Vec<SurfaceProbeReport>)> {
    validate_opt_in(config, diff)?;
    let has_json_parser = config.probe.iter().try_fold(false, |found, probe| {
        Ok::<_, anyhow::Error>(
            found
                || (probe
                    .surface_matcher()?
                    .is_match(OutputFormatRegion::SURFACE)
                    && probe.id == ProbeId::Parse
                    && probe.format == Some(ProbeFormat::Json)),
        )
    })?;
    ensure!(
        has_json_parser,
        "output-format auto requires an applicable parse/json probe"
    );
    let reports = crate::ward_probes::run_materialized(config, diff)?;
    ensure!(
        reports.len() == 1
            && reports.iter().all(|report| {
                report.error.is_none()
                    && report.status == ProbeStatus::Passed
                    && !report.results.is_empty()
                    && report
                        .results
                        .iter()
                        .all(|result| result.status == ProbeStatus::Passed)
            }),
        "output-format auto requires every applicable probe to freshly pass"
    );
    let projection = authoritative_projection(&reports);
    let bytes = serde_json::to_vec(&(evidence_hash, identity_evidence, config, projection))
        .context("serializing output-format regression commitment")?;
    let mut digest = Sha256::new();
    digest.update(b"coven:output-format-auto:v1");
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    Ok((digest.finalize().into(), reports))
}

pub(crate) fn authoritative_projection(reports: &[SurfaceProbeReport]) -> Value {
    json!(reports
        .iter()
        .map(|report| json!({
            "target": report.target,
            "surface": report.surface,
            "before": report.baseline_sha256,
            "after": report.proposed_sha256,
            "status": report.status,
            "error": report.error.is_some(),
            "results": report.results.iter().map(|result| json!({
                "id": result.id,
                "configuration": result.configuration_sha256,
                "surface": result.configured_surface,
                "status": result.status,
                "detail": result.detail,
            })).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>())
}
