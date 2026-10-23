// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mpa-awsfluffycoreservice` service: how it claims requests and how it words the
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
//! # The organization gate stands in FRONT of the request walk
//!
//! An operation this service restricts to the organization's MANAGEMENT ACCOUNT (the
//! creates that set an organization's multi-party approval up) is
//! authorized from the CALLER and the recorded organization alone — no value in the
//! request takes part in that decision — so the decision does not wait for the request
//! to be read or validated: a request whose body breaks a modelled constraint earns the
//! same 403 `AccessDeniedException` as a conforming one, and a refused caller never
//! learns whether its body would also have been refused.
//!
//! That ordering is unreachable from inside a handler. The framework's centralized walk
//! (deserialize -> validate -> `handle` -> serialize) refuses every modelled constraint
//! BEFORE a handler runs, and [`Service::override_refusal`] can only reword that refusal
//! — it cannot change its status, and it is handed the violations alone, with no caller
//! and no operation to authorize. So the gate is wired where it can run first:
//! [`ManagementAccountGate`] wraps each gated operation and is served through
//! [`Service::override_operation`], ahead of the generated routing table.
//!
//! The gate only REFUSES: a caller it permits is served by the one generated operation
//! in the routing table, which still owns the whole walk and every modelled error. Each
//! gated handler also keeps its own copy of the check, so no path can write to the store
//! for a caller the gate would refuse.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused,
    ServerHttpProtocol, Service,
};

/// The wire error code (`__type` / `x-amzn-errortype`) a management-account refusal
/// carries: the `AccessDeniedException` shape the model lists as a common error of every
/// operation of this service, and the same name a handler-returned one carries.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The status that refusal is served on: `AccessDeniedException`'s own
/// `@httpError(403)`.
const ACCESS_DENIED_EXCEPTION_STATUS: u16 = 403;

/// The `com-amazonaws-mpa-awsfluffycoreservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ManagementAccountGate`] per operation this service restricts to the
    /// organization's management account, keyed by the routing key the model names that
    /// operation by ([`Operation::NAME`], the same key the generated routing table
    /// uses). Built once here, never per request; each entry holds the SAME engine
    /// `Arc` the generated entry holds, so this is a table of names rather than a
    /// second set of operations or a second store.
    gates: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gates: management_account_gates::<Eng>(&engine),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The organization gate of each operation this service restricts to the management
/// account, keyed by the routing key the model names that operation by.
///
/// ONE list, read from the operations' own `Operation::NAME`, so a gated operation's key
/// cannot drift from the model. Multi-party approval is an AWS Organizations feature, and
/// the operations that SET AN ORGANIZATION'S MULTI-PARTY APPROVAL UP are the ones its
/// management account owns: every handler whose authorization decision is
/// `state::is_management_account(state::caller_account_id())` belongs here, and the gate
/// is how that decision is reached ahead of validation. Today that is
/// `CreateApprovalTeam` (the operation the recorded refusal was observed on) and
/// `CreateIdentitySource` (the same check, in `ops::create_identity_source`). Adding
/// another gated operation is one more entry here, gated ahead of validation like these,
/// with no second rule to write.
///
/// The operations that identify their resource BY ARN (`GetApprovalTeam`,
/// `UpdateApprovalTeam`, `StartActiveApprovalTeamDeletion`, …) are deliberately NOT here:
/// their authorization decision reads a value out of the request (the ARN's account
/// field), so it belongs after the request is read — it is
/// [`crate::state::authorize_resource_arn`], applied in each of those handlers.
fn management_account_gates<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $({
                let name = <crate::ops::$module::$operation<Eng> as Operation>::NAME;
                gates.insert(
                    name,
                    Box::new(ManagementAccountGate::new(Arc::clone(engine), name))
                        as Box<dyn ErasedOperation>,
                );
            })+
        };
    }
    gate!(
        create_approval_team::CreateApprovalTeam,
        create_identity_source::CreateIdentitySource,
    );
    gates
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

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // Serve an operation this service restricts to the organization's management
        // account through its [`ManagementAccountGate`], so the authorization decision
        // is made where the real service makes it: ahead of reading and validating the
        // request (see the module docs). Every other operation resolves through the
        // generated routing table, and so does a gated one whose caller the gate
        // permits — the wrapper delegates to it rather than serving the request itself.
        //
        // A request that names no modelled operation has nothing to authorize: the
        // routing failure is the answer it gets.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gates.get(key).map(|gate| &**gate)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording.
        //
        // This seam words a refusal; it cannot decide an authorization question.
        // It is handed the violations ALONE — no caller and no operation — and
        // cannot move the status off the 400 a constraint refusal carries, which
        // is why the organization gate lives in front of the walk instead (see
        // the module docs) and why a refusal that reaches here is always a
        // PERMITTED caller's bad request.
        //
        // MPA validates an Amazon Resource Name as a SERVICE RULE, not as a
        // per-member model constraint: a value that is not a well-formed MPA ARN
        // earns one fixed sentence
        // ([`crate::state::INVALID_ARN_MESSAGE`]) that names neither the member
        // nor the grammar, whichever ARN-valued member carried it. The framework
        // would instead render the constraint report it builds for every
        // violation — echoing the member name and the model's own regex, neither
        // of which this service exposes — so the ARN class is reworded here, in
        // the one place every operation's boundary refusal passes through,
        // rather than per operation.
        //
        // The code is replaced along with the message, and with the same code the
        // framework picks for a constraint refusal on this protocol
        // (`ValidationException`, which is also what this service's model
        // declares for a refused input, HTTP 400): the status and the envelope
        // stay with the protocol.
        match refusal {
            Refusal::Invalid(violations)
                if violations
                    .iter()
                    .any(crate::state::is_malformed_arn_violation) =>
            {
                Some(Refused {
                    code: "ValidationException".to_string(),
                    message: crate::state::INVALID_ARN_MESSAGE.to_string(),
                })
            }
            // Every other refusal keeps the framework's wording: this service has
            // one recorded rule for the ARN class and none for the rest.
            _ => None,
        }
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
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
    ) -> Option<RtResponse> {
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

/// One operation this service restricts to the organization's management account, served
/// behind that authorization gate.
///
/// Holds no operation of its own — only the shared engine (the same `Arc` the generated
/// entry holds, never a second store) and the routing key of the operation it stands in
/// front of. A caller the store records as the management account is served by that one
/// generated operation, which runs the whole framework sequence over the same bytes.
struct ManagementAccountGate<Eng> {
    engine: Arc<Eng>,
    /// The routing key of the gated operation, taken from its own `Operation::NAME`.
    operation: &'static str,
}

impl<Eng: Send + Sync + 'static> ManagementAccountGate<Eng> {
    const fn new(engine: Arc<Eng>, operation: &'static str) -> Self {
        Self { engine, operation }
    }

    /// Whether this service's store refuses the calling account.
    ///
    /// A READ: the store's lock is taken for the comparison and released again, and
    /// nothing is materialised or written, so consulting the gate cannot change what a
    /// later caller sees. A store this service cannot reach at all refuses nobody here —
    /// the operation's own handler answers that with the modelled server error rather
    /// than this reporting a denial it did not decide.
    fn denies_the_caller(&self) -> bool {
        crate::state::service_state(&*self.engine).is_some_and(|store| {
            !store
                .lock()
                .is_management_account(crate::state::caller_account_id())
        })
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ManagementAccountGate<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = Instant::now();
            if self.denies_the_caller() {
                // Decided from the caller and the recorded organization alone, so it is
                // decided HERE: ahead of the deserialize-and-validate boundary the
                // generated walk below runs, and therefore ahead of every answer that
                // boundary could give.
                return denial(
                    svc.definition().protocol(),
                    ctx,
                    self.operation,
                    started,
                );
            }
            // Nobody to refuse: the request is served by the generated operation, which
            // owns deserialize -> validate -> handle -> serialize and every modelled
            // error, over the same bytes.
            match svc.definition().routing_table().get(self.operation) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed (or not) as
        // the MODEL says for the operation it names, so authentication is decided by the
        // modelled operation this wrapper stands in front of.
        None
    }
}

