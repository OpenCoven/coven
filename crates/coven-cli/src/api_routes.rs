//! Route/version authority gate for the Coven daemon HTTP API.
//!
//! Every API request enters through [`crate::api::handle_request`] (or its
//! `_with_body` / `_with_runtime` / `_with_runtime_and_authority` variants),
//! and the first thing that entry point does is split the raw path with
//! [`split_path_query`] and classify the route with [`normalize_api_route`]
//! here. This module is the single place that decides whether a request path
//! carries a supported API version, so handlers below the gate never re-parse
//! the `/api/<version>` prefix and cannot be reached under an unsupported
//! version.
//!
//! Contract (pinned by the tests below and by the envelope tests in
//! `crate::api`):
//!
//! - paths without the `/api/` prefix pass through unchanged (borrowed, no
//!   allocation);
//! - `/api/<supported-version>/<route>` is rewritten to `/<route>`;
//! - any other `/api/...` shape is rejected before dispatch: an unsupported
//!   version answers `404 invalid_request` with an `apiVersion` and
//!   `supportedApiVersions` payload, and every other malformed shape answers
//!   `404 not_found`;
//! - the query string is split off the raw path before classification and is
//!   never part of the classified route.
//!
//! New routes belong in the `crate::api` dispatch behind this gate — never in
//! a helper that skips it. Route/version policy changes belong here;
//! validation, persistence, and response mapping stay in their own seams (see
//! `docs/authority-module-inventory.md`).

use std::borrow::Cow;

/// The only API route version this authority currently accepts.
pub const COVEN_API_ROUTE_VERSION: &str = "v1";

/// Route versions advertised to clients that send an unsupported version.
pub const SUPPORTED_API_ROUTE_VERSIONS: [&str; 1] = [COVEN_API_ROUTE_VERSION];

/// Classification of a raw request path after its query string was split off.
#[derive(Debug)]
pub(crate) enum ApiRoute<'a> {
    /// A routable path: either a non-`/api/` path passed through unchanged, or
    /// an `/api/<supported-version>/<route>` path with the version prefix
    /// stripped. Never retains a version prefix or a `?query` suffix.
    Route(Cow<'a, str>),
    /// `/api/<version>/...` with a version this authority does not support.
    Unsupported(String),
    /// An `/api/...` path that does not carry a routable suffix.
    Malformed,
}

/// Classify a raw request path (already split from its query string).
pub(crate) fn normalize_api_route(route: &str) -> ApiRoute<'_> {
    let Some(rest) = route.strip_prefix("/api/") else {
        return ApiRoute::Route(Cow::Borrowed(route));
    };
    let Some((version, suffix)) = rest.split_once('/') else {
        return ApiRoute::Malformed;
    };
    if version != COVEN_API_ROUTE_VERSION {
        return ApiRoute::Unsupported(version.to_string());
    }
    if suffix.is_empty() {
        return ApiRoute::Malformed;
    }
    ApiRoute::Route(Cow::Owned(format!("/{suffix}")))
}

/// Split the raw request path into its route part and optional query string.
pub(crate) fn split_path_query(path: &str) -> (&str, Option<&str>) {
    match path.split_once('?') {
        Some((route, query)) => (route, Some(query)),
        None => (path, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_supported_version_paths_to_stripped_route() {
        match normalize_api_route("/api/v1/health") {
            ApiRoute::Route(route) => assert_eq!(route, "/health"),
            other => panic!("expected a route, got {other:?}"),
        }
        match normalize_api_route("/api/v1/sessions/session-1/input") {
            ApiRoute::Route(route) => assert_eq!(route, "/sessions/session-1/input"),
            other => panic!("expected a route, got {other:?}"),
        }
    }

    #[test]
    fn passes_non_api_paths_through_without_allocation() {
        for path in ["/health", "", "/", "/api", "api/v1/health"] {
            match normalize_api_route(path) {
                ApiRoute::Route(Cow::Borrowed(passed)) => assert_eq!(passed, path),
                other => panic!("expected a borrowed passthrough for {path:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_unsupported_api_versions() {
        for (path, version) in [
            ("/api/v2/health", "v2"),
            ("/api/V1/health", "V1"),
            ("/api//health", ""),
        ] {
            match normalize_api_route(path) {
                ApiRoute::Unsupported(unsupported) => assert_eq!(unsupported, version),
                other => panic!("expected an unsupported version for {path:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_malformed_api_prefixes() {
        for path in ["/api/", "/api/v1", "/api/v1/"] {
            assert!(
                matches!(normalize_api_route(path), ApiRoute::Malformed),
                "expected a malformed classification for {path:?}"
            );
        }
    }

    #[test]
    fn stripped_routes_never_retain_version_prefix_or_query() {
        for path in [
            "/api/v1/health",
            "/api/v1/health?refresh=1",
            "/api/v1/sessions?limit=2&cursor=abc",
            "/api/v1/events?sessionId=s-1",
        ] {
            let (route, _query) = split_path_query(path);
            match normalize_api_route(route) {
                ApiRoute::Route(stripped) => {
                    assert!(!stripped.starts_with("/api/"), "{stripped:?}");
                    assert!(!stripped.contains('?'), "{stripped:?}");
                }
                other => panic!("expected a route for {path:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn splits_path_and_query() {
        assert_eq!(split_path_query("/health"), ("/health", None));
        assert_eq!(
            split_path_query("/events?sessionId=s-1"),
            ("/events", Some("sessionId=s-1"))
        );
        assert_eq!(
            split_path_query("/sessions?limit=2&cursor=a=b"),
            ("/sessions", Some("limit=2&cursor=a=b"))
        );
        assert_eq!(split_path_query("/events?"), ("/events", Some("")));
    }
}
