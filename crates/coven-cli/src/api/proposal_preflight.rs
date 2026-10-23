//! Bounded structural and content-budget validation of persisted proposal envelopes.
//! This module neither authorizes a proposal nor performs I/O. The API coordinator
//! retains typed authority decoding, policy, claims, filesystem and store access.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::ward;

// Stored proposals duplicate bounded content in tagged JSON strings and
// decimal byte arrays. The multiplier covers their worst-case encoding forms;
// the additive allowance covers keys, paths, lifecycle data, and whitespace.
pub(super) const PROPOSAL_ENVELOPE_SYNTAX_MAX_BYTES: u64 = 4 * 1024 * 1024;
const PROPOSAL_ENVELOPE_CONTENT_ENCODING_FACTOR: u64 = 24;
pub(super) const PROPOSAL_ENVELOPE_MAX_BYTES: u64 = ward::WARD_RETAINED_CONTENT_MAX_BYTES
    * PROPOSAL_ENVELOPE_CONTENT_ENCODING_FACTOR
    + PROPOSAL_ENVELOPE_SYNTAX_MAX_BYTES;
pub(super) const PROPOSAL_JSON_MAX_DEPTH: usize = 32;
const PROPOSAL_JSON_MAX_OBJECT_FIELDS: usize = 64;
const PROPOSAL_JSON_MAX_GENERIC_ARRAY_ITEMS: usize = 4_096;
const PROPOSAL_JSON_MAX_PROBE_RESULTS: usize = 256;
const PROPOSAL_JSON_MAX_KEY_BYTES: usize = 128;
const PROPOSAL_JSON_MAX_METADATA_STRING_BYTES: usize = 64 * 1024;
const PROPOSAL_JSON_MAX_ESTIMATED_HEAP_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct ProposalEnvelopeLimits {
    max_edits: usize,
    max_content_bytes: u64,
}

const WARD_PROPOSAL_ENVELOPE_LIMITS: ProposalEnvelopeLimits = ProposalEnvelopeLimits {
    max_edits: ward::WARD_EDIT_MAX_COUNT,
    max_content_bytes: ward::WARD_RETAINED_CONTENT_MAX_BYTES,
};

#[derive(Debug, Clone, Copy, Default)]
struct ProposalContentCollection {
    count: usize,
    bytes: u64,
}

impl ProposalContentCollection {
    fn push_len(&mut self, bytes: u64) {
        self.count = self.count.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct JsonByteArrayLen(u64);

impl<'de> Deserialize<'de> for JsonByteArrayLen {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ByteArrayVisitor;

        impl<'de> serde::de::Visitor<'de> for ByteArrayVisitor {
            type Value = JsonByteArrayLen;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON byte array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = 0_u64;
                while sequence.next_element::<u8>()?.is_some() {
                    bytes = bytes.saturating_add(1);
                }
                Ok(JsonByteArrayLen(bytes))
            }
        }

        deserializer.deserialize_seq(ByteArrayVisitor)
    }
}

#[derive(Debug, Clone, Copy)]
struct StagedDataPreflight {
    utf8_len: u64,
    base64_len: Option<u64>,
}

impl StagedDataPreflight {
    fn from_str(data: &str) -> Self {
        Self {
            utf8_len: u64::try_from(data.len()).unwrap_or(u64::MAX),
            base64_len: ward::staged_contents_decoded_len("base64", data).ok(),
        }
    }
}

impl<'de> Deserialize<'de> for StagedDataPreflight {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StagedDataVisitor;

