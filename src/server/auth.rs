//! AWS authentication header validation.
//!
//! Dynoxide never verifies signatures, but it mirrors DynamoDB's validation
//! of the auth material itself: header-based and query-string SigV4 are
//! checked for presence and completeness, with the same error types and
//! messages DynamoDB returns when parts are missing or conflicting.

use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::Response,
};

use super::dynamo_response;

/// Validate AWS authentication headers/query parameters.
///
/// DynamoDB checks auth after resolving the target operation. Returns `Some(Response)` if
/// auth validation fails, `None` if auth is present (or if we choose to skip full
/// signature verification).
pub(super) fn validate_auth(headers: &HeaderMap, uri: &Uri, response_ct: &str) -> Option<Response> {
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok());

    // Check query string for X-Amz-Algorithm
    let query = uri.query().unwrap_or("");
    let has_algorithm_query = query.split('&').any(|p| {
        let key = p.split('=').next().unwrap_or("");
        key == "X-Amz-Algorithm"
    });

    // If both Authorization header AND X-Amz-Algorithm query → InvalidSignatureException
    if auth_header.is_some() && has_algorithm_query {
        let body = serde_json::json!({
            "__type": "com.amazon.coral.service#InvalidSignatureException",
            "message": "Found both 'X-Amz-Algorithm' as a query-string param and 'Authorization' as HTTP header."
        })
        .to_string();
        return Some(dynamo_response(StatusCode::BAD_REQUEST, response_ct, body));
    }

    // Query-string auth (X-Amz-Algorithm present)
    if has_algorithm_query {
        let mut missing = Vec::new();
        let query_params: Vec<&str> = query
            .split('&')
            .map(|p| p.split('=').next().unwrap_or(""))
            .collect();

        // Check if X-Amz-Algorithm has a non-empty value
        let algo_has_value = query.split('&').any(|p| {
            let mut parts = p.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let val = parts.next().unwrap_or("");
            key == "X-Amz-Algorithm" && !val.is_empty()
        });

        if !algo_has_value {
            missing.push("'X-Amz-Algorithm'");
        }
        for (param, label) in [
            ("X-Amz-Credential", "'X-Amz-Credential'"),
            ("X-Amz-Signature", "'X-Amz-Signature'"),
            ("X-Amz-SignedHeaders", "'X-Amz-SignedHeaders'"),
            ("X-Amz-Date", "'X-Amz-Date'"),
        ] {
            if !query_params.contains(&param) {
                missing.push(label);
            }
        }

        if !missing.is_empty() {
            let parts: Vec<String> = missing
                .iter()
                .map(|p| format!("AWS query-string parameters must include {p}. "))
                .collect();
            let msg = format!("{}Re-examine the query-string parameters.", parts.join(""));
            let body = serde_json::json!({
                "__type": "com.amazon.coral.service#IncompleteSignatureException",
                "message": msg
            })
            .to_string();
            return Some(dynamo_response(StatusCode::BAD_REQUEST, response_ct, body));
        }

        // Query auth is present and complete — allow through
        return None;
    }

    // Header-based auth
    match auth_header {
        None => {
            // No Authorization header at all → MissingAuthenticationTokenException
            let body = serde_json::json!({
                "__type": "com.amazon.coral.service#MissingAuthenticationTokenException",
                "message": "Request is missing Authentication Token"
            })
            .to_string();
            Some(dynamo_response(StatusCode::BAD_REQUEST, response_ct, body))
        }
        Some(auth) => {
            if !auth.starts_with("AWS4-") {
                // Authorization header doesn't start with AWS4- → MissingAuthenticationTokenException
                let body = serde_json::json!({
                    "__type": "com.amazon.coral.service#MissingAuthenticationTokenException",
                    "message": "Request is missing Authentication Token"
                })
                .to_string();
                return Some(dynamo_response(StatusCode::BAD_REQUEST, response_ct, body));
            }

            // AWS4- prefix present — check for required parameters
            let has_date = headers.get("x-amz-date").is_some() || headers.get("date").is_some();

            // Parse auth header for Credential, Signature, SignedHeaders
            // These can be separated by spaces or commas
            let has_credential = auth.contains("Credential=") || auth.contains("credential=");
            let has_signature = auth.contains("Signature=") || auth.contains("signature=");
            let has_signed_headers =
                auth.contains("SignedHeaders=") || auth.contains("signedheaders=");

            let mut missing = Vec::new();
            if !has_credential {
                missing.push("'Credential'");
            }
            if !has_signature {
                missing.push("'Signature'");
            }
            if !has_signed_headers {
                missing.push("'SignedHeaders'");
            }
            if !has_date {
                missing.push("existence of either a 'X-Amz-Date' or a 'Date' header.");
            }

            if missing.is_empty() {
                // All required parts present — allow through (we don't verify signatures)
                return None;
            }

            // Build the IncompleteSignatureException message
            let mut parts: Vec<String> = missing
                .iter()
                .map(|p| {
                    if p.contains("existence of") {
                        format!("Authorization header requires {p}")
                    } else {
                        format!("Authorization header requires {p} parameter.")
                    }
                })
                .collect();
            parts.push(format!("Authorization={auth}"));
            let msg = parts.join(" ");
            let body = serde_json::json!({
                "__type": "com.amazon.coral.service#IncompleteSignatureException",
                "message": msg
            })
            .to_string();
            Some(dynamo_response(StatusCode::BAD_REQUEST, response_ct, body))
        }
    }
}
