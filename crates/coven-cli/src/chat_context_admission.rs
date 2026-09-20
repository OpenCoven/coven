//! Ordinary-chat context intent, not identity or execution authority.
//!
//! No production Familiar embodiment verifier/current-ledger adapter is wired
//! into this binary, and no native context-isolation profile is qualified.
//! Explicit requests therefore fail before execution; they never downgrade to
//! legacy chat or acquire a success-shaped, client-authored receipt.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::fmt;
use std::io::Read;
use std::path::Path;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, value::RawValue, Value};

use crate::api_response::{api_error, ApiResponse};
use crate::automations::contract::{sha256_digest, sha256_hex as digest};
use crate::request_authority::RequestAuthority;

const MAX_REQUEST_BYTES: usize = 65_536;
const PROFILE: &str = "coven.chat_context_admission.v1";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    profile: String,
    manifest: Manifest,
    manifest_digest: String,
    #[serde(deserialize_with = "required_nullable")]
    expected_receipt: Option<ReceiptReference>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    identity: IdentityReference,
    familiar_id: String,
    harness: String,
    #[serde(deserialize_with = "required_nullable")]
    model: Option<String>,
    adapter_profile: String,
    mode: ContextMode,
    project_root: String,
    resource_refs: Vec<String>,
    selections: Vec<Category>,
    sources: Vec<Source>,
    prompt_digest: String,
    launch_policy_digest: String,
    retention: Retention,
    optional_memory_policy: OptionalMemoryPolicy,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdentityReference {
    profile: String,
    binding_id: String,
    binding_digest: String,
    familiar_root_id: String,
    identity_revision_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptReference {
    receipt_id: String,
    session_id: String,
    manifest_digest: String,
    binding_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Source {
    source_ref: String,
    source_revision: String,
    content_digest: String,
    resource_ref: String,
    category: Category,
    truncation: Truncation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Truncation {
    original_bytes: u32,
    included_bytes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Category {
    Identity,
    Policy,
    DailyMemory,
    DurableMemory,
    Vault,
    History,
    SelectedText,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ContextMode {
    Continuity,
    Fresh,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Retention {
    Retained,
    Temporary,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OptionalMemoryPolicy {
    Disabled,
    SelectedOnly,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Debug)]
pub(crate) enum Rejection {
    Invalid(&'static str),
    Mismatch(&'static str),
    Forbidden,
    WrongRoute,
    ImplicitReferences,
    Unavailable,
    ReceiptUnavailable,
}

impl Rejection {
    fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "chat_context_invalid",
            Self::Mismatch(_) => "chat_context_mismatch",
            Self::Forbidden => "chat_context_forbidden",
            Self::WrongRoute => "chat_context_wrong_route",
            Self::ImplicitReferences => "chat_context_implicit_refs_unsupported",
            Self::Unavailable => "chat_context_admission_unavailable",
            Self::ReceiptUnavailable => "chat_context_receipt_unavailable",
        }
    }

    fn response(&self) -> anyhow::Result<ApiResponse> {
        let (status, details) = match self {
            Self::Invalid(field) => (400, json!({"fields": [field]})),
            Self::Mismatch(field) => (409, json!({"fields": [field]})),
            Self::Forbidden => (403, json!({"requiredTransport": "owner-local-ipc"})),
            Self::WrongRoute | Self::ImplicitReferences => (400, json!({})),
            Self::Unavailable | Self::ReceiptUnavailable => (
                503,
                json!({
                    "missing": [
                        "trusted-familiar-embodiment-verifier",
                        "current-authoritative-ledger-observation",
                        "qualified-adapter-context-profile"
                    ],
                    "accepted": false,
                    "receiptIssued": false
                }),
            ),
        };
        api_error(status, self.code(), &self.to_string(), Some(details))
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Invalid(_) => "Invalid ordinary-chat context intent.",
            Self::Mismatch(_) => "Context intent does not match this request.",
            Self::Forbidden => "Context admission requires owner-local IPC.",
            Self::WrongRoute => "Context admission is not supported on this operation.",
            Self::ImplicitReferences => "Context-bound CLI reference expansion is not supported.",
            Self::Unavailable => "Trusted Familiar authority and a qualified context profile are unavailable; nothing was admitted.",
            Self::ReceiptUnavailable => "No accepted ordinary-chat context receipt can be resolved; retry was not admitted.",
        };
        write!(formatter, "{}: {message}", self.code())
    }
}

impl std::error::Error for Rejection {}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn exact_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

impl Request {
    fn parse(raw: &str) -> Result<Self, Rejection> {
        if raw.len() > MAX_REQUEST_BYTES {
            return Err(Rejection::Invalid("contextAdmission"));
        }
        let request: Self =
            serde_json::from_str(raw).map_err(|_| Rejection::Invalid("contextAdmission"))?;
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn read(path: &Path) -> anyhow::Result<Self> {
        if !path.metadata()?.is_file() {
            return Err(Rejection::Invalid("contextAdmission").into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let raw =
            std::str::from_utf8(&bytes).map_err(|_| Rejection::Invalid("contextAdmission"))?;
        Ok(Self::parse(raw)?)
    }

    fn validate(&self) -> Result<(), Rejection> {
        let m = &self.manifest;
        let identity = &m.identity;
        if self.profile != PROFILE
            || identity.profile != "familiar.embodiment_binding.v1"
            || [
                &m.familiar_id,
                &m.harness,
                &m.adapter_profile,
                &m.project_root,
                &identity.binding_id,
                &identity.familiar_root_id,
                &identity.identity_revision_id,
            ]
            .into_iter()
            .any(|s| !exact_text(s))
            || m.model.as_ref().is_some_and(|s| !exact_text(s))
            || !Path::new(&m.project_root).is_absolute()
        {
            return Err(Rejection::Invalid("manifest"));
        }
        if [
            &self.manifest_digest,
            &identity.binding_digest,
            &m.prompt_digest,
            &m.launch_policy_digest,
        ]
        .into_iter()
        .any(|s| !valid_digest(s))
        {
            return Err(Rejection::Invalid("manifestDigest"));
        }
        let resources: BTreeSet<_> = m.resource_refs.iter().collect();
        let selections: BTreeSet<_> = m.selections.iter().copied().collect();
        if resources.is_empty()
            || resources.len() > 64
            || resources.len() != m.resource_refs.len()
            || resources.iter().any(|s| !exact_text(s))
            || selections.len() != m.selections.len()
            || !selections.contains(&Category::Identity)
            || !selections.contains(&Category::Policy)
            || m.sources.is_empty()
            || m.sources.len() > 128
        {
            return Err(Rejection::Invalid("manifest.selections"));
        }
        let mut seen_sources = BTreeSet::new();
        let mut sourced_categories = BTreeSet::new();
        for source in &m.sources {
            if !exact_text(&source.source_ref)
                || !exact_text(&source.source_revision)
                || !seen_sources.insert(&source.source_ref)
                || !valid_digest(&source.content_digest)
                || !resources.contains(&source.resource_ref)
                || !selections.contains(&source.category)
                || source.truncation.included_bytes == 0
                || source.truncation.included_bytes > source.truncation.original_bytes
            {
                return Err(Rejection::Invalid("manifest.sources"));
            }
            sourced_categories.insert(source.category);
        }
        if sourced_categories != selections
            || (m.optional_memory_policy == OptionalMemoryPolicy::Disabled
                && [Category::DailyMemory, Category::DurableMemory]
                    .iter()
                    .any(|category| selections.contains(category)))
            || (m.mode == ContextMode::Fresh
                && selections.iter().any(|category| {
                    !matches!(
                        category,
                        Category::Identity | Category::Policy | Category::SelectedText
                    )
                }))
        {
            return Err(Rejection::Invalid("manifest.selections"));
        }
        if sha256_digest(m).map_err(|_| Rejection::Invalid("manifest"))? != self.manifest_digest {
            return Err(Rejection::Mismatch("manifestDigest"));
        }
        if let Some(receipt) = &self.expected_receipt {
            if !exact_text(&receipt.receipt_id)
                || !exact_text(&receipt.session_id)
                || !valid_digest(&receipt.manifest_digest)
                || !valid_digest(&receipt.binding_digest)
            {
                return Err(Rejection::Invalid("expectedReceipt"));
            }
            if receipt.manifest_digest != self.manifest_digest
                || receipt.binding_digest != identity.binding_digest
            {
                return Err(Rejection::Mismatch("expectedReceipt"));
            }
        }
        Ok(())
    }

    fn check_prompt(&self, prompt: &str) -> Result<(), Rejection> {
        if digest(prompt.as_bytes()) != self.manifest.prompt_digest {
            return Err(Rejection::Mismatch("manifest.promptDigest"));
        }
        Ok(())
    }

    pub(crate) fn check_policy(&self, policy: &Value) -> Result<(), Rejection> {
        if sha256_digest(policy).map_err(|_| Rejection::Invalid("launchPolicy"))?
            != self.manifest.launch_policy_digest
        {
            return Err(Rejection::Mismatch("manifest.launchPolicyDigest"));
        }
        Ok(())
    }

    pub(crate) fn check_launch(
        &self,
        harness: &str,
        model: Option<&str>,
        familiar_id: Option<&str>,
        project_root: &Path,
        resumed: bool,
    ) -> Result<(), Rejection> {
        let m = &self.manifest;
        if m.harness != harness
            || m.model.as_deref() != model
            || Some(m.familiar_id.as_str()) != familiar_id
            || Path::new(&m.project_root) != project_root
        {
            return Err(Rejection::Mismatch("manifest"));
        }
        if resumed && (m.mode == ContextMode::Fresh || self.expected_receipt.is_none()) {
            return Err(Rejection::Mismatch("expectedReceipt"));
        }
        Ok(())
    }

    pub(crate) fn check_session_target(&self, session_id: &str) -> Result<(), Rejection> {
        if self.manifest.mode == ContextMode::Fresh {
            return Err(Rejection::Mismatch("manifest.mode"));
        }
        let receipt = self
            .expected_receipt
            .as_ref()
            .ok_or(Rejection::Invalid("expectedReceipt"))?;
        if receipt.session_id != session_id {
            return Err(Rejection::Mismatch("expectedReceipt.sessionId"));
        }
        Ok(())
    }

    fn unavailable(&self) -> Rejection {
        if self.expected_receipt.is_some() {
            Rejection::ReceiptUnavailable
        } else {
            Rejection::Unavailable
        }
    }

    pub(crate) fn reject_cli_dispatch(
        &self,
        prompt: &str,
        expand: impl FnOnce() -> anyhow::Result<String>,
    ) -> anyhow::Result<Infallible> {
        // A digest of the original prompt does not cover post-expansion file,
        // thread or search context. Until source authorization exists, do not
        // read those sources at all, even if the client listed matching refs.
        if !crate::prompt_refs::parse(prompt).refs.is_empty() {
            return Err(Rejection::ImplicitReferences.into());
        }
        let expanded = expand()?;
        self.check_prompt(&expanded)?;
        Err(self.unavailable().into())
    }
}

struct UniqueObject(BTreeMap<String, Box<RawValue>>);

impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = UniqueObject;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate request key"));
                    }
                }
                Ok(UniqueObject(fields))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

/// Only requests that actually contain the new field enter this boundary.
/// Legacy malformed-body precedence remains in the existing route handlers.
pub(crate) fn reject_api_intent(
    method: &str,
    route: &str,
    body: Option<&str>,
    authority: RequestAuthority,
) -> anyhow::Result<Option<ApiResponse>> {
    if !matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
        return Ok(None);
    }
    let Some(raw) = body else { return Ok(None) };
    let payload: Value = match serde_json::from_str(raw) {
        Ok(payload) => payload,
        Err(_) => return Ok(None), // Preserve the original handler's malformed-body behavior.
    };
    if payload.get("contextAdmission").is_none() {
        return Ok(None);
    }
    let reject = || -> Result<Infallible, Rejection> {
        if !authority.allows_chat_context_admission() {
            return Err(Rejection::Forbidden);
        }
        let object: UniqueObject =
            serde_json::from_str(raw).map_err(|_| Rejection::Invalid("contextAdmission"))?;
        let request = Request::parse(object.0["contextAdmission"].get())?;
        if method != "POST"
            || payload.get("executionBinding").is_some()
            || payload.get("requestAdoption").is_some()
        {
            return Err(Rejection::WrongRoute);
        }
        if route == "/sessions" {
            let text = |key| {
                payload
                    .get(key)
                    .and_then(Value::as_str)
                    .ok_or(Rejection::Invalid("launch"))
            };
            let prompt = text("prompt")?.trim();
            request.check_prompt(prompt)?;
            request.check_policy(payload.get("launchPolicy").unwrap_or(&Value::Null))?;
            request.check_launch(
                text("harness")?,
                payload.get("model").and_then(Value::as_str),
                payload.get("familiarId").and_then(Value::as_str),
                Path::new(text("projectRoot")?),
                payload
                    .get("conversation")
                    .is_some_and(|c| c.get("mode") == Some(&json!("resume"))),
            )?;
        } else if let Some(session_id) = route
            .strip_prefix("/sessions/")
            .and_then(|r| r.strip_suffix("/input"))
            .filter(|id| !id.is_empty())
        {
            let data = payload
                .get("data")
                .and_then(Value::as_str)
                .ok_or(Rejection::Invalid("data"))?;
            request.check_prompt(data)?;
            request.check_session_target(session_id)?;
        } else {
            return Err(Rejection::WrongRoute);
        }
        Err(request.unavailable())
    };
    match reject() {
        Err(rejection) => Ok(Some(rejection.response()?)),
        Ok(impossible) => match impossible {},
    }
}

#[cfg(test)]
pub(crate) fn test_intent(project_root: &Path, prompt: &str, policy: Value) -> Value {
    // These are deliberately unverified references, never a mock authority.
    let manifest = json!({
        "identity": {
            "profile": "familiar.embodiment_binding.v1",
            "bindingId": "binding:unverified",
            "bindingDigest": "a".repeat(64),
            "familiarRootId": "familiar:fixture-root",
            "identityRevisionId": "revision:fixture-current"
        },
        "familiarId": "fixture",
        "harness": "codex",
        "model": null,
        "adapterProfile": "codex:unqualified-context-v1",
        "mode": "continuity",
        "projectRoot": project_root,
        "resourceRefs": ["resource:fixture"],
        "selections": ["identity", "policy"],
        "sources": (["identity", "policy"].map(|category| json!({
            "sourceRef": format!("source:{category}"),
            "sourceRevision": "revision:fixture-source",
            "contentDigest": "b".repeat(64),
            "resourceRef": "resource:fixture",
            "category": category,
            "truncation": {"originalBytes": 10, "includedBytes": 10}
        }))),
        "promptDigest": digest(prompt.as_bytes()),
        "launchPolicyDigest": sha256_digest(&policy).unwrap(),
        "retention": "retained",
        "optionalMemoryPolicy": "disabled"
    });
    json!({
        "profile": PROFILE,
        "manifestDigest": sha256_digest(&manifest).unwrap(),
        "manifest": manifest,
        "expectedReceipt": null
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> Value {
        test_intent(&std::env::temp_dir(), "hello", Value::Null)
    }

    fn rehash(value: &mut Value) {
        value["manifestDigest"] = json!(sha256_digest(&value["manifest"]).unwrap());
    }

    #[test]
    fn chat_context_admission_closed_shape_and_presence_are_required() {
        let raw = intent().to_string();
        let unknown = raw.replacen("\"manifest\":", "\"extra\":true,\"manifest\":", 1);
        let duplicate = raw.replacen("\"manifest\":", "\"profile\":\"shadow\",\"manifest\":", 1);
        let duplicate_nested = raw.replacen("\"model\":null", "\"model\":null,\"model\":null", 1);
        for raw in ["null", "[]", "{}", &unknown, &duplicate, &duplicate_nested] {
            assert!(matches!(Request::parse(raw), Err(Rejection::Invalid(_))));
        }
        for field in ["expectedReceipt", "profile"] {
            let mut value = intent();
            value.as_object_mut().unwrap().remove(field);
            assert!(matches!(
                Request::parse(&value.to_string()),
                Err(Rejection::Invalid(_))
            ));
        }
        for field in [
            "model",
            "retention",
            "optionalMemoryPolicy",
            "launchPolicyDigest",
        ] {
            let mut value = intent();
            value["manifest"].as_object_mut().unwrap().remove(field);
            assert!(matches!(
                Request::parse(&value.to_string()),
                Err(Rejection::Invalid(_))
            ));
        }
    }

    #[test]
    fn chat_context_admission_binds_every_manifest_field() {
        for (pointer, replacement) in [
            ("/manifest/identity/bindingId", json!("binding:other")),
            (
                "/manifest/identity/familiarRootId",
                json!("familiar:same-display-different-root"),
            ),
            (
                "/manifest/identity/identityRevisionId",
                json!("revision:stale"),
            ),
            ("/manifest/identity/bindingDigest", json!("c".repeat(64))),
            ("/manifest/retention", json!("temporary")),
            ("/manifest/familiarId", json!("other-roster-slot")),
            ("/manifest/harness", json!("claude")),
            ("/manifest/model", json!("different-model")),
            ("/manifest/adapterProfile", json!("codex:other-profile")),
            ("/manifest/mode", json!("fresh")),
            (
                "/manifest/projectRoot",
                json!(std::env::temp_dir().join("other")),
            ),
            ("/manifest/promptDigest", json!("d".repeat(64))),
            ("/manifest/launchPolicyDigest", json!("e".repeat(64))),
            (
                "/manifest/sources/0/sourceRef",
                json!("source:other-identity"),
            ),
            (
                "/manifest/sources/0/sourceRevision",
                json!("revision:changed"),
            ),
            ("/manifest/sources/0/contentDigest", json!("c".repeat(64))),
            ("/manifest/sources/0/truncation/originalBytes", json!(11)),
            ("/manifest/sources/0/truncation/includedBytes", json!(9)),
            ("/manifest/optionalMemoryPolicy", json!("selected-only")),
        ] {
            let mut value = intent();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                matches!(
                    Request::parse(&value.to_string()),
                    Err(Rejection::Mismatch("manifestDigest"))
                ),
                "{pointer}"
            );
        }
    }

    #[test]
    fn chat_context_admission_caller_evidence_cannot_establish_authority() {
        for (pointer, key, evidence) in [
            (
                "/manifest/identity",
                "authentication",
                json!({"signerId": "signer:caller"}),
            ),
            (
                "/manifest/identity",
                "principal",
                json!({"authenticatedPrincipalId": "principal:other"}),
            ),
            (
                "/manifest/identity",
                "resolutionSnapshot",
                json!({"status": "active"}),
            ),
            (
                "/manifest/identity",
                "trustedLedger",
                json!({"generation": 1, "status": "active"}),
            ),
            (
                "/manifest",
                "executionBinding",
                json!({"bindingId": "execution:other-contract"}),
            ),
            ("/manifest", "accepted", json!(true)),
        ] {
            let mut value = intent();
            value.pointer_mut(pointer).unwrap()[key] = evidence;
            rehash(&mut value);
            assert!(
                matches!(
                    Request::parse(&value.to_string()),
                    Err(Rejection::Invalid(_))
                ),
                "{pointer}/{key}"
            );
        }
        // Reference digests do not become trusted when a caller recomputes them.
        for revision in ["revision:stale", "revision:revoked", "revision:other-root"] {
            let mut value = intent();
            value["manifest"]["identity"]["identityRevisionId"] = json!(revision);
            rehash(&mut value);
            let request = Request::parse(&value.to_string()).unwrap();
            assert!(matches!(request.unavailable(), Rejection::Unavailable));
        }
    }

    #[test]
    fn chat_context_admission_manifest_bounds_are_enforced() {
        for edit in 0..12 {
            let mut value = intent();
            match edit {
                0 => value["manifest"]["projectRoot"] = json!("relative/project"),
                1 => value["manifest"]["identity"]["bindingId"] = json!(" binding:padded"),
                2 => value["manifest"]["identity"]["familiarRootId"] = json!("familiar:\nroot"),
                3 => value["manifest"]["model"] = json!("m".repeat(2049)),
                4 => value["manifest"]["sources"][0]["contentDigest"] = json!("A".repeat(64)),
                5 => value["manifest"]["sources"][0]["truncation"]["includedBytes"] = json!(0),
                6 => value["manifest"]["sources"][0]["truncation"]["originalBytes"] = json!(-1),
                7 => {
                    value["manifest"]["sources"][0]["truncation"]["originalBytes"] =
                        json!(4_294_967_296_u64)
                }
                8 => value["manifest"]["sources"][0]["truncation"]["includedBytes"] = json!(1.5),
                9 => {
                    value["manifest"]["resourceRefs"] =
                        json!(["resource:fixture", "resource:fixture"])
                }
                10 => value["manifest"]["selections"] = json!(["identity", "policy", "identity"]),
                11 => value["manifest"]["mode"] = json!("isolated"),
                _ => unreachable!(),
            }
            rehash(&mut value);
            assert!(
                matches!(
                    Request::parse(&value.to_string()),
                    Err(Rejection::Invalid(_))
                ),
                "{edit}"
            );
        }
        let mut value = intent();
        value["manifest"]["resourceRefs"] = json!(std::iter::once("resource:fixture".to_string())
            .chain((0..64).map(|i| format!("resource:{i}")))
            .collect::<Vec<_>>());
        rehash(&mut value);
        assert!(matches!(
            Request::parse(&value.to_string()),
            Err(Rejection::Invalid(_))
        ));
        let mut value = intent();
        let source = value["manifest"]["sources"][0].clone();
        value["manifest"]["sources"] = json!((0..129)
            .map(|i| {
                let mut source = source.clone();
                source["sourceRef"] = json!(format!("source:{i}"));
                source["category"] = json!(if i == 0 { "policy" } else { "identity" });
                source
            })
            .collect::<Vec<_>>());
        rehash(&mut value);
        assert!(matches!(
            Request::parse(&value.to_string()),
            Err(Rejection::Invalid(_))
        ));
    }

    #[test]
    fn chat_context_admission_rejects_omitted_or_undeclared_context() {
        for edit in 0..6 {
            let mut value = intent();
            match edit {
                0 => {
                    value["manifest"]["selections"] = json!(["identity"]);
                }
                1 => {
                    value["manifest"]["sources"][0]["resourceRef"] =
                        json!("resource:other-project");
                }
                2 => {
                    value["manifest"]["sources"][0]["truncation"]["includedBytes"] = json!(11);
                }
                3 => {
                    value["manifest"]["sources"][0]["category"] = json!("vault");
                }
                4 => {
                    value["manifest"]["selections"] = json!(["identity", "policy", "daily-memory"]);
                }
                5 => {
                    value["manifest"]["sources"][1]["sourceRef"] = json!("source:identity");
                }
                _ => unreachable!(),
            }
            rehash(&mut value);
            assert!(
                matches!(
                    Request::parse(&value.to_string()),
                    Err(Rejection::Invalid(_))
                ),
                "{edit}"
            );
        }
    }

    #[test]
    fn chat_context_admission_optional_memory_requires_explicit_matching_selection() {
        let mut value = intent();
        value["manifest"]["selections"] = json!(["identity", "policy", "daily-memory"]);
        let mut memory = value["manifest"]["sources"][0].clone();
        memory["sourceRef"] = json!("source:daily");
        memory["category"] = json!("daily-memory");
        value["manifest"]["sources"]
            .as_array_mut()
            .unwrap()
            .push(memory);
        rehash(&mut value);
        assert!(Request::parse(&value.to_string()).is_err());
        value["manifest"]["optionalMemoryPolicy"] = json!("selected-only");
        rehash(&mut value);
        let request = Request::parse(&value.to_string()).unwrap();
        assert!(matches!(request.unavailable(), Rejection::Unavailable));
        value["manifest"]["mode"] = json!("fresh");
        rehash(&mut value);
        assert!(Request::parse(&value.to_string()).is_err());
    }

    #[test]
    fn chat_context_admission_cli_checks_post_expansion_bytes() {
        let request = Request::parse(&intent().to_string()).unwrap();
        let error = request
            .reject_cli_dispatch("hello", || Ok("implicit prefix\nhello".into()))
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<Rejection>(),
            Some(Rejection::Mismatch("manifest.promptDigest"))
        ));
    }

    #[test]
    fn chat_context_admission_cli_does_not_read_implicit_refs() {
        let request = Request::parse(&intent().to_string()).unwrap();
        for prompt in ["@secret.md", "@T-parent", "@@parent history"] {
            let error = request
                .reject_cli_dispatch(prompt, || panic!("must not read context"))
                .unwrap_err();
            assert!(matches!(
                error.downcast_ref::<Rejection>(),
                Some(Rejection::ImplicitReferences)
            ));
        }
    }

    #[test]
    fn chat_context_admission_known_prompt_and_self_consistent_receipt_never_authorize() {
        let mut value = intent();
        let request = Request::parse(&value.to_string()).unwrap();
        let error = request
            .reject_cli_dispatch("hello", || Ok("hello".into()))
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<Rejection>(),
            Some(Rejection::Unavailable)
        ));
        value["expectedReceipt"] = json!({
            "receiptId": "receipt:not-issued",
            "sessionId": "session-fixture",
            "manifestDigest": value["manifestDigest"],
            "bindingDigest": value["manifest"]["identity"]["bindingDigest"]
        });
        let request = Request::parse(&value.to_string()).unwrap();
        assert!(matches!(
            request.unavailable(),
            Rejection::ReceiptUnavailable
        ));
        value["manifest"]["retention"] = json!("temporary");
        rehash(&mut value);
        assert!(matches!(
            Request::parse(&value.to_string()),
            Err(Rejection::Mismatch("expectedReceipt"))
        ));
    }

    #[test]
    fn chat_context_admission_model_policy_and_resume_changes_are_fenced() {
        let request = Request::parse(&intent().to_string()).unwrap();
        assert!(request
            .check_launch("codex", None, Some("fixture"), &std::env::temp_dir(), false)
            .is_ok());
        for (model, familiar, root, resumed) in [
            (
                Some("other-model"),
                Some("fixture"),
                std::env::temp_dir(),
                false,
            ),
            (None, Some("other-familiar"), std::env::temp_dir(), false),
            (
                None,
                Some("fixture"),
                std::env::temp_dir().join("other"),
                false,
            ),
            (None, Some("fixture"), std::env::temp_dir(), true),
        ] {
            assert!(request
                .check_launch("codex", model, familiar, &root, resumed)
                .is_err());
        }
        assert!(request
            .check_policy(&json!({"sandbox": "read-only"}))
            .is_err());
    }

    #[test]
    fn chat_context_admission_file_is_bounded_and_never_becomes_a_trust_source(
    ) -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(Request::read(temp.path()).is_err());
        let path = temp.path().join("intent.json");
        std::fs::write(&path, intent().to_string())?;
        assert!(matches!(
            Request::read(&path)?.unavailable(),
            Rejection::Unavailable
        ));
        std::fs::write(&path, " ".repeat(MAX_REQUEST_BYTES + 1))?;
        assert!(Request::read(&path).is_err());
        std::fs::write(&path, [0xff])?;
        assert!(Request::read(&path).is_err());
        Ok(())
    }
}
