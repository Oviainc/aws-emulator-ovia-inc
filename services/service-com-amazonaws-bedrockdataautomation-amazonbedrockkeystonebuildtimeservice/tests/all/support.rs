// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the SDK's percent-encoding for a
//! `@httpLabel` path segment (every operation of this service but the two creates binds
//! an ARN there), and the three accessors over an answered body — a string member, a
//! modelled `date-time` member, and the error code of a refusal.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;

/// Percent-encode a `@httpLabel` value the way an AWS SDK does: everything outside
/// RFC 3986's unreserved set, so an ARN's `:` and `/` stay inside one path segment.
pub fn encode_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One string member of an answered body, or a panic naming the case and the member.
pub fn string_at(value: &Value, member: &str, case: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: member {member:?} missing from {value:?}"))
        .to_string()
}

/// Parse a modeled `date-time` member into comparable (seconds, nanos).
pub fn timestamp_at(value: &Value, member: &str, case: &str) -> (i64, u32) {
    let raw = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: timestamp {member:?} missing from {value:?}"));
    let parsed = match raw {
        Value::String(text) => DateTime::from_str(text, Format::DateTime).unwrap_or_else(|e| {
            panic!("case {case:?}: {member:?} = {text:?} is not a date-time: {e}")
        }),
        Value::Number(epoch) => DateTime::from_secs_f64(
            epoch
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: {member:?} = {epoch:?} is not numeric")),
        ),
        other => panic!("case {case:?}: {member:?} = {other:?} is not a timestamp"),
    };
    (parsed.secs(), parsed.subsec_nanos())
}

/// The error code carried in a restJson1 error body, with any namespace stripped
/// (`com.amazonaws.bedrockdataautomation#Foo` -> `Foo`).
pub fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}