/// The management-account refusal of one operation: HTTP 403 `AccessDeniedException`,
/// worded by [`crate::state::not_management_account`] — the one place this service states
/// that refusal, so the gate and a handler's own copy of the check cannot disagree.
///
/// Reached only for a caller [`ManagementAccountGate::denies_the_caller`] judged
/// unauthorized.
fn denial(
    protocol: &dyn ServerHttpProtocol,
    ctx: &OperationContext<'_>,
    operation: &'static str,
    started: Instant,
) -> RtResponse {
    let exception = crate::state::not_management_account();
    // Serialized through the service's own protocol, exactly as a handler-returned
    // `AccessDeniedException` is: the same body member name (`Message`), the same
    // `__type`, the same `x-amzn-errortype` header restJson1 resolves an error shape
    // from, and the status the model declares for it.
    let response = match protocol.serialize_error(
        &exception,
        &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
        ACCESS_DENIED_EXCEPTION,
        ACCESS_DENIED_EXCEPTION_STATUS,
    ) {
        Ok(response) => response,
        // An authorization decision never fails OPEN: a modelled error that will not
        // serialize is still a refusal, answered in this protocol's own envelope
        // (restJson1: the 403, the error-type header and a JSON body) rather than by
        // letting the request through to the operation.
        Err(error) => {
            tracing::warn!(
                operation,
                %error,
                "the modelled AccessDeniedException would not serialize; this refusal \
                 falls back to a hand-built 403"
            );
            unserialized_denial(&exception.message)
        }
    };
    // Recorded for BOTH renderings of the one refusal: the request was refused with this
    // status and this error code either way, so the telemetry series must not depend on
    // which envelope carried it.
    record_refusal(
        ctx,
        operation,
        ACCESS_DENIED_EXCEPTION,
        ACCESS_DENIED_EXCEPTION_STATUS,
        started.elapsed(),
    );
    response
}

/// The 403 a refusal falls back to when the modelled shape will not serialize: the same
/// status, the same wire error code and the same `Message`, in restJson1's own envelope.
fn unserialized_denial(message: &str) -> RtResponse {
    let body = ::serde_json::json!({
        "__type": ACCESS_DENIED_EXCEPTION,
        "Message": message,
    })
    .to_string();
    let status = StatusCode::try_from(ACCESS_DENIED_EXCEPTION_STATUS)
        .expect("the modelled 403 is a valid HTTP status");
    let mut response = RtResponse::new(status, SdkBody::from(body));
    let headers = response.headers_mut();
    headers.insert("content-type", "application/json");
    headers.insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
    response
}

/// Record a refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as
/// the framework gates it.
fn record_refusal(
    ctx: &OperationContext<'_>,
    operation: &'static str,
    error_code: &'static str,
    status: u16,
    latency: std::time::Duration,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), operation);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            status, error_code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}
