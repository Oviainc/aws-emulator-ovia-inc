// SPDX-License-Identifier: CC0-1.0

//! How this service names, in a response HEADER, the exception a refused call is refused
//! with.
//!
//! Every failure this service answers states the exception shape twice: in the
//! `awsJson1_1` body, as the `__type` member the framework's error renderers write, and in
//! the `x-amzn-errortype` header beside it:
//!
//! ```text
//! HTTP/1.1 400 Bad Request
//! content-type: application/x-amz-json-1.1
//! x-amzn-errortype: ResourceNotFoundException:
//!
//! {"__type":"ResourceNotFoundException","message":"IdentityPool '…' not found."}
//! ```
//!
//! The header belongs to the response, not to any modelled shape — no error shape of this
//! model declares it, so no handler can set it — and the framework's `awsJson` renderers
//! write the body alone (`vendor/smithy-server-runtime/src/pre_dispatch.rs` emits the
//! header for `restJson1` only). The recorded responses of this service carry it on every
//! failure, which is what a client reads when it classifies a failure without parsing the
//! body: an SDK resolving the error shape, a proxy or a log pipeline routing on headers.
//!
//! This module completes that envelope at the two layers every response of this service
//! passes through, beside the shared request-id reporter of
//! `service_common::query_error_envelope` so one response names both the request it
//! answers and the exception it answers with:
//!
//! * [`with_error_type_header`], which the per-operation wrapper
//!   `Service::override_operation` hands the router applies, covers everything an operation
//!   answers — a success (untouched), a modelled error, and a constraint refusal raised for
//!   that operation's request;
//! * [`ErrorTypeHeaderDefinition`], the wrapper `Service::definition` hands back, covers a
//!   response NO operation produced: a routing failure, a transport failure detected up
//!   front, and a pre-dispatch authentication refusal.
//!
//! The value follows the response, never a recording: it is the exception shape the
//! response ALREADY names in its body, so a shape this module has never heard of names
//! itself correctly. Nothing else about the response changes — the status, the body and
//! every other header stay as the renderer wrote them, and a success keeps every byte.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use serde_json::Value;
use smithy_server_runtime::{
    ErasedServiceDefinition, OperationRoutingTable, ServerHttpProtocol, ServiceIdentity,
};

/// The header a response names its exception shape in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The body member the framework's JSON error renderers write the exception shape into.
const TYPE_MEMBER: &str = "__type";

/// The lowest HTTP status that is a failure, and so the lowest one that names an exception
/// at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// What separates the shape name from the locator the header's value ends with.
///
/// The value AWS puts in this header is `<shape><sep><locator>`, and the locator is the
/// service stack's own URI for the shape — not something an emulator can state, and not
/// something a client reads: every SDK resolves the shape from the text BEFORE this
/// separator (and before a `#`, for a namespaced name). This service's recorded responses
/// carry the separator with an empty locator after it, so that is the form written here:
/// the shape, then the separator, then nothing.
const LOCATOR_SEPARATOR: char = ':';

/// The bare shape name a discriminator value names.
///
/// A value may arrive namespaced (`com.amazonaws.cognitoidentity#ResourceNotFoundException`)
/// or with a locator already appended; neither is part of the shape's name.
fn shape_name(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace
        .split(LOCATOR_SEPARATOR)
        .next()
        .unwrap_or(after_namespace)
}

/// The exception shape an error body names, or `None` when the body names none: it is not
/// a JSON object, it carries no `__type`, or what it carries is empty.
fn body_shape_name(body: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let raw = document.as_object()?.get(TYPE_MEMBER)?.as_str()?;
    let shape = shape_name(raw);
    (!shape.is_empty()).then(|| shape.to_string())
}

/// The header value for the exception shape `shape`: the shape, then the locator separator
/// (see [`LOCATOR_SEPARATOR`]).
fn error_type_value(shape: &str) -> String {
    format!("{shape}{LOCATOR_SEPARATOR}")
}

/// The response as this service answers it: a failure naming in `x-amzn-errortype` the
/// exception shape its body already names.
///
/// A response that is not a failure is returned untouched, as is a failure whose body names
/// no exception shape. A response that already names one keeps the header it has, so this
/// is idempotent and the two layers below cannot report two shapes for one call. A header
/// the HTTP layer will not carry is dropped rather than turned into a transport failure,
/// which on an error path would replace a modelled 4xx with a 500 an SDK retries.
pub fn with_error_type_header(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS
        || resp.headers().get(ERROR_TYPE_HEADER).is_some()
    {
        return resp;
    }
    let shape = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match body_shape_name(bytes) {
            Some(shape) => shape,
            None => return resp,
        }
    };
    let _ = resp
        .headers_mut()
        .try_insert(ERROR_TYPE_HEADER, error_type_value(&shape));
    resp
}

