// SPDX-License-Identifier: CC0-1.0

//! The media type a service ANNOUNCES on its responses, where that is not the one its
//! protocol trait implies.
//!
//! # The rule
//!
//! A response's `content-type` is the media type the SERVICE announces, which is a fact of
//! its recorded wire rather than a consequence of the protocol trait its model carries: a
//! service whose model declares `aws.protocols#awsJson1_0` and whose responses are recorded
//! as `application/x-amz-json-1.1` answers `1.1` — on every status, success and error
//! alike — because that is what a client reads. The framework's renderers stamp the media
//! type the protocol trait implies (`AwsJson10()` in
//! `vendor/smithy-schema-server/src/rpc.rs` for a serialized body, the service macro's
//! per-protocol pre-dispatch declaration for a framework-rendered one), so a service whose
//! wire differs from it re-announces that ONE value and leaves every other response header
//! alone.
//!
//! The REQUEST side is untouched: the media type a caller sends is not re-read, routing
//! still follows the model's protocol, and the awsJson `1.0` and `1.1` codecs are the same
//! `JsonCodec` over the same body, so only the announced value differs.
//!
//! # Where a service takes it
//!
//! [`ResponseMediaType::announce`] is the whole mechanism; which seams a service applies it
//! at is the service's own business, because the layers that can produce a response differ
//! per service (`vendor/smithy-server-runtime/src/service.rs` has their contracts):
//!
//! * `Service::override_protocol` can serve the codec that already announces the right
//!   value — for the awsJson family, `aws_smithy_schema_server::rpc::AwsJson11()` — which
//!   covers every success and every error a handler raises;
//! * the framework-rendered responses — the refusal tail inside the centralized walk, the
//!   routing failure, a transport failure — keep the model-derived envelope, which
//!   `override_protocol` deliberately does not reach, and are re-announced with
//!   [`ResponseMediaType::announce`] through `Service::override_operation`,
//!   `Service::override_routing_error` and `Service::override_framework_error`.
//!
//! Nothing else about a response is touched: the status, every other header and every body
//! byte stay with the renderer that owns them, and a response announcing a media type that
//! is NOT the protocol-implied one — an event-stream frame, say — is left exactly as its
//! producer wrote it rather than relabelled.
//!
//! This rule is CALLED by
//! `service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator`, and it lives here
//! rather than in that crate because it is not that service's rule alone: the sibling
//! `service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice` — the same AWS billing
//! front door, the same `1.1` framing over an `awsJson1_0` model — states it inline today
//! (`src/service/response_media_type.rs`), as do
//! `service-com-amazonaws-amp-amazonprometheusservice` (`RESPONSE_CONTENT_TYPE` in its
//! `src/state.rs`, with the rewrite in its `src/service.rs`) and
//! `service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions`
//! (which takes `override_protocol` alone, so the responses its framework renders still
//! announce the protocol-implied value).
//!
//! STILL TO MIGRATE: those three onto this module. Each is a change of its own rather than a
//! side effect of the fix that hoisted the rule — two of them need a path dependency their
//! crate does not carry yet, and none of their issues or recordings is owned here — but
//! nothing new should state the rule a fourth time.

use aws_smithy_runtime_api::http::Response as RtResponse;

/// The response header carrying the media type.
const CONTENT_TYPE: &str = "content-type";

/// The media type the `aws.protocols#awsJson1_0` protocol trait implies.
pub const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";

/// The media type the `aws.protocols#awsJson1_1` protocol trait implies.
pub const AWS_JSON_1_1: &str = "application/x-amz-json-1.1";

/// The pair one service's responses are re-announced across: the media type its protocol
/// trait implies, and the one its wire record shows it answers under.
///
/// Held as a value (and constructible in a `const`) so a service states the pair once and
/// every seam it re-announces at reads the same two strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseMediaType {
    implied: &'static str,
    announced: &'static str,
}

impl ResponseMediaType {
    /// The pair `implied` → `announced`: a response carrying `implied` is re-announced as
    /// `announced`, and nothing else is touched.
    pub const fn new(implied: &'static str, announced: &'static str) -> Self {
        Self { implied, announced }
    }

    /// The media type the service's protocol trait implies — the only value
    /// [`Self::announce`] replaces.
    pub const fn implied(&self) -> &'static str {
        self.implied
    }

    /// The media type the service announces on every response.
    pub const fn announced(&self) -> &'static str {
        self.announced
    }

    /// `resp` as the service announces it: the protocol-implied media type replaced by the
    /// announced one. A response carrying any other media type, or none at all, is returned
    /// untouched — see the module header.
    ///
    /// The comparison is on the WHOLE header value, not on the media type with its
    /// parameters stripped, because the values this replaces are constants: the framework's
    /// renderers stamp the bare protocol media type (`AwsJson10()` and the service macro's
    /// pre-dispatch declaration both carry no parameter). A `content-type` that carries a
    /// parameter was therefore chosen by its producer for that response, and rewriting it
    /// would drop what the producer said — so it is left alone, like any other value.
    pub fn announce(&self, mut resp: RtResponse) -> RtResponse {
        if resp.headers().get(CONTENT_TYPE) == Some(self.implied) {
            resp.headers_mut().insert(CONTENT_TYPE, self.announced);
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

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

    /// The protocol-implied media type becomes the announced one; every other value, and a
    /// response with no media type at all, is left alone.
    ///
    /// The rows span what a producer can put in the header — the implied value (what the
    /// framework's renderers stamp), the announced value (already right), a deliberately
    /// different media type (an event-stream frame, a REST JSON body), and no header — and
    /// the pairs span both directions, so the replacement is the stated one rather than
    /// "whatever looks like awsJson".
    #[test]
    fn only_the_implied_media_type_is_reannounced() {
        let pairs = [
            ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1),
            ResponseMediaType::new(AWS_JSON_1_1, AWS_JSON_1_0),
            ResponseMediaType::new("application/json", "application/json; charset=utf-8"),
        ];
        let others = [
            "application/vnd.amazon.eventstream",
            "text/xml",
            "application/cbor",
        ];

        for pair in pairs {
            let actual = media_type(&pair.announce(response(Some(pair.implied()))));
            assert_eq!(
                actual.as_deref(),
                Some(pair.announced()),
                "case {pair:?} on its implied media type: expected {:?}, got {actual:?}",
                pair.announced()
            );

            let actual = media_type(&pair.announce(response(Some(pair.announced()))));
            assert_eq!(
                actual.as_deref(),
                Some(pair.announced()),
                "case {pair:?} on its announced media type: expected it unchanged, got \
                 {actual:?}"
            );

            for other in others {
                let actual = media_type(&pair.announce(response(Some(other))));
                assert_eq!(
                    actual.as_deref(),
                    Some(other),
                    "case {pair:?} on {other:?}: a media type the producer chose must be \
                     left alone, got {actual:?}"
                );
            }

            let actual = media_type(&pair.announce(response(None)));
            assert_eq!(
                actual, None,
                "case {pair:?} on a response with no media type: expected none, got \
                 {actual:?}"
            );
        }
    }
}
