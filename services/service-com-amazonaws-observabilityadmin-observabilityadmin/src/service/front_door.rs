// SPDX-License-Identifier: CC0-1.0

//! How this service states the refusal of a request the emulator's authentication layer
//! would not verify — the authorization pre-check that stands in front of the
//! ObservabilityAdmin API, ahead of any operation.
//!
//! # The rule
//!
//! Every request to this endpoint is authorized before it is dispatched: the layer in
//! front of the service verifies the SigV4 credentials it carries, and a caller it cannot
//! verify — one that is unsigned, that carries a malformed `Authorization` header, or that
//! presents credentials this emulator does not hold — is refused there, with that layer's
//! own answer and never with one of this service's modelled errors:
//!
//! * HTTP [`UNAUTHORIZED_STATUS`];
//! * the one fixed sentence [`UNAUTHORIZED_MESSAGE`], under a LOWERCASE `message` member —
//!   this service's own modelled errors (`AccessDeniedException`, `ValidationException`,
//!   `ResourceNotFoundException`) state theirs under `Message`, so which member carries the
//!   sentence is what tells a client which layer answered;
//! * the wire error code [`UNAUTHORIZED_CODE`] in `x-amzn-errortype`, the header `restJson1`
//!   resolves an error shape from, and NOTHING in the body beside the sentence: the layer
//!   that states this refusal publishes no error shape for a client to resolve.
//!
//! None of the three is in this service's model — `smithy_query(query="digest")` lists
//! `AccessDeniedException` (400), `InternalServerException` (500) and
//! `TooManyRequestsException` (429) as its service-common errors and no 401 shape at all —
//! which is the point: the refusal happens ahead of the code that owns the modelled errors,
//! so it travels with its own status, code and envelope rather than through an operation's
//! error enum. A caller the layer DOES verify reaches the operation it named and gets this
//! service's own answers, including the modelled `AccessDeniedException` an operation raises
//! itself.
//!
//! # Why it lives in the definition
//!
//! The emulator verifies every request's signature centrally, and a service never checks
//! one itself: the authentication layer refuses the caller before any handler runs and
//! hands the service ONE structural refusal to word
//! ([`ProtocolError::Unauthenticated`]). The only seam handed that refusal is the
//! service's own definition, where `refusal_response` renders it — the same place
//! [`::service_common::authentication_refusal`] restates the refusal for the services
//! whose answer is a modelled 403 shape. This service's answer is not a modelled shape and
//! not a 403, so it is rendered here: the status and the envelope are part of what the
//! front door states, and the wording seam alone cannot replace them.
//!
//! Nothing here reads a credential, a secret, a request body or any state. The layer's
//! verdict is translated into the answer this endpoint states for it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    ErasedServiceDefinition, OperationRoutingTable, Refused, ServerHttpProtocol, ServiceIdentity,
    WireProtocol,
};

/// The HTTP status the front door refuses a caller it could not verify with.
pub const UNAUTHORIZED_STATUS: u16 = 401;

/// The wire error code that refusal names, in `x-amzn-errortype`.
pub const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The one sentence that refusal states, for every caller it refuses.
///
/// Wholly fixed text: it names no member, no value, no resource and no principal, so there
/// is nothing in it derived from the request — and nothing about this account, or about
/// which half of the credential was wrong, reaches a caller the layer refused.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The member that sentence travels under: LOWERCASE `message` (see the module docs).
const UNAUTHORIZED_MESSAGE_MEMBER: &str = "message";

/// This service's model-derived half, with the authentication layer's refusal stated as
/// the front door states it (see the module docs).
///
/// Identity, the routing table, the protocol and every other rendering come from the
/// wrapped definition untouched — nothing model-derived is restated here. Generic over that
/// definition rather than over an engine, so this module names no engine type.
pub struct FrontDoorDefinition<D> {
    inner: D,
}

impl<D: ErasedServiceDefinition> FrontDoorDefinition<D> {
    /// Wrap this service's generated definition.
    pub fn new(inner: D) -> Self {
        Self { inner }
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for FrontDoorDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.inner.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.inner.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        match (&chosen, &error) {
            // The authentication layer refused the caller: this endpoint states that
            // refusal itself, whichever way the credential failed — the sentence names no
            // half of it, so there is nothing per-failure to word. A refusal this service
            // worded through `Service::override_refusal` is passed through, as is every
            // other refusal class (a modelled constraint violation, which is a 400 in this
            // service's own validation envelope).
            (None, ProtocolError::Unauthenticated { .. }) => unauthorized(),
            _ => self.inner.refusal_response(error, chosen, req),
        }
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        self.inner.wire_protocol(req)
    }
}

/// The front door's refusal: the status, the wire error code in the header `restJson1`
/// resolves an error from, and a body carrying the one fixed sentence under its own
/// lowercase member — and nothing else.
pub fn unauthorized() -> RtResponse {
    let body = ::serde_json::json!({ UNAUTHORIZED_MESSAGE_MEMBER: UNAUTHORIZED_MESSAGE });
    let status =
        StatusCode::try_from(UNAUTHORIZED_STATUS).expect("the refusal's status is a constant");
    let mut response = RtResponse::new(status, SdkBody::from(body.to_string()));
    let headers = response.headers_mut();
    // `restJson1`'s own media type, so the body a client parses is the one it parses for
    // every other answer from this endpoint.
    headers.insert("content-type", "application/json");
    headers.insert("x-amzn-errortype", UNAUTHORIZED_CODE);
    response
}
