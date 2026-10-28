// SPDX-License-Identifier: CC0-1.0

//! The media type this service answers every response under.
//!
//! # What the wire shows
//!
//! Every recorded response of this service — a success, a modelled exception, a refusal —
//! carries `content-type: application/x-amz-json-1.1`, while the emulator stamped
//! `application/x-amz-json-1.0`. The two are not interchangeable: a client (or a recording
//! proxy) that selects its JSON-protocol decoder from the response media type reads a
//! different protocol version than the one the service is speaking.
//!
//! The REQUEST side is untouched and matches throughout: the model declares
//! `aws.protocols#awsJson1_0`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json")`),
//! requests still route by `X-Amz-Target: AWSBCMDashboardsService.<Operation>`, and the
//! awsJson `1.1` codec this service answers in is the SAME `JsonCodec` over the same empty
//! body, so a request sent under either media type still reads byte for byte. Only the
//! media type the RESPONSE announces differs from what the protocol trait implies — the
//! real service fronts its awsJson1_0 API with a dispatcher that stamps the 1.1 media type
//! on what it sends back — and the wire record is what a client sees.
//!
//! # Where the rule lives
//!
//! Not here. The rule, the one function that applies it
//! ([`ResponseMediaType::announce`]) and the operation-seam wrapper that carries it to the
//! framework's refusal tail
//! ([`ResponseMediaTypeOperation`](service_common::response_media_type::ResponseMediaTypeOperation))
//! are [`service_common::response_media_type`], the tree's one statement of them. This
//! module holds only what is this SERVICE's: the pair its responses are re-announced
//! across, named once so every seam in [`crate::service`] reads the same two values. Which
//! seams those are, and the one response no author seam reaches — the authentication
//! refusal, which the router renders before this service is asked anything — are in that
//! file's header and pinned by `tests/all/response_media_type.rs`.

use service_common::response_media_type::{ResponseMediaType, AWS_JSON_1_0, AWS_JSON_1_1};

/// The pair this service's responses are re-announced across: the media type its model's
/// `awsJson1_0` protocol trait implies — stamped verbatim by the framework's own renderers,
/// and the ONLY value replaced — and the `application/x-amz-json-1.1` its recorded wire
/// announces.
pub const RESPONSE_MEDIA_TYPE: ResponseMediaType =
    ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1);

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::Response as RtResponse;
    use aws_smithy_types::body::SdkBody;

    /// The response header carrying the media type.
    const CONTENT_TYPE: &str = "content-type";

    /// A response carrying `content_type`, or none at all when it is `None`.
    fn response(content_type: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200).expect("200 is a status"),
            SdkBody::from(&b"{}"[..]),
        );
        if let Some(content_type) = content_type {
            resp.headers_mut()
                .insert(CONTENT_TYPE, content_type.to_string());
        }
        resp
    }

    /// The media type of a response, as a client would read it.
    fn media_type(resp: &RtResponse) -> Option<String> {
        resp.headers().get(CONTENT_TYPE).map(str::to_string)
    }

    /// The protocol-implied media type becomes the one this service answers under; every
    /// other value is left alone.
    ///
    /// The cases span what a producer can put in the header: the protocol-implied media
    /// type (what the framework's renderers stamp), the value this service answers under
    /// (already right, so unchanged), a deliberately different media type (an event-stream
    /// frame), and no header at all.
    #[test]
    fn only_the_protocol_implied_media_type_is_reannounced() {
        let cases: [(Option<&str>, Option<&str>); 4] = [
            (
                Some("application/x-amz-json-1.0"),
                Some("application/x-amz-json-1.1"),
            ),
            (
                Some("application/x-amz-json-1.1"),
                Some("application/x-amz-json-1.1"),
            ),
            (
                Some("application/vnd.amazon.eventstream"),
                Some("application/vnd.amazon.eventstream"),
            ),
            (None, None),
        ];

        for (given, expected) in cases {
            let actual = media_type(&RESPONSE_MEDIA_TYPE.announce(response(given)));
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {given:?}: expected content-type {expected:?}, got {actual:?}"
            );
        }
    }
}
