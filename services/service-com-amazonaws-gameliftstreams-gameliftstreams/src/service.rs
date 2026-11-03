// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-gameliftstreams-gameliftstreams` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. The routing key and the handler stay the
//!   generated table's; every modelled operation is served through
//!   [`ConstraintRefusal`], which states a request refused for breaking a modelled
//!   constraint in this service's OWN `ValidationException`. See "Why this file
//!   validates input" below.
//! - `override_refusal` — EDITED HERE. A refusal the framework still renders (one
//!   with no modelled operation to read the request against, and the malformed
//!   `ResourceArn` this API answers `BadRequestException` for) is worded from the
//!   same violations through the same [`refusal_message`], so the two refusal paths
//!   cannot disagree.
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
//! ## Why this file validates input
//!
//! `com.amazonaws.gameliftstreams#ValidationException` (`@error("client")`,
//! `@httpError(400)`) is a COMMON error of every operation of this service and its
//! `Message` member is `@required`, so the 400 a caller gets for a value outside a
//! rule the model declares always carries `$.Message` — the text an SDK surfaces as
//! the exception's message.
//!
//! The framework's pre-dispatch envelope cannot answer in a modelled shape: at that
//! point the refusal is a protocol-level fact rather than one of this service's error
//! SHAPES, and the only seam over it ([`Service::override_refusal`]) chooses an error
//! code and a sentence, not a body — so the sentence goes out under the JSON
//! protocols' generic lowercase `message` key and the modelled, required `Message` is
//! absent from the body entirely.
//!
//! [`ConstraintRefusal`] closes that gap: `override_operation` routes every modelled
//! operation through it, it asks the framework's OWN boundary walk — read the request
//! into the operation's input shape, then run the model's constraint traits over what
//! was read (`aws_smithy_schema_server::constraints::validate`, the same call the
//! centralized sequence makes) — what the request breaks, and renders those
//! violations as the modelled `ValidationException`. The rules enforced are therefore
//! exactly the model's, and the words are the framework's own constraint envelope
//! (`N validation error(s) detected: <clause>`) with the two enumerated sets this
//! service does not spell the model's way replaced by its own
//! (`crate::state::service_violation`). A request the model accepts is delegated,
//! untouched, to the generated operation in the routing table, so each handler, its
//! state and its 501 are exactly as before.
//!
//! One refusal is deliberately NOT answered that way: a `ResourceArn` that is not an
//! ARN at all, which this API answers with the error code `BadRequestException`.
//! That shape is in no model of this service, so there is no modelled member to carry
//! its text, and it keeps the framework's envelope through `override_refusal` below.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use aws_smithy_schema_server::envelope;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// The wire error code (`__type` / `x-amzn-errortype`) of this service's
/// input-validation refusal: the shape the model lists as a common error of every
/// operation for "One or more parameter values in the request fail to satisfy the
/// specified constraints", and the same name a handler-returned
/// `ValidationException` carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status an input-validation refusal is served on: `ValidationException`'s own
/// `@httpError(400)`.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The wire error code this API answers a `ResourceArn` that is not an ARN with —
/// not the `ValidationException` the rest of the API refuses a bad value with, and
/// not a code the model lists for the tagging operations at all. A client branching
/// on the code needs the code the service sends.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The text that refusal carries.
const BAD_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The `com-amazonaws-gameliftstreams-gameliftstreams` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ConstraintRefusal`] per modelled operation, keyed by the routing key the
    /// model names that operation by ([`Operation::NAME`], the same key the generated
    /// routing table uses). Built once here, never per request; each entry holds the
    /// operation's TYPE only, so this is a table of names, not a second set of
    /// operations and not a second store.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            refusals: constraint_refusal_table::<Eng>(),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The modelled-constraint refusal of every operation this service declares, keyed
/// by the routing key the model names each operation by.
///
/// A constraint is a property of the INPUT SHAPE and `ValidationException` is a
/// common error of every operation of this service, so the check goes in front of
/// all of them — implemented or still scaffolded. A stub answers 501 for a request
/// it could serve and the modelled 400 for one no operation could, which is the
/// order the real service evaluates them in.
///
/// An operation left out of this list keeps the framework's generic envelope — the
/// defect this check removes — so the list is not left to review:
/// `every_modelled_operation_states_a_constraint_refusal_in_the_modelled_shape` in
/// `tests/all/refusal_envelope.rs` drives every operation the model declares and
/// fails when one of them is not served through the wrapper.
fn constraint_refusal_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $({
                refusals.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(ConstraintRefusal::<crate::ops::$module::$operation<Eng>>::new())
                        as Box<dyn ErasedOperation>,
                );
            })+
        };
    }
    register!(
        add_stream_group_locations::AddStreamGroupLocations,
        associate_applications::AssociateApplications,
        create_application::CreateApplication,
        create_stream_group::CreateStreamGroup,
        create_stream_session_admin_shell::CreateStreamSessionAdminShell,
        create_stream_session_connection::CreateStreamSessionConnection,
        create_stream_url::CreateStreamUrl,
        delete_application::DeleteApplication,
        delete_stream_group::DeleteStreamGroup,
        disassociate_applications::DisassociateApplications,
        export_stream_session_files::ExportStreamSessionFiles,
        get_application::GetApplication,
        get_stream_group::GetStreamGroup,
        get_stream_session::GetStreamSession,
        get_stream_url::GetStreamUrl,
        list_application_shader_caches::ListApplicationShaderCaches,
        list_applications::ListApplications,
        list_stream_groups::ListStreamGroups,
        list_stream_sessions::ListStreamSessions,
        list_stream_sessions_by_account::ListStreamSessionsByAccount,
        list_stream_urls::ListStreamUrls,
        list_tags_for_resource::ListTagsForResource,
        remove_stream_group_locations::RemoveStreamGroupLocations,
        revoke_stream_url::RevokeStreamUrl,
        start_stream_session::StartStreamSession,
        tag_resource::TagResource,
        terminate_stream_session::TerminateStreamSession,
        untag_resource::UntagResource,
        update_application::UpdateApplication,
        update_stream_group::UpdateStreamGroup,
    );
    refusals
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
        // Serve every modelled operation through its [`ConstraintRefusal`] wrapper, so
        // a request refused before the handler runs is stated in this service's OWN
        // `ValidationException`, whose `@required Message` member is what puts
        // `$.Message` in the body, instead of the framework's generic envelope. The
        // wrapper delegates a request that passes to the generated operation in the
        // routing table, which stays the only thing that serves it.
        //
        // A request that names no modelled operation has nothing to validate: the
        // routing failure is the answer it gets.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The FALLBACK path. A refusal of a request that named a modelled operation
        // is answered by [`ConstraintRefusal`] above, in the modelled shape; what
        // still arrives here is a refusal with no modelled operation to read the
        // request against, and the malformed-`ResourceArn` refusal the wrapper
        // deliberately leaves to this seam. Both are worded from the same
        // violations through the same helpers, so the two paths cannot disagree.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };

        // A `ResourceArn` that is not an ARN at all is answered by this API with
        // the error code `BadRequestException` (HTTP 400). That shape is in no
        // model of this service, so it has no modelled `Message` member to carry
        // its text and it keeps this envelope.
        //
        // A `resourceArn` pattern violation takes PRECEDENCE over any other
        // violation in the same request, deliberately: a request that is malformed
        // this way and in another way too is answered as a bad resource ARN,
        // because that is the refusal the caller has to fix first — the ARN names
        // what the rest of the request is about.
        if is_bad_resource_arn(violations) {
            return Some(Refused {
                code: BAD_REQUEST_EXCEPTION.to_string(),
                message: BAD_RESOURCE_ARN_MESSAGE.to_string(),
            });
        }

        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: refusal_message(violations)?,
        })
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

/// Whether `violations` are the malformed-`ResourceArn` refusal this API answers
/// `BadRequestException` for, which takes precedence over every other violation in
/// the same request.
fn is_bad_resource_arn(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        violation.path == crate::state::RESOURCE_ARN_PATH
            && matches!(violation.kind, ViolationKind::Pattern(_))
    })
}

/// How this service words a refusal the model's own constraints produced: the
/// framework's constraint envelope (`N validation error(s) detected: <clause>`)
/// over the violations as THIS service states them
/// (`crate::state::service_violation`).
///
/// `None` only when there is no violation to state, which is also the one case
/// neither caller reaches.
fn refusal_message(violations: &[Violation]) -> Option<String> {
    let stated: Vec<Violation> = violations
        .iter()
        .map(crate::state::service_violation)
        .collect();
    envelope(&stated)
}

/// One modelled operation, wrapped so the refusal that precedes its handler — a
/// value outside a constraint the model declares, or a member it requires that the
/// request left out — is answered in this service's own `ValidationException`
/// rather than the framework's generic envelope.
///
/// Holds no operation and no engine: it carries the operation TYPE, which is all
/// the check needs (the input shape to read the request into and the input `Schema`
/// to validate it against), and delegates a request that passes to the one
/// operation instance the generated routing table built from the shared engine.
/// There is therefore no second instance of any operation and no second store.
struct ConstraintRefusal<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ConstraintRefusal<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ConstraintRefusal<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let protocol = svc.definition().protocol();
            if let Some(exception) = modelled_refusal::<Op>(protocol, req) {
                // Serialized through the service's own protocol, exactly as a
                // handler-returned `ValidationException` is: the same body member
                // name (`Message`), the same `__type`, the same `x-amzn-errortype`
                // header restJson1 resolves an error shape from, and the status the
                // model declares for it.
                match protocol.serialize_error(
                    &exception,
                    &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                    VALIDATION_EXCEPTION,
                    VALIDATION_EXCEPTION_STATUS,
                ) {
                    Ok(response) => {
                        record_refusal(ctx, Op::NAME, started.elapsed());
                        return response;
                    }
                    // A modelled error that will not serialize leaves the request to
                    // the framework, which refuses it in the generic envelope — the
                    // very divergence this wrapper removes, so it is logged rather
                    // than passed over silently.
                    Err(error) => tracing::warn!(
                        operation = Op::NAME,
                        %error,
                        "the modelled ValidationException would not serialize; this \
                         refusal falls back to the framework's generic envelope"
                    ),
                }
            }
            // Nothing to reword: the request is served by the generated operation,
            // which runs the whole framework sequence (deserialize, validate,
            // handle, serialize) over the same bytes.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the
                // key the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed (or not)
        // as the MODEL says for the operation it names, so authentication is decided
        // by the modelled operation this wrapper stands in front of.
        None
    }
}

/// This service's `ValidationException` for the request `Op` cannot be served from,
/// or `None` when it has nothing of its own to say — in which case the request goes
/// on to the generated operation and the framework answers as it always did.
///
/// This is the boundary walk the framework runs, in the framework's order and with
/// the framework's own code: read the request into the operation's input shape, then
/// run constraint-trait validation over what was read. Only the ENVELOPE is this
/// service's.
fn modelled_refusal<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<crate::ops::types::ValidationException> {
    let input = {
        let mut deserializer = protocol.deserialize_input(req, Op::INPUT_SCHEMA).ok()?;
        match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
            Ok(input) => input,
            // A member the shape requires is absent. It is found while READING the
            // request rather than by walking what was read, but to a caller it is
            // the same refusal — so it is stated the same way, naming the member and
            // the rule it broke.
            Err(SerdeError::MissingMember { member_name }) => {
                let absent = Violation {
                    // The validating walk spells a member's path in lowerCamel; the
                    // codec hands over the Smithy name, so it is cased here to the
                    // one spelling the clause builder uses.
                    path: constraints::path::member(&member_name),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                };
                return refusal_message(&[absent]).map(validation_exception);
            }
            // Any other codec failure is not a refusal this service rewords.
            Err(_) => return None,
        }
    };
    let violations = constraints::validate(&input, Op::INPUT_SCHEMA);
    if violations.is_empty() || is_bad_resource_arn(&violations) {
        // A malformed `ResourceArn` is this API's `BadRequestException`, which is no
        // modelled shape of this service: it keeps the framework's envelope through
        // `override_refusal`, so the request is left to the framework path.
        return None;
    }
    refusal_message(&violations).map(validation_exception)
}

/// This service's modelled refusal carrying `message` — the one place the boundary's
/// `ValidationException` is built from a sentence. The shape models `Message` alone.
fn validation_exception(message: String) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException { message }
}

/// Record a refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the
/// same classification (from the status and error shape this refusal carries), the
/// same folded handle (this service's signing name and the operation's own name),
/// and the same already sanitized request metadata. Everything is gated on a client
/// collecting, exactly as the framework gates it.
fn record_refusal(ctx: &OperationContext<'_>, operation: &'static str, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), operation);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            VALIDATION_EXCEPTION_STATUS,
            VALIDATION_EXCEPTION,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}
