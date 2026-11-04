// SPDX-License-Identifier: CC0-1.0

//! Whether an in-process call to ANOTHER emulated service reached that service at all.
//!
//! A service that performs a call for its caller — an origin fetch, an event delivery, an
//! integration naming another service's action — makes it over the in-process transport
//! `OperationContext::callback` hands a handler, which dispatches the request back into the
//! same emulator. That bridge answers with the FRONT DOOR's own refusal when no mounted
//! service claims the request ([`UNKNOWN_SERVICE`]) or when more than one does
//! ([`AMBIGUOUS_SERVICE`]), and neither is an answer from the service that was addressed:
//! the call reached nothing. A caller that maps such a refusal as if it were the addressed
//! service's answer reports a status and a body for work nothing performed — a 200 carrying
//! the refusal envelope, say — so every performer of an in-process call has to tell the two
//! apart, and does it here.
//!
//! The refusal is told by the error type its envelope carries, because the bridge keeps the
//! body but not the headers: the envelope is where that type can be read at all.
//!
//! Services still carrying their own copy of this reading, each to be MIGRATED onto this
//! module in the change that owns its service: `is_transport_refusal` in
//! `service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/service/edge.rs` (an origin
//! the edge could not reach) and in `service-com-amazonaws-eventbridge-awsevents/src/state.rs`
//! (an api-destination delivery nothing served). Nothing new inlines the rule again — call
//! this module.

/// The error type the front door answers with when NO mounted service claims the request.
pub const UNKNOWN_SERVICE: &str = "UnknownService";

/// The error type it answers with when MORE THAN ONE mounted service claims the request.
pub const AMBIGUOUS_SERVICE: &str = "AmbiguousService";

/// How much of an answer is read to tell the transport's own refusal from the addressed
/// service's response. The refusal envelope is two short fields; anything larger is a
/// response from something that served the request, so it is not read at all.
const MAX_REFUSAL_ENVELOPE_BYTES: usize = 4 * 1024;

/// Whether `body` is the in-process transport's OWN refusal of the call rather than a
/// response from the service it was addressed to.
#[must_use]
pub fn is_transport_refusal(body: &[u8]) -> bool {
    if body.len() > MAX_REFUSAL_ENVELOPE_BYTES {
        return false;
    }
    let Ok(envelope) = ::serde_json::from_slice::<::serde_json::Value>(body) else {
        return false;
    };
    let Some(error_type) = envelope.get("__type").and_then(::serde_json::Value::as_str) else {
        return false;
    };
    // The type may carry a shape namespace; the code is its last segment.
    let code = error_type.rsplit('#').next().unwrap_or(error_type);
    code == UNKNOWN_SERVICE || code == AMBIGUOUS_SERVICE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transports_own_refusal_is_told_from_a_services_answer() {
        // Rows differ along what the body IS: each of the two refusal envelopes, one
        // carrying a namespaced type, another service's own JSON error, a response that is
        // not JSON at all, an envelope naming no type, and a body too large to be the
        // envelope.
        let oversized = format!(
            "{{\"__type\":\"UnknownService\",\"message\":\"{}\"}}",
            "p".repeat(MAX_REFUSAL_ENVELOPE_BYTES)
        );
        let cases: [(&str, &str, bool); 7] = [
            (
                "the front door's refusal for a host no service claims",
                r#"{"__type":"UnknownService","message":"no emulated service is registered"}"#,
                true,
            ),
            (
                "the front door's refusal for a request two services claim",
                r#"{"__type":"AmbiguousService","message":"two services claim it"}"#,
                true,
            ),
            (
                "the same type carrying a shape namespace",
                r#"{"__type":"com.amazonaws.s3#UnknownService"}"#,
                true,
            ),
            (
                "an addressed service's own modelled error",
                r#"{"__type":"NoSuchKey","message":"The specified key does not exist."}"#,
                false,
            ),
            (
                "an addressed service's XML error document",
                "<Error><Code>AccessDenied</Code></Error>",
                false,
            ),
            ("an envelope naming no type at all", r#"{"message":"x"}"#, false),
            ("a body too large to be the envelope", oversized.as_str(), false),
        ];
        for (case, body, expected) in &cases {
            assert_eq!(
                is_transport_refusal(body.as_bytes()),
                *expected,
                "case {case:?}: expected is_transport_refusal {expected}, got {}",
                is_transport_refusal(body.as_bytes())
            );
        }
    }
}
