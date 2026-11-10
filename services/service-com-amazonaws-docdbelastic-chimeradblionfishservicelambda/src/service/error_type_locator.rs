// SPDX-License-Identifier: CC0-1.0

//! How this service NAMES, in the `x-amzn-errortype` response header, the error shape a
//! refused call is refused with.
//!
//! A restJson1 client resolves the error shape from that header, and the framework's
//! renderers write the shape's bare name into it. What this service's wire carries is the
//! QUALIFIED form: the shape name, the separator, and then the service stack's own locator
//! for that shape — the namespace that declares it, under the stack's locator root:
//!
//! ```text
//! HTTP/1.1 404 Not Found
//! x-amzn-errortype: ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/
//!
//! {"__type":"ResourceNotFoundException","message":"Resource was not found", …}
//! ```
//!
//! The locator is not part of the shape's NAME — every SDK resolves the shape from the
//! text before the separator (and after a `#`, for a namespaced name), so nothing a client
//! resolves changes — but it is what this service's wire carries, and a client that
//! validates the whole header value sees it. The body keeps naming the bare shape in
//! `__type`, exactly as the renderer wrote it.
//!
//! The namespace is read off this service's own identity rather than written out here, so
//! the value follows the model: the shape id the generated identity states
//! (`com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda`) carries the namespace that
//! declares this service's shapes.
//!
//! The header belongs to the RESPONSE, not to any modelled shape — no error shape of this
//! model declares it, so no handler can set it — so this module completes it at the two
//! layers every response of this service passes through:
//!
//! * [`QualifyingErrorType`], the wrapper [`crate::service::EmulatedService`] hands the
//!   router from `Service::override_operation`, covers everything an operation answers: a
//!   success (untouched), a modelled error, and a constraint refusal raised while that
//!   operation was being served;
//! * [`ErrorTypeLocatorDefinition`], the definition wrapper `Service::definition` hands
//!   back, covers a response NO operation produced: a routing failure, a transport failure
//!   detected up front, and a pre-dispatch refusal.
//!
//! Nothing else about a response changes: the status, the body and every other header stay
//! as the renderer wrote them, and a success keeps every byte.
//!
//! Those same two layers are where the other response header this service's wire carries is
//! completed — the media type a JSON payload is announced with
//! ([`super::json_media_type`]) — so [`answered`] composes the two and each layer applies
//! that one function.

use super::json_media_type::{announced_media_type, with_json_media_type};
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    Refused, Service, ServerHttpProtocol, ServiceIdentity,
};

/// The header a restJson1 client resolves the error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// What separates the shape name from the locator that follows it.
const SEPARATOR: char = ':';

/// The root every shape locator of the service stack hangs under; the namespace that
/// declares the shape follows it.
const LOCATOR_ROOT: &str = "http://internal.amazon.com/coral/";

/// The lowest HTTP status that is a failure, and so the lowest one that names an error
/// shape at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// The locator this service qualifies a shape name with, for the service whose Smithy
/// shape id is `service_shape_id`: the separator, the locator root, then the namespace that
/// id carries.
///
/// Derived, not written out: a model moved to another namespace qualifies its shapes under
/// that namespace with no edit here.
fn locator(service_shape_id: &str) -> String {
    let namespace = service_shape_id.split('#').next().unwrap_or_default().trim();
    format!("{SEPARATOR}{LOCATOR_ROOT}{namespace}/")
}

/// The bare error SHAPE a header value names.
///
/// A value may arrive namespaced (`com.amazonaws.docdbelastic#ResourceNotFoundException`)
/// or with a locator already appended; neither is part of the shape's name.
fn shape_named(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace
        .split(SEPARATOR)
        .next()
        .unwrap_or(after_namespace)
        .trim()
}

/// `resp` as this service answers it: a failure names its error shape in
/// [`ERROR_TYPE_HEADER`] with this service's locator appended.
///
/// A success is returned untouched, as is a failure whose renderer named no shape (there is
/// nothing to qualify, and this invents no header the renderer did not write). IDEMPOTENT:
/// the shape is read back out of whatever the header carries, so a value that already
/// carries the locator is rewritten to itself and the two layers above cannot report two
/// locators for one call. A header value the HTTP layer will not carry is left as it was
/// rather than turned into a transport failure, which on an error path would replace a
/// modelled 4xx with a 500 an SDK retries.
pub fn with_error_type_locator(mut resp: RtResponse, locator: &str) -> RtResponse {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS {
        return resp;
    }
    let Some(named) = resp.headers().get(ERROR_TYPE_HEADER) else {
        return resp;
    };
    let shape = shape_named(named);
    if shape.is_empty() {
        return resp;
    }
    let qualified = format!("{shape}{locator}");
    let _ = resp.headers_mut().try_insert(ERROR_TYPE_HEADER, qualified);
    resp
}

/// `resp` as this service ANSWERS `req`: the two completions of a response this service
/// owns, applied together at every layer a response passes through.
///
/// Each is its own concern and lives in its own module — the error shape's locator here,
/// the media type a JSON payload is announced with in
/// [`super::json_media_type`] — and this is the one place they are composed, so no layer
/// can apply one and forget the other.
fn answered(resp: RtResponse, req: &RtRequest, locator: &str) -> RtResponse {
    with_json_media_type(
        with_error_type_locator(resp, locator),
        &announced_media_type(req),
    )
}