        impl serde::de::Visitor<'_> for StagedDataVisitor {
            type Value = StagedDataPreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a staged-content data string")
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(StagedDataPreflight::from_str(value))
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_str(StagedDataVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StagedContentsPreflight {
    encoding: String,
    data: StagedDataPreflight,
}

impl StagedContentsPreflight {
    fn decoded_len(&self) -> Result<u64> {
        match self.encoding.as_str() {
            "utf8" => Ok(self.data.utf8_len),
            "base64" => self
                .data
                .base64_len
                .context("staged base64 contents are invalid"),
            encoding => anyhow::bail!("staged contents use unknown encoding `{encoding}`"),
        }
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingEditPreflight {
    #[serde(default)]
    surface: Option<serde::de::IgnoredAny>,
    contents: StagedContentsPreflight,
}

#[derive(Debug, Clone, Copy, Default)]
struct PendingEditsPreflight(ProposalContentCollection);

impl<'de> Deserialize<'de> for PendingEditsPreflight {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PendingEditsVisitor;

        impl<'de> serde::de::Visitor<'de> for PendingEditsVisitor {
            type Value = PendingEditsPreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a pending edits array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut collection = ProposalContentCollection::default();
                while let Some(edit) = sequence.next_element::<PendingEditPreflight>()? {
                    let bytes = edit
                        .contents
                        .decoded_len()
                        .map_err(serde::de::Error::custom)?;
                    collection.push_len(bytes);
                }
                Ok(PendingEditsPreflight(collection))
            }
        }

        deserializer.deserialize_seq(PendingEditsVisitor)
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum FrayReasonPreflight {
    ContentHashMismatch,
    SignatureInvalid,
    ManifestEntryMismatch,
    AuditTrailUnverifiable,
    RequiredStrandMissing { kind: serde::de::IgnoredAny },
    SerializationMarkerMismatch,
    Other(String),
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum SnapReasonPreflight {
    Revoked,
    MultipleStrandFray,
    PatternBroken,
    Other(String),
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum FrayOrSnapPreflight {
    NotCovered {
        channel: serde::de::IgnoredAny,
    },
    Frayed {
        strand: Option<serde::de::IgnoredAny>,
        channel: serde::de::IgnoredAny,
        reason: FrayReasonPreflight,
    },
    Snapped {
        channel: serde::de::IgnoredAny,
        reason: SnapReasonPreflight,
    },
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PendingProposalPreflight {
    #[serde(default)]
    id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    familiar_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    writer: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    channel: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    thread_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    fray: Option<FrayOrSnapPreflight>,
    #[serde(default)]
    edits: Option<PendingEditsPreflight>,
    #[serde(default)]
    staged_at: Option<serde::de::IgnoredAny>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MaterializedSurfacePreflight {
    #[serde(default)]
    surface: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    before: Option<JsonByteArrayLen>,
    #[serde(default)]
    after: Option<JsonByteArrayLen>,
}

#[derive(Debug, Clone, Copy, Default)]
struct MaterializedSurfacesPreflight {
    count: usize,
    before_bytes: u64,
    after_bytes: u64,
}

impl<'de> Deserialize<'de> for MaterializedSurfacesPreflight {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MaterializedSurfacesVisitor;

        impl<'de> serde::de::Visitor<'de> for MaterializedSurfacesVisitor {
            type Value = MaterializedSurfacesPreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a materialized surfaces array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut surfaces = MaterializedSurfacesPreflight::default();
                while let Some(surface) = sequence.next_element::<MaterializedSurfacePreflight>()? {
                    surfaces.count = surfaces.count.saturating_add(1);
                    surfaces.before_bytes = surfaces
                        .before_bytes
                        .saturating_add(surface.before.map_or(0, |bytes| bytes.0));
                    surfaces.after_bytes = surfaces
                        .after_bytes
                        .saturating_add(surface.after.map_or(0, |bytes| bytes.0));
                }
                Ok(surfaces)
            }
        }

        deserializer.deserialize_seq(MaterializedSurfacesVisitor)
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MaterializedDiffPreflight {
    #[serde(default)]
    surfaces: Option<MaterializedSurfacesPreflight>,
}

#[derive(Debug, Clone, Copy, Default)]
struct JsonSequenceLen(usize);

impl<'de> Deserialize<'de> for JsonSequenceLen {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SequenceVisitor;

        impl<'de> serde::de::Visitor<'de> for SequenceVisitor {
            type Value = JsonSequenceLen;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut count = 0_usize;
                while sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    count = count.saturating_add(1);
                }
                Ok(JsonSequenceLen(count))
            }
        }

        deserializer.deserialize_seq(SequenceVisitor)
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalDurationPreflight {
    secs: serde::de::IgnoredAny,
    nanos: serde::de::IgnoredAny,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalVetoPreflight {
    duration: ProposalDurationPreflight,
    min_visible: ProposalDurationPreflight,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ApprovalPathPreflight {
    AutoRegression { veto: Option<ProposalVetoPreflight> },
    FamiliarCoherence { veto: ProposalVetoPreflight },
    HumanApproval,
    HumanApprovalWithRationale,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProposalClassificationPreflight {
    #[serde(default)]
    proposal_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    familiar_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    channel: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    affected_surfaces: Option<JsonSequenceLen>,
    #[serde(default)]
    affected_regions: Option<JsonSequenceLen>,
    #[serde(default)]
    path_tier_floor: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    approval_path: Option<ApprovalPathPreflight>,
    #[serde(default)]
    evidence_replay_hash: Option<JsonByteArrayLen>,
    #[serde(default)]
    classified_at: Option<serde::de::IgnoredAny>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProposalBeforeImagePreflight {
    #[serde(default)]
    target: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    resolved: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    contents: Option<StagedContentsPreflight>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ProposalBeforeImagesPreflight(ProposalContentCollection);

impl<'de> Deserialize<'de> for ProposalBeforeImagesPreflight {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct BeforeImagesVisitor;

        impl<'de> serde::de::Visitor<'de> for BeforeImagesVisitor {
            type Value = ProposalBeforeImagesPreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a proposal before-images array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut collection = ProposalContentCollection::default();
                while let Some(image) = sequence.next_element::<ProposalBeforeImagePreflight>()? {
                    let bytes = image
                        .contents
                        .as_ref()
                        .map(StagedContentsPreflight::decoded_len)
                        .transpose()
                        .map_err(serde::de::Error::custom)?
                        .unwrap_or(0);
                    collection.push_len(bytes);
                }
                Ok(ProposalBeforeImagesPreflight(collection))
            }
        }

        deserializer.deserialize_seq(BeforeImagesVisitor)
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProbeSummaryPreflight {
    status: serde::de::IgnoredAny,
    passed: serde::de::IgnoredAny,
    failed: serde::de::IgnoredAny,
    unscored: serde::de::IgnoredAny,
    targets: serde::de::IgnoredAny,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct ProposalDecisionStatePreflight {
    #[serde(default)]
    decision: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    before_images: Option<ProposalBeforeImagesPreflight>,
    #[serde(default)]
    recovery_commitment: Option<JsonByteArrayLen>,
    #[serde(default)]
    baseline_snapshot: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    weave_hash: Option<JsonByteArrayLen>,
    #[serde(default)]
    rationale: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    probe_summary: Option<ProbeSummaryPreflight>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RegionEvidenceItemPreflight {
    #[serde(default)]
    region_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    affected_surfaces: Option<JsonSequenceLen>,
    #[serde(default)]
    min_path_tier: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    replay_bytes: Option<JsonByteArrayLen>,
    #[serde(default)]
    rationale: Option<serde::de::IgnoredAny>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RegionEvidencePreflight {
    collection: ProposalContentCollection,
    max_affected_surfaces: usize,
}

impl<'de> Deserialize<'de> for RegionEvidencePreflight {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RegionEvidenceVisitor;

        impl<'de> serde::de::Visitor<'de> for RegionEvidenceVisitor {
            type Value = RegionEvidencePreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a region evidence array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut collection = ProposalContentCollection::default();
                let mut max_affected_surfaces = 0;
                while let Some(evidence) = sequence.next_element::<RegionEvidenceItemPreflight>()? {
                    collection.push_len(evidence.replay_bytes.map_or(0, |bytes| bytes.0));
                    max_affected_surfaces = max_affected_surfaces
                        .max(evidence.affected_surfaces.map_or(0, |surfaces| surfaces.0));
                }
                Ok(RegionEvidencePreflight {
                    collection,
                    max_affected_surfaces,
                })
            }
        }

        deserializer.deserialize_seq(RegionEvidenceVisitor)
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ProposalLifecyclePreflight {
    AwaitingHumanApproval,
    VetoWindowOpen,
    ReadyForReplay,
    Blocked { reason: serde::de::IgnoredAny },
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalDecisionRequestPreflight {
    decision: serde::de::IgnoredAny,
    rationale: Option<serde::de::IgnoredAny>,
    claimed_at: serde::de::IgnoredAny,
    #[serde(default)]
    expected_revision: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    revision_required: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    expired: Option<serde::de::IgnoredAny>,
    #[serde(rename = "replacementProposalId", default)]
    replacement_proposal_id: Option<serde::de::IgnoredAny>,
    #[serde(rename = "replacementProposalRevision", default)]
    replacement_proposal_revision: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    decision_actor: Option<serde::de::IgnoredAny>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProposalEnvelopePreflight {
    #[serde(default)]
    schema: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pending: Option<PendingProposalPreflight>,
    #[serde(default)]
    edits: Option<PendingEditsPreflight>,
    #[serde(default)]
    id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    familiar_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    writer: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    channel: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    thread_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    fray: Option<FrayOrSnapPreflight>,
    #[serde(default)]
    staged_at: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    materialized_diff: Option<MaterializedDiffPreflight>,
    #[serde(default)]
    classification: Option<ProposalClassificationPreflight>,
    #[serde(default)]
    region_evidence: Option<RegionEvidencePreflight>,
    #[serde(default)]
    lifecycle: Option<ProposalLifecyclePreflight>,
    #[serde(default)]
    veto_deadline: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    earliest_close: Option<serde::de::IgnoredAny>,
    #[serde(rename = "reviewKind", default)]
    review_kind: Option<serde::de::IgnoredAny>,
    #[serde(rename = "identityEvidence", default)]
    identity_evidence: Option<serde::de::IgnoredAny>,
    #[serde(rename = "autoRegressionEvidence", default)]
    auto_regression_evidence: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    probes: Option<serde::de::IgnoredAny>,
    #[serde(rename = "decisionRequest", default)]
    decision_request: Option<ProposalDecisionRequestPreflight>,
    #[serde(rename = "decisionState", default)]
    decision_state: Option<ProposalDecisionStatePreflight>,
}

#[derive(Debug, Clone, Copy)]
struct ProposalJsonStructureLimits {
    max_depth: usize,
    max_object_fields: usize,
    max_generic_array_items: usize,
    max_probe_reports: usize,
    max_probe_results: usize,
    max_key_bytes: usize,
    max_metadata_string_bytes: usize,
    max_content_string_bytes: usize,
    max_estimated_heap_bytes: u64,
}

const WARD_PROPOSAL_JSON_STRUCTURE_LIMITS: ProposalJsonStructureLimits =
    ProposalJsonStructureLimits {
        max_depth: PROPOSAL_JSON_MAX_DEPTH,
        max_object_fields: PROPOSAL_JSON_MAX_OBJECT_FIELDS,
        max_generic_array_items: PROPOSAL_JSON_MAX_GENERIC_ARRAY_ITEMS,
        max_probe_reports: ward::WARD_EDIT_MAX_COUNT,
        max_probe_results: PROPOSAL_JSON_MAX_PROBE_RESULTS,
        max_key_bytes: PROPOSAL_JSON_MAX_KEY_BYTES,
        max_metadata_string_bytes: PROPOSAL_JSON_MAX_METADATA_STRING_BYTES,
        max_content_string_bytes: PROPOSAL_ENVELOPE_MAX_BYTES as usize,
        max_estimated_heap_bytes: PROPOSAL_JSON_MAX_ESTIMATED_HEAP_BYTES,
    };

#[derive(Debug, Clone, Copy)]
enum ProposalJsonValueContext {
    Root,
    Field(ProposalJsonField),
    ArrayItem(ProposalJsonField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProposalJsonField {
    ContentData,
    ByteArray,
    Edits,
    MaterializedSurfaces,
    BeforeImages,
    AffectedSurfaces,
    AffectedRegions,
    RegionEvidence,
    Probes,
    ProbeResults,
    Generic,
}

struct ProposalJsonStructureScanner<'a> {
    raw: &'a [u8],
    offset: usize,
    estimated_heap_bytes: u64,
    limits: ProposalJsonStructureLimits,
}

impl ProposalJsonField {
    fn for_key(key: &str) -> Self {
        match key {
            "data" => Self::ContentData,
            "before"
            | "after"
            | "replay_bytes"
            | "evidence_replay_hash"
            | "identityEvidence"
            | "autoRegressionEvidence"
            | "recoveryCommitment"
            | "weaveHash" => Self::ByteArray,
            "edits" => Self::Edits,
            "surfaces" => Self::MaterializedSurfaces,
            "beforeImages" => Self::BeforeImages,
            "affected_surfaces" => Self::AffectedSurfaces,
            "affected_regions" => Self::AffectedRegions,
            "region_evidence" => Self::RegionEvidence,
            "probes" => Self::Probes,
            "results" => Self::ProbeResults,
            _ => Self::Generic,
        }
    }
}

fn proposal_json_object_key_allowed(context: ProposalJsonValueContext, key: &str) -> bool {
    match context {
        ProposalJsonValueContext::Root => [
            "schema",
            "pending",
            "classification",
            "materialized_diff",
            "region_evidence",
            "lifecycle",
            "veto_deadline",
            "earliest_close",
            "id",
            "familiar_id",
            "writer",
            "channel",
            "thread_id",
            "fray",
            "edits",
            "staged_at",
            "reviewKind",
            "identityEvidence",
            "autoRegressionEvidence",
            "probes",
            "decisionRequest",
            "decisionState",
        ]
        .contains(&key),
        ProposalJsonValueContext::ArrayItem(ProposalJsonField::Probes) => [
            "target",
            "surface",
            "baselineSha256",
            "proposedSha256",
            "status",
            "error",
            "results",
        ]
        .contains(&key),
        ProposalJsonValueContext::ArrayItem(ProposalJsonField::ProbeResults) => [
            "id",
            "configuredSurface",
            "configurationSha256",
            "status",
            "summary",
            "detail",
        ]
        .contains(&key),
        _ => true,
    }
}

impl ProposalJsonStructureScanner<'_> {
    fn error(&self, message: impl Into<String>) -> anyhow::Error {
        serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.into(),
        ))
        .into()
    }

    fn charge_heap(&mut self, bytes: u64) -> Result<()> {
        self.estimated_heap_bytes = self.estimated_heap_bytes.saturating_add(bytes);
        if self.estimated_heap_bytes > self.limits.max_estimated_heap_bytes {
            return Err(self.error(format!(
                "proposal JSON estimated heap exceeds {} bytes",
                self.limits.max_estimated_heap_bytes
            )));
        }
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self
            .raw
            .get(self.offset)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.offset += 1;
        }
    }

    fn parse(mut self) -> Result<()> {
        self.skip_whitespace();
        self.parse_value(0, ProposalJsonValueContext::Root)?;
        self.skip_whitespace();
        if self.offset != self.raw.len() {
            return Err(self.error("proposal JSON has trailing data"));
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize, context: ProposalJsonValueContext) -> Result<()> {
        if depth > self.limits.max_depth {
            return Err(self.error(format!(
                "proposal JSON nesting depth exceeds {}",
                self.limits.max_depth
            )));
        }
        self.skip_whitespace();
        match self.raw.get(self.offset).copied() {
            Some(b'{') => self.parse_object(depth, context),
            Some(b'[') => self.parse_array(depth, context),
            Some(b'"') => {
                let field = match context {
                    ProposalJsonValueContext::Field(field)
                    | ProposalJsonValueContext::ArrayItem(field) => field,
                    ProposalJsonValueContext::Root => ProposalJsonField::Generic,
                };
                let max = if field == ProposalJsonField::ContentData {
                    self.limits.max_content_string_bytes
                } else {
                    self.limits.max_metadata_string_bytes
                };
                let len = self.parse_string(max, false)?;
                self.charge_heap(u64::try_from(len).unwrap_or(u64::MAX))
            }
            Some(b't') => self.parse_literal(b"true"),
            Some(b'f') => self.parse_literal(b"false"),
            Some(b'n') => self.parse_literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.parse_number(context),
            Some(byte) => Err(self.error(format!(
                "proposal JSON contains unexpected byte 0x{byte:02x}"
            ))),
            None => Err(self.error("proposal JSON ended before a value")),
        }
    }

    fn parse_object(&mut self, depth: usize, context: ProposalJsonValueContext) -> Result<()> {
        self.offset += 1;
        self.charge_heap(32)?;
        self.skip_whitespace();
        if self.raw.get(self.offset) == Some(&b'}') {
            self.offset += 1;
            return Ok(());
        }
        let mut fields = 0_usize;
        loop {
            self.skip_whitespace();
            let key_start = self.offset;
            let key_len = self.parse_string(self.limits.max_key_bytes, true)?;
            let key: String = serde_json::from_slice(&self.raw[key_start..self.offset])
                .map_err(anyhow::Error::from)?;
            if !proposal_json_object_key_allowed(context, &key) {
                return Err(self.error(format!(
                    "proposal JSON object contains unknown field `{key}`"
                )));
            }
            fields = fields.saturating_add(1);
            if fields > self.limits.max_object_fields {
                return Err(self.error(format!(
                    "proposal JSON object contains more than {} fields",
                    self.limits.max_object_fields
                )));
            }
            self.charge_heap(
                u64::try_from(key_len)
                    .unwrap_or(u64::MAX)
                    .saturating_add(48),
            )?;
            self.skip_whitespace();
            if self.raw.get(self.offset) != Some(&b':') {
                return Err(self.error("proposal JSON object field is missing `:`"));
            }
            self.offset += 1;
            self.parse_value(
                depth.saturating_add(1),
                ProposalJsonValueContext::Field(ProposalJsonField::for_key(&key)),
            )?;
            self.skip_whitespace();
            match self.raw.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(());
                }
                _ => return Err(self.error("proposal JSON object is missing `,` or closing `}`")),
            }
        }
    }

    fn parse_array(&mut self, depth: usize, context: ProposalJsonValueContext) -> Result<()> {
        self.offset += 1;
        self.charge_heap(24)?;
        self.skip_whitespace();
        if self.raw.get(self.offset) == Some(&b']') {
            self.offset += 1;
            return Ok(());
        }
        let field = match context {
            ProposalJsonValueContext::Field(field) | ProposalJsonValueContext::ArrayItem(field) => {
                field
            }
            ProposalJsonValueContext::Root => ProposalJsonField::Generic,
        };
        let max_items = match field {
            ProposalJsonField::ByteArray => {
                usize::try_from(ward::WARD_RETAINED_CONTENT_MAX_BYTES.saturating_mul(2))
                    .unwrap_or(usize::MAX)
            }
            ProposalJsonField::BeforeImages
            | ProposalJsonField::AffectedSurfaces
            | ProposalJsonField::AffectedRegions
            | ProposalJsonField::RegionEvidence => ward::WARD_EDIT_MAX_COUNT,
            ProposalJsonField::Edits | ProposalJsonField::MaterializedSurfaces => {
                self.limits.max_generic_array_items
            }
            ProposalJsonField::Probes => self.limits.max_probe_reports,
            ProposalJsonField::ProbeResults => self.limits.max_probe_results,
            ProposalJsonField::ContentData | ProposalJsonField::Generic => {
                self.limits.max_generic_array_items
            }
        };
        let mut items = 0_usize;
        loop {
            items = items.saturating_add(1);
            if items > max_items {
                let label = match field {
                    ProposalJsonField::Probes => "probes",
                    ProposalJsonField::ProbeResults => "probe results",
                    ProposalJsonField::Edits => "edits",
                    ProposalJsonField::MaterializedSurfaces => "materialized surfaces",
                    ProposalJsonField::BeforeImages => "before images",
                    ProposalJsonField::AffectedSurfaces => "affected surfaces",
                    ProposalJsonField::AffectedRegions => "affected regions",
                    ProposalJsonField::RegionEvidence => "region evidence",
                    ProposalJsonField::ByteArray => "byte array",
                    ProposalJsonField::ContentData | ProposalJsonField::Generic => "array",
                };
                return Err(self.error(format!(
                    "proposal JSON {label} contains more than {max_items} items"
                )));
            }
            self.charge_heap(if field == ProposalJsonField::ByteArray {
                1
            } else {
                24
            })?;
            self.parse_value(
                depth.saturating_add(1),
                ProposalJsonValueContext::ArrayItem(field),
            )?;
            self.skip_whitespace();
            match self.raw.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    return Ok(());
                }
                _ => return Err(self.error("proposal JSON array is missing `,` or closing `]`")),
            }
        }
    }

    fn parse_string(&mut self, max_bytes: usize, key: bool) -> Result<usize> {
        if self.raw.get(self.offset) != Some(&b'"') {
            return Err(self.error("proposal JSON object key is not a string"));
        }
        self.offset += 1;
        let content_start = self.offset;
        loop {
            let Some(byte) = self.raw.get(self.offset).copied() else {
                return Err(self.error("proposal JSON contains an unterminated string"));
            };
            match byte {
                b'"' => {
                    let raw_bytes = self.offset.saturating_sub(content_start);
                    self.offset += 1;
                    if raw_bytes > max_bytes {
                        let label = if key { "key" } else { "string" };
                        return Err(
                            self.error(format!("proposal JSON {label} exceeds {max_bytes} bytes"))
                        );
                    }
                    return Ok(raw_bytes);
                }
                b'\\' => {
                    self.offset += 1;
                    let Some(escape) = self.raw.get(self.offset).copied() else {
                        return Err(self.error("proposal JSON contains an incomplete escape"));
                    };
                    if escape == b'u' {
                        for _ in 0..4 {
                            self.offset += 1;
                            if !self
                                .raw
                                .get(self.offset)
                                .is_some_and(|byte| byte.is_ascii_hexdigit())
                            {
                                return Err(
                                    self.error("proposal JSON contains an invalid unicode escape")
                                );
                            }
                        }
                    } else if !matches!(
                        escape,
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't'
                    ) {
                        return Err(self.error("proposal JSON contains an invalid escape"));
                    }
                    self.offset += 1;
                }
                0x00..=0x1f => {
                    return Err(self.error("proposal JSON string contains a control byte"))
                }
                _ => self.offset += 1,
            }
            if self.offset.saturating_sub(content_start) > max_bytes {
                let label = if key { "key" } else { "string" };
                return Err(self.error(format!("proposal JSON {label} exceeds {max_bytes} bytes")));
            }
        }
    }

    fn parse_literal(&mut self, literal: &[u8]) -> Result<()> {
        if self
            .raw
            .get(self.offset..self.offset.saturating_add(literal.len()))
            != Some(literal)
        {
            return Err(self.error("proposal JSON contains an invalid literal"));
        }
        self.offset += literal.len();
        self.charge_heap(8)
    }

    fn parse_number(&mut self, context: ProposalJsonValueContext) -> Result<()> {
        let start = self.offset;
        while self
            .raw
            .get(self.offset)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.offset += 1;
        }
        if self.offset == start {
            return Err(self.error("proposal JSON contains an invalid number"));
        }
        self.charge_heap(
            if matches!(
                context,
                ProposalJsonValueContext::ArrayItem(ProposalJsonField::ByteArray)
            ) {
                1
            } else {
                16
            },
        )
    }
}

fn validate_proposal_json_structure(raw: &[u8]) -> Result<()> {
    ProposalJsonStructureScanner {
        raw,
        offset: 0,
        estimated_heap_bytes: 0,
        limits: WARD_PROPOSAL_JSON_STRUCTURE_LIMITS,
    }
    .parse()
}

fn validate_proposal_collection(
    collection: ProposalContentCollection,
    limits: ProposalEnvelopeLimits,
) -> Result<()> {
    if collection.count > limits.max_edits {
        return Err(ward::WardEditBudgetFailure::BatchEditCount {
            attempted_edits: collection.count,
            max_edits: limits.max_edits,
        }
        .into());
    }
    if collection.bytes == u64::MAX || collection.bytes > limits.max_content_bytes {
        return Err(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: collection.bytes,
            max_bytes: limits.max_content_bytes,
        }
        .into());
    }
    Ok(())
}

fn validate_combined_proposal_bytes(
    proposed_bytes: u64,
    before_bytes: u64,
    limits: ProposalEnvelopeLimits,
) -> Result<()> {
    let Some(attempted_bytes) = proposed_bytes.checked_add(before_bytes) else {
        return Err(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: u64::MAX,
            max_bytes: limits.max_content_bytes,
        }
        .into());
    };
    if attempted_bytes > limits.max_content_bytes {
        return Err(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes,
            max_bytes: limits.max_content_bytes,
        }
        .into());
    }
    Ok(())
}

fn validate_proposal_envelope_preflight_with_limits(
    raw: &[u8],
    limits: ProposalEnvelopeLimits,
) -> Result<ward::WardEditBudget> {
    validate_proposal_json_structure(raw)?;
    let envelope: ProposalEnvelopePreflight =
        serde_json::from_slice(raw).context("preflighting proposal envelope")?;
    let nested_edits = envelope.pending.and_then(|pending| pending.edits);
    if let Some(edits) = nested_edits {
        validate_proposal_collection(edits.0, limits)?;
    }
    if let Some(edits) = envelope.edits {
        validate_proposal_collection(edits.0, limits)?;
    }
    let pending = nested_edits
        .or(envelope.edits)
        .context("proposal envelope is missing pending edits")?;

    let materialized = envelope.materialized_diff.and_then(|diff| diff.surfaces);
    if let Some(surfaces) = materialized {
        validate_proposal_collection(
            ProposalContentCollection {
                count: surfaces.count,
                bytes: surfaces.after_bytes,
            },
            limits,
        )?;
        validate_proposal_collection(
            ProposalContentCollection {
                count: surfaces.count,
                bytes: surfaces.before_bytes,
            },
            limits,
        )?;
        validate_combined_proposal_bytes(pending.0.bytes, surfaces.before_bytes, limits)?;
    }

    let before_images = envelope
        .decision_state
        .as_ref()
        .and_then(|state| state.before_images);
    if let Some(before_images) = before_images {
        validate_proposal_collection(before_images.0, limits)?;
        validate_combined_proposal_bytes(pending.0.bytes, before_images.0.bytes, limits)?;
    }
    if let Some(state) = envelope.decision_state {
        for bytes in [state.recovery_commitment, state.weave_hash]
            .into_iter()
            .flatten()
        {
            validate_proposal_collection(
                ProposalContentCollection {
                    count: 1,
                    bytes: bytes.0,
                },
                limits,
            )?;
        }
    }
    if let Some(classification) = envelope.classification {
        for count in [
            classification.affected_surfaces,
            classification.affected_regions,
        ]
        .into_iter()
        .flatten()
        {
            validate_proposal_collection(
                ProposalContentCollection {
                    count: count.0,
                    bytes: 0,
                },
                limits,
            )?;
        }
        if let Some(bytes) = classification.evidence_replay_hash {
            validate_proposal_collection(
                ProposalContentCollection {
                    count: 1,
                    bytes: bytes.0,
                },
                limits,
            )?;
        }
    }
    if let Some(evidence) = envelope.region_evidence {
        validate_proposal_collection(evidence.collection, limits)?;
        validate_proposal_collection(
            ProposalContentCollection {
                count: evidence.max_affected_surfaces,
                bytes: 0,
            },
            limits,
        )?;
    }

    let mut budget = ward::WardEditBudget::for_edit_count(pending.0.count)?;
    budget.reserve_retained_content(pending.0.bytes)?;
    if let Some(before_images) = before_images {
        budget.reserve_retained_content(before_images.0.bytes)?;
    } else if let Some(surfaces) = materialized {
        budget.reserve_retained_content(surfaces.before_bytes)?;
    }
    Ok(budget)
}

pub(crate) fn validate_proposal_envelope_preflight(raw: &[u8]) -> Result<ward::WardEditBudget> {
    validate_proposal_envelope_preflight_with_limits(raw, WARD_PROPOSAL_ENVELOPE_LIMITS)
}

#[cfg(test)]
mod tests;
