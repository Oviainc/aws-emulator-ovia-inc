// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ses-simpleemailservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! Two of those seams are taken here. One is `can_handle` (this service's signing name is
//! shared — see that method). The other is the `awsQuery` ERROR ENVELOPE: a refusal of
//! this API answers `<ErrorResponse>` with a `<RequestId>` beside its `<Error>`, naming
//! the request it refused, which no model declares and the framework's error renderers do
//! not write. So the generated definition and every routable operation are served behind
//! `service_common::query_error_envelope`'s two wrappers, which leave every status, code
//! and message where they are and add that one element — the error-path counterpart of
//! the `ResponseMetadata/RequestId` a successful response already carries.

use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdEnvelopeOperation,
};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// How this service WORDS a request the framework refused before any handler ran, and
/// which error code it names. Applied through [`EmulatedService::override_refusal`].
pub mod validation;

/// Whether IAM lets a request SEND the message it carries: the request-authorization
/// decision every send of this service takes before it reads the account's identities.
pub mod authorization;

/// This service's endpoint prefix, as its model gives it
/// (`smithy_query(query="service_info")`: `awsService.endpointPrefix` `email`) — the name
/// a host-addressed request resolves to, beside the SigV4 signing name (`ses`) the
/// generated identity carries.
pub const ENDPOINT_PREFIX: &str = "email";

/// The URI path space this service's wire protocol uses: the ROOT path, and nothing else.
///
/// SES v1 speaks `awsQuery` (`smithy_query(query="service_info")`: protocols
/// `["awsQuery"]`) and no operation in its model carries a `smithy.api#http` binding, so
/// every one of its 71 operations arrives as `POST /` with the operation named by the
/// `Action` form field. The generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY for exactly
/// that case — the generator keeps it empty for an RPC service with no bindings so as not
/// to narrow services that already work — and an empty list means "claims every path",
/// which is what made this service collide with its three signing-name peers. The root
/// claim is therefore stated here, using the framework's own sentinel for it. It is
/// deliberately NOT called `ROUTE_PATH_SEGMENTS`: that name belongs to the generated
/// crate-level constant, which is empty.
const WIRE_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// The `com-amazonaws-ses-simpleemailservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, behind the `awsQuery` error-envelope wrapper: the failures the
    /// router renders OUTSIDE the dispatch path — a routing failure, a transport failure
    /// and a pre-dispatch refusal — name the request they answer in the envelope's
    /// `<RequestId>`, as every other error of this service does. Identity, the protocol
    /// and the routing table are the generated entry's, untouched.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// The envelope wrapper every modelled operation of this service is served through,
    /// so a handler-returned modelled error and a constraint refusal raised while an
    /// operation was being served carry that same element. Held in a field because
    /// [`Service::override_operation`] hands back a reference borrowed from `&self`.
    envelope: RequestIdEnvelopeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::new(crate::ServiceDefinition::new(engine)),
            envelope: RequestIdEnvelopeOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        //
        // It is reached through the `awsQuery` envelope wrapper, which leaves identity,
        // the protocol and the routing table exactly as the entry carries them and adds
        // the `<RequestId>` element to the framework-level error envelopes it renders, so
        // they carry what a modelled error served through [`RequestIdEnvelopeOperation`]
        // carries.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `ses` — a signing name it SHARES with three other
        // emulated services: SESv2 and Pinpoint Email (`restJson1`, endpoint `email`) and
        // MailManager (`awsJson1_0` + `rpcv2Cbor`, endpoint `mail-manager`). The router
        // resolves a request's routing key from the SigV4 credential scope first, so all
        // four see the SAME key, and claiming on the signing name alone makes all four
        // claim every request — which the router answers `500 AmbiguousService` for, in a
        // JSON body an awsQuery client cannot even parse.
        //
        // So the claim is narrowed to this service's own wire form: the routing key names
        // this endpoint (its signing name, or its endpoint prefix for a host-addressed
        // request), the request arrives on the root path every `awsQuery` operation posts
        // to (see `WIRE_PATH_SEGMENTS`), and it names no service out of band in an
        // `awsJson` target — an `awsQuery` request carries its operation in the `Action`
        // form field and never an `X-Amz-Target`, while the RPC peer that posts to the
        // same root path always does, and a request that names a service out of band
        // belongs to the service it names. The two `restJson1` peers own their `/v1/…`
        // and `/v2/…` path spaces.
        service_common::routes_to_any(
            parts,
            &[self.entry.identity().signing_name(), ENDPOINT_PREFIX],
        )
            && service_common::claims_modeled_path(parts, WIRE_PATH_SEGMENTS)
            && !parts
                .headers
                .contains_key(service_common::rpc_target::TARGET_HEADER)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: every request the generated routing table resolves is served by
        // the operation THAT table names, through the wrapper that adds the `<RequestId>`
        // an `awsQuery` error envelope carries. The framework still owns the whole
        // deserialize → validate → handle → serialize sequence and every status, code and
        // message it answers with.
        //
        // An `Action` naming no modelled operation is declined here (the wrapper claims
        // only what the routing table holds), so it stays the router's routing failure,
        // whose envelope the wrapped definition names the request in.
        self.entry.routes(req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. The words and the code are [`validation`]'s: this API answers
        // every structural refusal with one code, and states an absent member and an
        // enum rejection without echoing the request, which the framework's own
        // default does neither of. Both halves are replaced together, because a
        // client branches on the code.
        validation::refusal(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