/// The modelled operation the service's own routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let definition = svc.definition();
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// Serves the modelled operation the service's routing table names for a request exactly as
/// that table would, then completes the response headers this service's wire carries (see
/// [`answered`]).
///
/// It stores no operation of its own and makes no routing decision: the operation it
/// delegates to is the one the service's own table names for the request, so the framework
/// keeps owning the whole deserialize -> validate -> handle -> serialize sequence and every
/// status, shape and message it answers with. One wrapper serves every operation of this
/// service, and it is the one place each of their responses passes through.
pub struct QualifyingErrorType;

impl ErasedOperation for QualifyingErrorType {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read out of the definition BEFORE the await: `&dyn ErasedServiceDefinition`
            // is not `Sync`, so holding one across the await would make this future
            // non-`Send`.
            let locator = locator(svc.definition().identity().shape_id());
            match modeled_operation(svc, req) {
                Some(operation) => {
                    answered(operation.run(svc, req, ctx).await, req, &locator)
                }
                // Unreachable through a `Service::override_operation` that claims a request
                // only when the routing table holds its key; answered as the framework
                // answers an unroutable request rather than guessed at (and the definition
                // wrapper below names the shape on it there).
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    /// `None` is "defer to the modelled operation the routing key names" — which is the
    /// operation this wrapper delegates to, so its model-declared schemes decide and
    /// wrapping cannot weaken them.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// This service's model-derived half with its framework-level error renderers wrapped, so a
/// response NO operation produced carries the same completed headers (see [`answered`]) as
/// one an operation produced.
///
/// Identity, the routing table and the protocol come from the wrapped definition untouched:
/// nothing model-derived is restated here.
pub struct ErrorTypeLocatorDefinition<D>(D);

impl<D: ErasedServiceDefinition> ErrorTypeLocatorDefinition<D> {
    /// Wrap `inner`, completing the headers of every framework-level response it renders.
    pub fn new(inner: D) -> Self {
        Self(inner)
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`QualifyingErrorType`], so a request naming no
    /// modelled operation stays the framework's routing failure.
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.0
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.0.routing_table().get(key))
            .is_some()
    }

    /// The locator this definition's identity names: the one value both layers qualify with.
    fn locator(&self) -> String {
        locator(self.0.identity().shape_id())
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for ErrorTypeLocatorDefinition<D> {
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
        answered(self.0.routing_error_response(req), req, &self.locator())
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        answered(
            self.0.framework_error_response(error, req),
            req,
            &self.locator(),
        )
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        answered(
            self.0.refusal_response(error, chosen, req),
            req,
            &self.locator(),
        )
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.0.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    //! The header over the forms a response can name its error shape in. The wire
    //! behaviour — that every refusal of every operation carries the qualified form — is
    //! asserted through the router in `tests/all/tagging_not_found_sentence.rs`.

    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    /// This service's Smithy shape id, as the generated identity states it
    /// (`smithy_query(query="service_info")`).
    const SHAPE_ID: &str = "com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda";

    /// One response, built from a status and the error type its renderer named.
    fn response(status: u16, error_type: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(r#"{"__type":"ResourceNotFoundException"}"#),
        );
        if let Some(error_type) = error_type {
            resp.headers_mut().insert(ERROR_TYPE_HEADER, error_type.to_string());
        }
        resp
    }

    /// The locator is built from the namespace of the shape id, whatever that namespace is.
    #[test]
    fn the_locator_names_the_namespace_that_declares_the_shapes() {
        for (case, shape_id, expected) in [
            (
                "this service",
                SHAPE_ID,
                ":http://internal.amazon.com/coral/com.amazonaws.docdbelastic/",
            ),
            (
                "another namespace",
                "com.amazon.coral.service#UnknownOperationException",
                ":http://internal.amazon.com/coral/com.amazon.coral.service/",
            ),
        ] {
            let actual = locator(shape_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the locator {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every shape a renderer can name gets its own name qualified, so the value is not
    /// keyed to one recorded refusal.
    #[test]
    fn the_header_qualifies_whatever_shape_the_renderer_named() {
        for shape in [
            "ResourceNotFoundException",
            "ValidationException",
            "ConflictException",
            "ThrottlingException",
            "InternalServerException",
        ] {
            let answered = with_error_type_locator(response(400, Some(shape)), &locator(SHAPE_ID));
            let expected = format!("{shape}{}", locator(SHAPE_ID));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(expected.as_str()),
                "case {shape:?}: expected {expected:?}, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// The shape is read out of every form a renderer can state it in, and a value that
    /// already carries the locator is not doubled.
    #[test]
    fn the_shape_is_read_out_of_every_form_and_qualified_once() {
        for (case, named) in [
            ("bare", "ResourceNotFoundException".to_string()),
            (
                "namespaced",
                "com.amazonaws.docdbelastic#ResourceNotFoundException".to_string(),
            ),
            (
                "already qualified",
                format!("ResourceNotFoundException{}", locator(SHAPE_ID)),
            ),
        ] {
            let answered =
                with_error_type_locator(response(404, Some(&named)), &locator(SHAPE_ID));
            let expected = format!("ResourceNotFoundException{}", locator(SHAPE_ID));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// Nothing to qualify: a success, and a failure whose renderer named no shape (or an
    /// empty one), are left exactly as they were.
    #[test]
    fn a_response_that_names_no_shape_is_left_as_it_was() {
        for (case, status, named) in [
            ("a success", 200, Some("ResourceNotFoundException")),
            ("no header", 404, None),
            ("an empty header", 404, Some("")),
        ] {
            let answered =
                with_error_type_locator(response(status, named), &locator(SHAPE_ID));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                named,
                "case {case:?}: expected the header left as it was ({named:?}), got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answered.status().as_u16(),
                status,
                "case {case:?}: the status must not move, got {}",
                answered.status().as_u16()
            );
        }
    }
}
