//! Refusal-only session-policy admission. No store, familiar resolution, or
//! runtime handle enters this module; discovery and parsed requests grant nothing.

use anyhow::Result;
use serde::{de::MapAccess, Deserialize, Deserializer, Serialize};

use crate::{
    api_response::{api_error, json_response, ApiResponse},
    api_routes::{normalize_api_route, split_path_query, ApiRoute},
    automations::contract::canonical_json::{sha256_hex, MAX_SAFE_INTEGER},
    request_authority::RequestAuthority,
};

pub(crate) const CONTRACT: &str = "coven.session-policy.v1";
const PROFILE: &str = "workspace-readonly-no-network.v1";
const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_ADMISSION_WINDOW_MS: i64 = 300_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    contract: &'static str,
    enforcement: &'static str,
    supported_profiles: [&'static str; 0],
    reason: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestrictedRequest {
    contract: String,
    request_id: String,
    invocation_id: String,
    profile: String,
    expires_at_unix_ms: i64,
    #[serde(deserialize_with = "deserialize_object")]
    launch: RestrictedLaunch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestrictedLaunch {
    project_root: String,
    cwd: String,
    harness: String,
    familiar_id: String,
    launch_mode: String,
    prompt: String,
    title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Refusal<'a> {
    contract: &'static str,
    request_id: &'a str,
    invocation_id: &'a str,
    request_digest: String,
    decision: &'static str,
    code: &'static str,
    admission: &'static str,
}

// Derived struct deserializers also accept positional arrays. Require maps
// while retaining serde's duplicate/missing/unknown-field and type validation.
fn deserialize_object<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ObjectVisitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for ObjectVisitor<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<M: MapAccess<'de>>(self, map: M) -> std::result::Result<T, M::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(ObjectVisitor(std::marker::PhantomData))
}

pub(crate) fn discovery_response() -> Result<ApiResponse> {
    json_response(
        200,
        &Discovery {
            contract: CONTRACT,
            enforcement: "unavailable",
            supported_profiles: [],
            reason: "no_verified_enforcement_backend",
        },
    )
}

pub(crate) fn is_restricted_route(method: &str, path: &str) -> bool {
    let (route, _) = split_path_query(path);
    method == "POST"
        && matches!(normalize_api_route(route), ApiRoute::Route(route) if route == "/sessions/restricted")
}

pub(crate) fn preflight(
    authority: RequestAuthority,
    body_bytes: usize,
) -> Result<Option<ApiResponse>> {
    if !authority.allows_session_launch_policy() {
        return api_error(
            403,
            "forbidden",
            "Restricted sessions require the owner-gated local IPC transport.",
            None,
        )
        .map(Some);
    }
    if body_bytes > MAX_BODY_BYTES {
        return invalid_request("Restricted session request exceeds the 1048576-byte limit.")
            .map(Some);
    }
    Ok(None)
}

pub(crate) fn invalid_request(message: &str) -> Result<ApiResponse> {
    api_error(400, "invalid_request", message, None)
}

pub(crate) fn restricted_response(
    body: Option<&[u8]>,
    authority: RequestAuthority,
    now_unix_ms: i64,
) -> Result<ApiResponse> {
    if let Some(response) = preflight(authority, body.map_or(0, <[u8]>::len))? {
        return Ok(response);
    }
    let Some(body) = body else {
        return invalid_request("Restricted session request body is required.");
    };
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let request: RestrictedRequest = match deserialize_object(&mut deserializer) {
        Ok(request) => request,
        Err(_) => {
            return invalid_request("Restricted session request must match the closed schema.")
        }
    };
    if deserializer.end().is_err() {
        return invalid_request("Restricted session request must contain one JSON object.");
    }
    if request.contract != CONTRACT || request.profile != PROFILE {
        return invalid_request("Unsupported session-policy contract or profile.");
    }
    if !canonical_uuid(&request.request_id) || !canonical_uuid(&request.invocation_id) {
        return invalid_request("Session-policy IDs must be lowercase canonical UUIDs.");
    }
    let launch = &request.launch;
    if !bounded_string(&launch.project_root, 4096, false)
        || !bounded_string(&launch.cwd, 4096, false)
        || !bounded_string(&launch.harness, 128, false)
        || !bounded_string(&launch.familiar_id, 128, false)
        || launch.familiar_id.trim() != launch.familiar_id
        || !bounded_string(&launch.prompt, 1_000_000, false)
        || !bounded_string(&launch.title, 512, true)
        || launch.launch_mode != "nonInteractive"
    {
        return invalid_request("Restricted launch fields violate the session-policy contract.");
    }
    // The bundled registry is pure data. Do not scan configured adapters,
    // executables, paths, or familiar state merely to refuse admission.
    if !crate::harness::built_in_harness_specs()
        .iter()
        .any(|spec| spec.id == launch.harness)
    {
        return invalid_request("Unsupported restricted launch harness.");
    }
    if request.expires_at_unix_ms.unsigned_abs() > MAX_SAFE_INTEGER {
        return invalid_request("Session-policy deadline must be a JavaScript safe integer.");
    }
    if request.expires_at_unix_ms <= now_unix_ms {
        return api_error(
            409,
            "session_policy_expired",
            "Session-policy admission deadline has expired.",
            None,
        );
    }
    if request
        .expires_at_unix_ms
        .checked_sub(now_unix_ms)
        .is_none_or(|remaining| remaining > MAX_ADMISSION_WINDOW_MS)
    {
        return invalid_request("Session-policy deadline exceeds the 300000-ms admission window.");
    }
    json_response(
        409,
        &Refusal {
            contract: CONTRACT,
            request_id: &request.request_id,
            invocation_id: &request.invocation_id,
            request_digest: format!("sha256:{}", sha256_hex(body)),
            decision: "rejected",
            code: "enforcement_unavailable",
            admission: "not_started",
        },
    )
}

fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|uuid| uuid.hyphenated().to_string() == value)
}

