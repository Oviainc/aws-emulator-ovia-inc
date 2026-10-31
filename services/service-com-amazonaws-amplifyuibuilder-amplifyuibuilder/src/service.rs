// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-amplifyuibuilder-amplifyuibuilder` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. THIS SERVICE
//!   uses it to serve every routable request through
//!   [`untyped_errors::UntypedErrorOperation`], which runs the operation the routing
//!   table names and then removes the error-type discriminator from the response the
//!   framework rendered, for the reason named in [`untyped_errors`]: AmplifyUIBuilder
//!   names none of its exceptions on the wire, so a client classifies a refusal by
//!   status. That same wrapper is the single response path this service's responses
//!   leave through, so it is also where [`front_door`] finishes each one with the two
//!   headers AWS's fronting layer adds to every AmplifyUIBuilder response.
//! - `override_routing_error` — see below: this service renders the framework's own
//!   routing failure and then finishes it through [`front_door`] too, so a response
//!   that named no operation declares the same media type and validator as one that
//!   did.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS SERVICE words one here: a request that fails the
//!   model's own input rules is answered with the exception this service's model
//!   declares for an invalid input value, so a refusal caught at the boundary and one
//!   caught in a handler name the same shape. See the inline note on that method.
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

pub mod front_door;
pub mod untyped_errors;

/// The exception this service's model declares for an input value its own rules refuse:
/// the one shape documented as "an invalid or out-of-range value was supplied for the
/// input parameter", and a common error of every operation
/// (`smithy_query(query="shape",shape="InvalidParameterException",model_path="services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json")`
/// — `errorKind` client, `httpError` 400, which is the status the framework already
/// answers a refusal with).
///
/// It is what [`EmulatedService::override_refusal`] names for a refusal the framework
/// found while reading or walking the request, so an out-of-bound value and a value a
/// handler checks itself are reported as one class. Removing the discriminator from the
/// rendered response is [`untyped_errors`]'s job, as for every other exception of this
/// service.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The `com-amazonaws-amplifyuibuilder-amplifyuibuilder` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper `override_operation` hands back for every routable request,
    /// built once because that seam returns a reference to it.
    untyped: untyped_errors::UntypedErrorOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            untyped: untyped_errors::UntypedErrorOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every routable request is served through the wrapper: it runs the
        // operation the generated routing table names — the framework still owns
        // the whole walk and every status, message and member it answers with —
        // and then removes the error-type discriminator from whatever it answered
        // (see [`untyped_errors`]). A request the routing table cannot serve is
        // left to the framework's own unroutable-request answer.
        untyped_errors::routes(&self.entry, req).then_some(&self.untyped as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // A request that fails the model's own input rules is refused for the reason
        // [`INVALID_PARAMETER_EXCEPTION`] is declared for, so it is answered with that
        // shape rather than with the framework's `ValidationException` — a shape this
        // service's model does not declare, and the one error this service would
        // otherwise name on the wire (`untyped_errors` strips the discriminator of a
        // modelled exception, so this is also what makes a boundary refusal leave in
        // this service's own untyped form). It covers both classes the framework
        // reports structurally — a required member that was absent, and a value
        // outside a declared `@length` / `@range` / `@pattern` / enum / uniqueness
        // bound — because to a caller both are an input parameter this service would
        // not accept, and this service declares no other 400 for one.
        //
        // The MESSAGE is the one the framework renders for the same violations, so
        // only the code moves: `override_refusal` replaces the code and the message
        // together, so the message is restated rather than changed. `None` when there
        // is nothing to state, which keeps the framework's default wording.
        //
        // An authentication refusal is left alone: it is not an input-value failure,
        // and it is answered before this service's response path runs.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_PARAMETER_EXCEPTION.to_string(),
                message: aws_smithy_schema_server::envelope(violations)?,
            }),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. The ENVELOPE is left to the framework: this is
        // the protocol-correct default rendering (rest -> 501), rendered here rather
        // than by returning `None` only so that it, too, leaves through this
        // service's front door — the media type and the `ETag` AWS's fronting layer
        // puts on every response it serves, including one that named no operation.
        Some(front_door::served(self.entry.routing_error_response(req)))
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
        //
        // NOT finished through `front_door`, unlike every response this service's own
        // path renders: the default rendering is reached only by MOVING the error into
        // `framework_error_response`, and this seam holds a borrow of an error that is
        // not `Clone`, so there is nothing here to finish. A transport failure the
        // emulator detects before this service ran is the emulator's own signal rather
        // than an AmplifyUIBuilder response, so it is left to the framework whole.
        let _ = error;
        None
    }
}
