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
//! # Where it is decided
//!
//! Two places, because one seam cannot reach every response:
//!
//! * `Service::override_protocol` returns the awsJson `1.1` server protocol
//!   ([`aws_smithy_schema_server::rpc::AwsJson11`]), which is the codec the centralized
//!   walk serializes this service's own responses with — every success and every error a
//!   handler raises (a modelled exception, the handler's own `ValidationException`, the
//!   scaffold's 501). That is the one place a serializer's media type is chosen, and no
//!   operation handler is involved.
//! * The responses the FRAMEWORK renders in front of the service keep the model-derived
//!   envelope, which `override_protocol` deliberately does not reach (see its contract in
//!   `vendor/smithy-server-runtime/src/service.rs`): the refusal tail that answers a
//!   constraint the model declares, the routing failure, and a transport failure. Those
//!   are re-announced with [`with_response_media_type`], which replaces the
//!   protocol-implied media type and nothing else. [`ResponseMediaTypeOperation`] is what
//!   reaches the refusal tail: it is the wrapper the service hands back from
//!   `Service::override_operation`, it serves the modelled operation the generated routing
//!   table names for the request — so the framework still owns the whole
//!   deserialize → validate → handle → serialize walk and every status, code, message and
//!   body it answers with — and then re-announces the response. The service's
//!   `override_routing_error` and `override_framework_error` seams re-announce the other
//!   two.
//!
//! Nothing else about a response is touched: the status, the headers and every body byte
//! stay with the framework renderer that owns them, and a response whose media type is NOT
//! the one the protocol trait implies — an event-stream frame, say — is left exactly as its
//! producer wrote it rather than relabelled.
//!
//! One response is still announced by the framework alone, because no author seam reaches
//! it: the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything
//! (`vendor/smithy-server-runtime/src/multi_service_router.rs`). It is pinned by
//! `tests/all/response_media_type.rs` so the gap is recorded rather than hidden.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The response header carrying the media type.
const CONTENT_TYPE: &str = "content-type";

/// The media type the model's `awsJson1_0` protocol trait implies for a response, stamped
/// verbatim by the framework's own renderers — `AwsJson10()` in
/// `vendor/smithy-schema-server/src/rpc.rs` for a serialized body, and the macro's
/// per-protocol pre-dispatch declaration for a framework-rendered one. This exact value is
/// the ONLY one re-announced: anything else on a response was chosen by its producer.
pub const PROTOCOL_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The media type this service answers under, on every response, as the wire record shows.
pub const RESPONSE_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// `resp` as this service announces it: the protocol-implied media type replaced by the one
/// it answers under. Any other `content-type`, and a response carrying none, is returned
/// untouched.
pub fn with_response_media_type(mut resp: RtResponse) -> RtResponse {
    if resp.headers().get(CONTENT_TYPE) == Some(PROTOCOL_MEDIA_TYPE) {
        resp.headers_mut().insert(CONTENT_TYPE, RESPONSE_MEDIA_TYPE);
    }
    resp
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ResponseMediaTypeOperation`].
///
/// A request the table cannot serve is declined rather than claimed, so the router answers
/// it as the unroutable request it is (through `Service::override_routing_error`) instead of
/// authenticating a request that names no operation first.
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then announces the response as this service announces it.
///
/// This wrapper exists for ONE response: the framework-level refusal tail, which renders a
/// constraint the model declares in the model-derived envelope INSIDE the centralized walk
/// (`run_operation_http`), where `Service::override_protocol` explicitly does not reach.
/// Every other response of a routed request is already serialized by the protocol that seam
/// returns, so [`with_response_media_type`] finds nothing to change on it.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`] — a wrapper held on the service cannot borrow from the entry it
/// is held beside, which is why the key is resolved here rather than carried in.
pub struct ResponseMediaTypeOperation;

impl ErasedOperation for ResponseMediaTypeOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            let modeled = definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key));
            let resp = match modeled {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => definition.routing_error_response(req),
            };
            with_response_media_type(resp)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
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
            let actual = media_type(&with_response_media_type(response(given)));
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {given:?}: expected content-type {expected:?}, got {actual:?}"
            );
        }
    }
}
