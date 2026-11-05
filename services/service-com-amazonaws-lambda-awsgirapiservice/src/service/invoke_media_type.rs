// SPDX-License-Identifier: CC0-1.0

//! The media type an INVOCATION's response announces.
//!
//! # The rule
//!
//! `Invoke`'s `Payload` is "The response from the function, or an error object" and is the
//! operation's `@httpPayload`, so the response body IS that document — and what the
//! runtime serialized it to is JSON. The response therefore announces `application/json`,
//! which is what a client reads to parse it.
//!
//! The framework's renderer cannot say that: the member is a `Blob`, and a blob
//! `@httpPayload` is opaque bytes to the protocol, so it is announced as
//! `application/octet-stream`. That is the media type of the BINDING, not of the document
//! the function produced.
//!
//! # Why it is stated here
//!
//! The operation's output declares no member bound to `content-type` — there is nothing a
//! handler could set — so the value is the renderer's, and the one seam that sees a whole
//! rendered response is the per-operation wrapper every modelled operation of this service
//! is served behind ([`crate::service`]). Only the ONE value the renderer stamps for this
//! binding is replaced, and only for the operations whose body is a function's own response
//! ([`ANNOUNCING_OPERATIONS`]): every other header, the status and the body are left exactly
//! as the renderer wrote them, and a response that carries any other media type is
//! untouched.

use aws_smithy_runtime_api::http::Response as RtResponse;

/// The operations whose response body is the document a FUNCTION produced.
///
/// `InvokeAsync` is not among them: its answer carries no payload at all.
/// `InvokeWithResponseStream` is not either — its body is an event stream, and its own
/// `ResponseStreamContentType` member says what the stream carries.
const ANNOUNCING_OPERATIONS: &[&str] = &["Invoke"];

/// The media type the framework's renderer stamps for a blob `@httpPayload`.
const IMPLIED: &str = "application/octet-stream";

/// The media type an invocation's response announces.
const ANNOUNCED: &str = "application/json";

/// `resp` as this service announces it, for the operation `operation`.
#[must_use]
pub(super) fn announced(operation: &str, resp: RtResponse) -> RtResponse {
    if !ANNOUNCING_OPERATIONS.contains(&operation) {
        return resp;
    }
    ::service_common::response_media_type::ResponseMediaType::new(IMPLIED, ANNOUNCED)
        .announce(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// A response carrying `content_type`, if any.
    fn response(content_type: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("a valid status"),
            SdkBody::from(r#"{"ok":true}"#),
        );
        if let Some(content_type) = content_type {
            resp.headers_mut()
                .insert("content-type", content_type.to_string());
        }
        resp
    }

    /// One row per operation and media type: the invocation's own response is announced as
    /// the JSON it is, and nothing else is touched.
    #[test]
    fn only_an_invocations_own_payload_is_announced_as_json() {
        let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            ("Invoke", "the implied blob media type", Some(IMPLIED), Some(ANNOUNCED)),
            (
                "Invoke",
                "a media type the renderer chose",
                Some("text/plain"),
                Some("text/plain"),
            ),
            ("Invoke", "no media type at all", None, None),
            (
                "InvokeAsync",
                "another operation's blob",
                Some(IMPLIED),
                Some(IMPLIED),
            ),
            (
                "GetFunction",
                "a modelled JSON body",
                Some(ANNOUNCED),
                Some(ANNOUNCED),
            ),
        ];
        for (operation, case, carried, expected) in cases {
            let answered = announced(operation, response(*carried));
            let actual = answered.headers().get("content-type");
            assert_eq!(
                actual, *expected,
                "case {operation} with {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