fn bounded_string(value: &str, max_bytes: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty()) && value.len() <= max_bytes && !value.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{handle_request_with_runtime_and_authority, SessionLaunch, SessionRuntime};
    use serde_json::{json, Value};
    use std::cell::Cell;

    const NOW: i64 = 1_799_999_700_000;
    const REQUEST: &[u8] =
        include_bytes!("../../../spec/coven-session-policy/v1/fixtures/request.json");
    const DISCOVERY: &str =
        include_str!("../../../spec/coven-session-policy/v1/fixtures/discovery.json");
    const REFUSAL: &str =
        include_str!("../../../spec/coven-session-policy/v1/fixtures/refusal.json");

    #[derive(Default)]
    struct CountingRuntime(Cell<usize>);

    impl SessionRuntime for CountingRuntime {
        fn launch_session(&self, _: &SessionLaunch) -> Result<()> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }

        fn send_input(&self, _: &str, _: &Value) -> Result<()> {
            panic!("policy must not access the runtime")
        }

        fn kill_session(&self, _: &str) -> Result<()> {
            panic!("policy must not access the runtime")
        }

        fn event_writer_health(&self) -> Option<crate::event_writer::EventWriterHealth> {
            panic!("policy discovery must not inspect runtime health")
        }
    }

    fn request() -> Value {
        serde_json::from_slice(REQUEST).unwrap()
    }

    #[test]
    fn shared_fixture_manifest_pins_bytes_digests_and_admission_time() {
        let manifest: Value = serde_json::from_str(include_str!(
            "../../../spec/coven-session-policy/v1/fixtures/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["admissionTimeUnixMs"], NOW);
        for (name, bytes) in [
            ("request", REQUEST),
            ("refusal", REFUSAL.as_bytes()),
            ("discovery", DISCOVERY.as_bytes()),
        ] {
            assert_eq!(manifest[name]["bytes"], bytes.len());
            assert_eq!(
                manifest[name]["digest"],
                format!("sha256:{}", sha256_hex(bytes))
            );
            assert!(bytes.ends_with(b"\n"));
            assert!(!bytes.ends_with(b"\n\n"));
        }
    }

    fn assert_error(response: ApiResponse, status: u16, code: &str) {
        assert_eq!(response.status, status, "{}", response.body);
        let payload: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(payload["error"]["code"], code, "{payload}");
        assert!(payload["error"]["message"].is_string(), "{payload}");
        assert!(payload.get("admission").is_none(), "{payload}");
        assert!(payload.get("requestDigest").is_none(), "{payload}");
    }

    fn assert_invalid(body: &[u8]) {
        assert_error(
            restricted_response(Some(body), RequestAuthority::OwnerLocalIpc, NOW).unwrap(),
            400,
            "invalid_request",
        );
        // The same malformed envelope must fail before any store or runtime
        // access in the real router, not just in the pure validation seam.
        if let Ok(body) = std::str::from_utf8(body) {
            assert_error(
                route_without_effects(
                    "POST",
                    "/api/v1/sessions/restricted",
                    Some(body),
                    RequestAuthority::OwnerLocalIpc,
                ),
                400,
                "invalid_request",
            );
        }
    }

    fn route_without_effects(
        method: &str,
        path: &str,
        body: Option<&str>,
        authority: RequestAuthority,
    ) -> ApiResponse {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("must-not-be-created");
        let runtime = CountingRuntime::default();
        let response = handle_request_with_runtime_and_authority(
            method, path, &home, None, body, &runtime, authority,
        )
        .unwrap();
        assert_eq!(runtime.0.get(), 0, "denied route launched a runtime");
        assert!(!home.exists(), "denied route created its store/home");
        response
    }

    #[test]
    fn valid_request_is_correlated_refusal_not_a_grant() {
        let response =
            restricted_response(Some(REQUEST), RequestAuthority::OwnerLocalIpc, NOW).unwrap();
        assert_eq!(response.status, 409);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, REFUSAL.trim_end_matches('\n'));
        let payload: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            payload,
            json!({
                "contract": "coven.session-policy.v1",
                "requestId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "invocationId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                "requestDigest": format!("sha256:{}", crate::automations::contract::canonical_json::sha256_hex(REQUEST)),
                "decision": "rejected",
                "code": "enforcement_unavailable",
                "admission": "not_started"
            })
        );
    }

    #[test]
    fn request_digest_binds_exact_raw_bytes_not_reserialized_json() {
        let compact = serde_json::to_vec(&request()).unwrap();
        let pretty = serde_json::to_vec_pretty(&request()).unwrap();
        let unicode = std::str::from_utf8(REQUEST)
            .unwrap()
            .replace("Wand review", "Wand caf\u{e9}");
        let escaped = unicode.replace('\u{e9}', "\\u00e9");
        let mut digests = std::collections::HashSet::new();
        for bytes in [
            REQUEST,
            compact.as_slice(),
            pretty.as_slice(),
            unicode.as_bytes(),
            escaped.as_bytes(),
        ] {
            let response =
                restricted_response(Some(bytes), RequestAuthority::OwnerLocalIpc, NOW).unwrap();
            assert_eq!(response.status, 409);
            let payload: Value = serde_json::from_str(&response.body).unwrap();
            let digest = payload["requestDigest"].as_str().unwrap();
            assert_eq!(
                digest,
                format!(
                    "sha256:{}",
                    crate::automations::contract::canonical_json::sha256_hex(bytes)
                )
            );
            assert!(
                digests.insert(digest.to_owned()),
                "digest ignored changed wire bytes"
            );
        }
    }

    #[test]
    fn discovery_is_inert_on_owner_and_tcp() {
        for authority in [RequestAuthority::OwnerLocalIpc, RequestAuthority::Tcp] {
            for path in ["/api/v1/session-policy", "/session-policy"] {
                let response = route_without_effects("GET", path, None, authority);
                assert_eq!(response.status, 200);
                assert_eq!(response.body, DISCOVERY.trim_end_matches('\n'));
            }
        }
    }

    #[test]
    fn restricted_route_refuses_all_bundled_harnesses_without_resolving_paths_or_familiar() {
        for harness in ["codex", "claude", "coven-code", "copilot"] {
            let mut value = request();
            value["launch"]["harness"] = json!(harness);
            value["expiresAtUnixMs"] = json!(chrono::Utc::now().timestamp_millis() + 300_000);
            let body = value.to_string();
            let response = route_without_effects(
                "POST",
                "/api/v1/sessions/restricted",
                Some(&body),
                RequestAuthority::OwnerLocalIpc,
            );
            assert_eq!(response.status, 409, "{}", response.body);
            assert_eq!(
                serde_json::from_str::<Value>(&response.body).unwrap()["admission"],
                "not_started"
            );
        }
    }

    #[test]
    fn transport_authority_precedes_even_malformed_json() {
        for body in [None, Some(REQUEST), Some(&b"\xff"[..]), Some(&b"{"[..])] {
            assert_error(
                restricted_response(body, RequestAuthority::Tcp, NOW).unwrap(),
                403,
                "forbidden",
            );
        }
        for body in [None, Some("{"), std::str::from_utf8(REQUEST).ok()] {
            assert_error(
                route_without_effects(
                    "POST",
                    "/api/v1/sessions/restricted",
                    body,
                    RequestAuthority::Tcp,
                ),
                403,
                "forbidden",
            );
        }
    }

    #[test]
    fn malformed_json_and_nonobjects_are_rejected() {
        for body in [
            b"".as_slice(),
            b"{",
            b"null",
            b"[]",
            b"true",
            b"42",
            b"\"text\"",
            b"{}{}",
            b"{/*comment*/}",
            b"\xef\xbb\xbf{}",
        ] {
            assert_invalid(body);
        }
        assert_error(
            restricted_response(None, RequestAuthority::OwnerLocalIpc, NOW).unwrap(),
            400,
            "invalid_request",
        );
        assert_error(
            route_without_effects(
                "POST",
                "/api/v1/sessions/restricted",
                None,
                RequestAuthority::OwnerLocalIpc,
            ),
            400,
            "invalid_request",
        );
        let value = request();
        let sequence = json!([
            value["contract"],
            value["requestId"],
            value["invocationId"],
            value["profile"],
            value["expiresAtUnixMs"],
            value["launch"]
        ]);
        assert_invalid(sequence.to_string().as_bytes());
        let mut value = request();
        let launch = value["launch"].clone();
        value["launch"] = json!([
            launch["projectRoot"],
            launch["cwd"],
            launch["harness"],
            launch["familiarId"],
            launch["launchMode"],
            launch["prompt"],
            launch["title"]
        ]);
        assert_invalid(value.to_string().as_bytes());
    }

    #[test]
    fn every_field_is_required_nonnull_typed_and_unique() {
        for nested in [false, true] {
            let original = request();
            let object = if nested {
                &original["launch"]
            } else {
                &original
            };
            for (field, valid) in object.as_object().unwrap() {
                for replacement in [
                    None,
                    Some(Value::Null),
                    Some(json!(true)),
                    Some(json!([])),
                    Some(json!({})),
                ] {
                    let mut value = request();
                    let object = if nested {
                        &mut value["launch"]
                    } else {
                        &mut value
                    };
                    if let Some(replacement) = replacement {
                        object[field] = replacement;
                    } else {
                        object.as_object_mut().unwrap().remove(field);
                    }
                    assert_invalid(&serde_json::to_vec(&value).unwrap());
                }
                let escaped_duplicate = std::str::from_utf8(REQUEST)
                    .unwrap()
                    .replace("\"title\":", "\"\\u0074itle\":\"duplicate\",\"title\":");
                assert_invalid(escaped_duplicate.as_bytes());
                let body = std::str::from_utf8(REQUEST).unwrap();
                let needle = format!("\"{field}\":");
                let duplicate = body.replacen(&needle, &format!("{needle}{valid},{needle}"), 1);
                assert_invalid(duplicate.as_bytes());
            }
        }
        for nested in [false, true] {
            let mut value = request();
            let object = if nested {
                &mut value["launch"]
            } else {
                &mut value
            };
            object["unknown"] = json!("rejected");
            assert_invalid(&serde_json::to_vec(&value).unwrap());
        }
    }

    #[test]
    fn contract_profile_mode_harness_and_canonical_uuid_are_exact() {
        for (field, values) in [
            (
                "contract",
                vec!["", "coven.session-policy.v2", " coven.session-policy.v1"],
            ),
            (
                "profile",
                vec!["", "workspace-write.v1", "workspace-readonly-no-network.v2"],
            ),
            (
                "requestId",
                vec![
                    "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
                    "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa",
                    "{aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa}",
                    "urn:uuid:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                ],
            ),
            (
                "invocationId",
                vec!["BBBBBBBB-BBBB-4BBB-8BBB-BBBBBBBBBBBB", "", "not-a-uuid"],
            ),
        ] {
            for text in values {
                let mut value = request();
                value[field] = json!(text);
                assert_invalid(&serde_json::to_vec(&value).unwrap());
            }
        }
        for (field, values) in [
            (
                "harness",
                vec!["Codex", "claude-code", "github-copilot", "hermes", " codex"],
            ),
            (
                "launchMode",
                vec!["interactive", "non-interactive", "stream", "NonInteractive"],
            ),
            ("familiarId", vec![" sage", "sage ", "\tsage", "sage\n"]),
        ] {
            for text in values {
                let mut value = request();
                value["launch"][field] = json!(text);
                assert_invalid(&serde_json::to_vec(&value).unwrap());
            }
        }
    }

    #[test]
    fn invalid_unicode_and_nul_are_rejected() {
        for replacement in ["\\ud800", "\\udc00", "\\ud800text"] {
            let body = std::str::from_utf8(REQUEST)
                .unwrap()
                .replace("Wand review", replacement);
            assert_invalid(body.as_bytes());
        }
        let mut invalid_utf8 = REQUEST.to_vec();
        invalid_utf8[0] = 0xff;
        assert_invalid(&invalid_utf8);
        for field in [
            "projectRoot",
            "cwd",
            "harness",
            "familiarId",
            "launchMode",
            "prompt",
            "title",
        ] {
            let mut value = request();
            value["launch"][field] = json!("nul\0value");
            assert_invalid(&serde_json::to_vec(&value).unwrap());
        }
    }

    #[test]
    fn deadline_boundaries_use_injected_admission_time() {
        for (deadline, status) in [
            (-9_007_199_254_740_991, 409),
            (-1, 409),
            (0, 409),
            (NOW - 1, 409),
            (NOW, 409),
            (NOW + 1, 409),
            (NOW + 300_000, 409),
            (NOW + 300_001, 400),
        ] {
            let mut value = request();
            value["expiresAtUnixMs"] = json!(deadline);
            let response = restricted_response(
                Some(value.to_string().as_bytes()),
                RequestAuthority::OwnerLocalIpc,
                NOW,
            )
            .unwrap();
            if deadline <= NOW {
                assert_error(response, status, "session_policy_expired");
            } else if deadline > NOW + 300_000 {
                assert_error(response, status, "invalid_request");
            } else {
                assert_eq!(response.status, status);
                assert_eq!(
                    serde_json::from_str::<Value>(&response.body).unwrap()["admission"],
                    "not_started"
                );
            }
        }
        for deadline in [
            json!(-9_007_199_254_740_992_i64),
            json!(9_007_199_254_740_992_u64),
            json!(u64::MAX),
            json!(1800000000000.0),
            json!("1800000000000"),
        ] {
            let mut value = request();
            value["expiresAtUnixMs"] = deadline;
            assert_invalid(&serde_json::to_vec(&value).unwrap());
        }
        let response = route_without_effects(
            "POST",
            "/api/v1/sessions/restricted",
            Some(
                &std::str::from_utf8(REQUEST)
                    .unwrap()
                    .replace("1800000000000", "1"),
            ),
            RequestAuthority::OwnerLocalIpc,
        );
        assert_error(response, 409, "session_policy_expired");
    }

    #[test]
    fn string_limits_count_utf8_bytes_and_title_alone_may_be_empty() {
        for (field, limit) in [
            ("projectRoot", 4096),
            ("cwd", 4096),
            ("familiarId", 128),
            ("prompt", 1_000_000),
            ("title", 512),
        ] {
            for (text, valid) in [
                ("x".repeat(limit), true),
                ("\u{e9}".repeat(limit / 2), true),
                ("x".repeat(limit + 1), false),
                ("\u{e9}".repeat(limit / 2 + 1), false),
                (String::new(), field == "title"),
            ] {
                let mut value = request();
                value["launch"][field] = json!(text);
                let body = serde_json::to_vec(&value).unwrap();
                if valid {
                    let response =
                        restricted_response(Some(&body), RequestAuthority::OwnerLocalIpc, NOW)
                            .unwrap();
                    assert_eq!(response.status, 409, "{field}: {}", response.body);
                    assert!(response.body.contains("\"not_started\""));
                } else {
                    assert_invalid(&body);
                }
            }
        }
    }

    #[test]
    fn body_limit_and_closed_schema_bound_depth() {
        let mut padded = REQUEST.to_vec();
        padded.resize(1_048_576, b' ');
        let response =
            restricted_response(Some(&padded), RequestAuthority::OwnerLocalIpc, NOW).unwrap();
        assert_eq!(response.status, 409);
        assert!(response.body.contains("\"not_started\""));
        padded.push(b' ');
        assert_invalid(&padded);
        let body = format!("{{\"launch\":{}0{}}}", "[".repeat(17), "]".repeat(17));
        assert_invalid(body.as_bytes());
    }

    #[test]
    fn route_classification_preserves_the_api_version_gate() {
        for path in [
            "/api/v1/sessions/restricted",
            "/sessions/restricted",
            "/api/v1/sessions/restricted?source=wand",
        ] {
            assert!(is_restricted_route("POST", path));
            assert!(!is_restricted_route("GET", path));
        }
        for path in [
            "/api/v2/sessions/restricted",
            "/api/V1/sessions/restricted",
            "/api/v1/sessions/restricted/extra",
            "/api/v1/sessions",
            "/api/v1/",
        ] {
            assert!(!is_restricted_route("POST", path));
        }
        for path in ["/api/v2/session-policy", "/api/v2/sessions/restricted"] {
            let method = if path.ends_with("restricted") {
                "POST"
            } else {
                "GET"
            };
            assert_error(
                route_without_effects(method, path, None, RequestAuthority::OwnerLocalIpc),
                404,
                "invalid_request",
            );
        }
    }

    #[test]
    fn legacy_session_policy_downgrade_is_rejected_before_launch() {
        for authority in [RequestAuthority::OwnerLocalIpc, RequestAuthority::Tcp] {
            for policy in [
                json!(null),
                json!({}),
                json!({"contract": "coven.session-policy.v1"}),
                json!(false),
            ] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let runtime = CountingRuntime::default();
                let body =
                    json!({"projectRoot": temp.path(), "harness": "codex", "prompt": "hello",
                    "sessionPolicy": policy})
                    .to_string();
                for path in ["/sessions", "/api/v1/sessions"] {
                    let response = handle_request_with_runtime_and_authority(
                        "POST",
                        path,
                        &home,
                        None,
                        Some(&body),
                        &runtime,
                        authority,
                    )
                    .unwrap();
                    assert_eq!(runtime.0.get(), 0, "legacy downgrade launched");
                    assert!(!home.exists(), "legacy downgrade created store");
                    assert_error(response, 400, "invalid_request");
                }
            }
        }
    }
}