/// A service's definition with its framework-level error renderers wrapped, so a response
/// no operation produced names its exception shape exactly as one an operation produced
/// does.
///
/// Identity, the routing table and the protocol come from the wrapped definition untouched:
/// nothing model-derived is restated here.
pub struct ErrorTypeHeaderDefinition<D>(D);

impl<D: ErasedServiceDefinition> ErrorTypeHeaderDefinition<D> {
    /// Wrap `inner`, adding the exception's name to every framework-level failure it
    /// renders.
    pub fn new(inner: D) -> Self {
        Self(inner)
    }

    /// The wrapped definition, for a caller that needs a decision this wrapper does not
    /// change — whether the routing table can serve a request, say.
    pub fn inner(&self) -> &D {
        &self.0
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for ErrorTypeHeaderDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.0.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.0.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.0.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_error_type_header(self.0.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_error_type_header(self.0.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_error_type_header(self.0.refusal_response(error, chosen, req))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.0.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    //! The header over the forms a response can name its exception in. The wire behaviour —
    //! that every refusal of every operation carries it — is asserted through the router in
    //! `tests/all/wire_error_envelope.rs`.

    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    /// One response, built from a status and a body.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body.to_string()),
        )
    }

    /// Every exception shape a body can name gets its own name in the header, so the value
    /// is not keyed to one recorded refusal.
    #[test]
    fn the_header_names_whatever_shape_the_body_names() {
        for shape in [
            "ResourceNotFoundException",
            "InvalidParameterException",
            "NotAuthorizedException",
            "ResourceConflictException",
            "TooManyRequestsException",
            "InternalErrorException",
            "ValidationException",
        ] {
            let body = format!(r#"{{"__type":"{shape}","message":"refused"}}"#);
            let answered = with_error_type_header(response(400, &body));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(format!("{shape}:").as_str()),
                "case {shape:?}: expected the header to name the shape, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answered.body().bytes().expect("bytes"),
                body.as_bytes(),
                "case {shape:?}: the body must survive byte for byte"
            );
        }
    }

    /// The shape is recognised whichever form the body names it in, and a value that
    /// already carries a locator is not doubled.
    #[test]
    fn the_shape_is_read_out_of_every_form_a_body_states_it_in() {
        for (case, body) in [
            ("bare", r#"{"__type":"ResourceNotFoundException"}"#),
            (
                "namespaced",
                r#"{"__type":"com.amazonaws.cognitoidentity#ResourceNotFoundException"}"#,
            ),
            (
                "with a locator",
                r#"{"__type":"ResourceNotFoundException:http://internal.amazon.com/coral/"}"#,
            ),
        ] {
            let answered = with_error_type_header(response(400, body));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some("ResourceNotFoundException:"),
                "case {case:?}: expected the bare shape and one separator, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// Nothing to name: a success, a failure whose body names no shape, a body that is not
    /// a JSON object, and a response that already names its shape are all left alone.
    #[test]
    fn a_response_that_names_no_exception_is_left_as_it_was() {
        for (case, status, body) in [
            ("a success", 200, r#"{"IdentityPoolId":"kept"}"#),
            ("no __type", 400, r#"{"message":"refused"}"#),
            ("not an object", 400, r#"["ResourceNotFoundException"]"#),
            ("not JSON", 400, "<Error/>"),
            ("an empty __type", 400, r#"{"__type":""}"#),
        ] {
            let answered = with_error_type_header(response(status, body));
            assert!(
                answered.headers().get(ERROR_TYPE_HEADER).is_none(),
                "case {case:?}: expected no header, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answered.body().bytes().expect("bytes"),
                body.as_bytes(),
                "case {case:?}: the body must survive byte for byte"
            );
        }

        let mut already = response(400, r#"{"__type":"ResourceNotFoundException"}"#);
        already
            .headers_mut()
            .insert(ERROR_TYPE_HEADER, "StatedByTheRenderer:");
        let answered = with_error_type_header(already);
        assert_eq!(
            answered.headers().get(ERROR_TYPE_HEADER),
            Some("StatedByTheRenderer:"),
            "case \"already named\": expected the stated header to stand, got {:?}",
            answered.headers().get(ERROR_TYPE_HEADER)
        );
    }

    /// The status is this module's business in no way: it answers with whatever it was
    /// handed.
    #[test]
    fn the_status_is_left_exactly_as_the_renderer_wrote_it() {
        for status in [400u16, 403, 409, 429, 500, 501] {
            let answered = with_error_type_header(response(
                status,
                r#"{"__type":"NotAuthorizedException","message":"no"}"#,
            ));
            assert_eq!(
                answered.status().as_u16(),
                status,
                "case {status}: the status must not move, got {}",
                answered.status().as_u16()
            );
        }
    }
}
