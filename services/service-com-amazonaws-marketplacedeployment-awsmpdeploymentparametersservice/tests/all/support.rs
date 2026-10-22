// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the SDK's percent-encoding for a
//! path segment or a query value, and the three accessors over an answered body —
//! the error code, the message, and the `fieldName` the modelled
//! `ValidationException` carries beside it.

use serde_json::Value;

/// Percent-encode a value the way an SDK does for one `@httpLabel` path segment
/// or one `@httpQuery` value: everything outside RFC 3986's unreserved set. A
/// non-greedy label may not carry a raw `/`, and a query value may not carry a
/// raw `+` (which decodes to a space) or `&`.
pub fn pct(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The error code a failed call reported, with any namespace stripped
/// (`com.amazonaws.marketplacedeployment#Foo` -> `Foo`).
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` of an answered body, whichever casing the envelope spells it with.
pub fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The `fieldName` of an error body — the modelled `ValidationException` carries it
/// beside the message, naming the member the refusal is about.
pub fn field_name(body: &Value) -> String {
    body.get("fieldName")
        .and_then(Value::as_str)
        .unwrap_or("<no fieldName>")
        .to_string()
}
