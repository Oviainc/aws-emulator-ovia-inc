// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the SDK's percent-encoding for a
//! path segment or a query value, and the three accessors over an answered body —
//! the error code, the message, and the `fieldName` the modelled
//! `ValidationException` carries beside it.

use serde_json::Value;

/// `epoch_secs` as the RFC 3339 date-time string the wire carries a
/// `@timestampFormat("date-time")` member as.
///
/// `PutDeploymentParameterRequest$expirationDate` is marked
/// `@timestampFormat("date-time")`
/// (`smithy_query(query="operation",operation="PutDeploymentParameter")`), so on
/// restJson1 the member travels as a string in this form — which is what every AWS
/// SDK serializes for it. The epoch second is how a test says WHICH instant it means;
/// the rendering is the model's.
pub fn rfc3339(epoch_secs: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(epoch_secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an epoch second renders as an RFC 3339 date-time")
}

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
