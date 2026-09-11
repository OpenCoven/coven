//! Pure response and error-envelope mapping for the Coven daemon HTTP API.
//!
//! Route handlers decide status, error code, message, and details before
//! entering this module. These constructors only serialize that decision into
//! the stable [`ApiResponse`] transport shape; they perform no validation,
//! policy, persistence, or I/O.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl ApiResponse {
    pub(crate) fn json_body(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json",
            body,
        }
    }
}

pub(crate) fn api_error(
    status: u16,
    code: &str,
    message: &str,
    details: Option<Value>,
) -> Result<ApiResponse> {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(details) = details {
        error["details"] = details;
    }
    json_response(status, &json!({ "error": error }))
}

pub(crate) fn json_response<T: Serialize>(status: u16, body: &T) -> Result<ApiResponse> {
    Ok(ApiResponse {
        status,
        content_type: "application/json",
        body: serde_json::to_string(body).context("failed to serialize API response")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_response_preserves_status_content_type_and_compact_body() -> Result<()> {
        let response = json_response(202, &vec!["first", "second"])?;

        assert_eq!(
            response,
            ApiResponse {
                status: 202,
                content_type: "application/json",
                body: r#"["first","second"]"#.to_string(),
            }
        );
        Ok(())
    }

    #[test]
    fn json_body_preserves_a_pre_serialized_bounded_body() {
        let body = r#"{"events":[],"nextCursor":null}"#.to_string();
        let response = ApiResponse::json_body(200, body.clone());

        assert_eq!(
            response,
            ApiResponse {
                status: 200,
                content_type: "application/json",
                body,
            }
        );
    }

    #[test]
    fn api_error_omits_details_when_absent() -> Result<()> {
        let response = api_error(400, "invalid_request", "Invalid request.", None)?;

        assert_eq!(response.status, 400);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(
            serde_json::from_str::<Value>(&response.body)?,
            json!({
                "error": {
                    "code": "invalid_request",
                    "message": "Invalid request.",
                }
            })
        );
        assert!(!response.body.contains("details"));
        Ok(())
    }

    #[test]
    fn api_error_preserves_structured_details() -> Result<()> {
        let details = json!({
            "apiVersion": "v2",
            "supportedApiVersions": ["v1"],
        });
        let response = api_error(
            404,
            "invalid_request",
            "Unsupported API version.",
            Some(details.clone()),
        )?;

        assert_eq!(
            serde_json::from_str::<Value>(&response.body)?,
            json!({
                "error": {
                    "code": "invalid_request",
                    "message": "Unsupported API version.",
                    "details": details,
                }
            })
        );
        Ok(())
    }

    #[test]
    fn json_response_reports_serialization_context() {
        struct SerializationFailure;

        impl Serialize for SerializationFailure {
            fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("fixture serialization failure"))
            }
        }

        let error = json_response(200, &SerializationFailure).unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("failed to serialize API response: fixture serialization failure"),
            "unexpected serialization error: {error:#}"
        );
    }
}
