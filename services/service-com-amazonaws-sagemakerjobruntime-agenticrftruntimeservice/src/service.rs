// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: this service's
//!   SigV4 signing name (`sagemaker`) is shared with every other SageMaker-family
//!   service, so the claim also requires the request to address this service's own
//!   modeled URI path space.
//! - `override_operation` — which operation serves a request. EDITED HERE: the key and
//!   the handler stay the generated table's, but the operation is served through
//!   [`InModelledEnvelope`], which refuses a request outside a modelled constraint with
//!   this service's own `ValidationException` — whose prose member the model calls
//!   `Message` — instead of the framework's pre-dispatch envelope, which names only a
//!   lowercase `message`.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: a value outside a declared constraint is
//!   `ValidationException`, worded from the violations the framework collected, and a
//!   value that is not an ARN at all is reported as a malformed ARN.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The routing keys that name THIS service rather than its signing name: the
/// model's endpoint prefix (`job-runtime.sagemaker`) and that prefix's first
/// label, which is what `emulator_server::service_from_host` derives from this
/// service's own endpoint host (`job-runtime.sagemaker.<region>.amazonaws.com`).
///
/// The signing name (`sagemaker`) is shared with every other SageMaker-family
/// service, so it cannot identify this one on its own; the endpoint prefix can.
const ENDPOINT_KEYS: [&str; 2] = ["job-runtime.sagemaker", "job-runtime"];

/// The dotted member path of this service's ARN-valued member, as the validating walk
/// spells it: `JobArn` in lowerCamel. Every operation of this service binds it, so the
/// path is matched by its tail rather than by whole-string equality.
const ARN_MEMBER_PATH: &str = "jobArn";

/// The error code a refused request answers with, unchanged from the one the framework
/// would have chosen for a constraint violation: the shape
/// `com.amazonaws.sagemakerjobruntime#ValidationException`, which every operation of
/// this service lists and whose one prose member the model calls `Message`
/// (`smithy_query(query="shape",shape="ValidationException")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// How this service reports a value that is not an ARN at all.
///
/// Built from the value the request sent, which is the only thing the caller can act on.
#[must_use]
fn malformed_arn_message(value: &str) -> String {
    format!("Input arn {value} is malformed")
}

/// Whether `violation` is an ARN-valued member holding something that is not an ARN.
///
/// The member is named by the LAST segment of the dotted path, so a member whose name
/// merely ends in those letters (`notAJobArn`) is not mistaken for it.
fn is_malformed_arn(violation: &Violation) -> bool {
    violation.path.rsplit('.').next() == Some(ARN_MEMBER_PATH)
        && matches!(violation.kind, ViolationKind::Pattern(_))
}

/// The sentence this service refuses `violations` with, or `None` when there is nothing
/// to refuse.
///
/// Every clause comes from the framework's own constraint renderer
/// (`constraints::envelope`), so a constraint class this service has never seen cannot
/// arrive with a sentence of its own. ONE class is worded differently: a value that is
/// not an ARN at all. Every member of this service whose value is an ARN (`JobArn`, on
/// all four operations) carries the `@pattern`
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/…$`, and the service reports a
/// value that fails it as a malformed ARN naming the value it was given, rather than as
/// a regular-expression clause: a caller cannot act on the pattern source, and the ARN
/// is the thing it got wrong.
fn refusal_message(violations: &[Violation]) -> Option<String> {
    // Only when the value is actually known: a pattern violation that captured none
    // would otherwise be reported as "Input arn  is malformed", which tells the caller
    // nothing, so the framework's own clause is kept for it instead.
    let malformed_arn = violations
        .iter()
        .filter(|violation| is_malformed_arn(violation))
        .find_map(|violation| violation.value.as_deref());
    if let Some(value) = malformed_arn {
        return Some(malformed_arn_message(value));
    }
    constraints::envelope(violations)
}

/// The names that identify THIS service to the router: its SigV4 signing name — shared
/// with every other SageMaker-family service, so it cannot identify this one on its own
/// — and its own [`ENDPOINT_KEYS`].
fn claiming_names(signing_name: &str) -> impl Iterator<Item = &str> {
    std::iter::once(signing_name).chain(ENDPOINT_KEYS)
}

/// The `com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before the handler runs is answered
    /// in this service's own `ValidationException`. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            complete_rollout::CompleteRollout,
            sample::Sample,
            sample_with_response_stream::SampleWithResponseStream,
            update_reward::UpdateReward,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated in this service's own
/// `ValidationException`.
///
/// The framework's pre-dispatch renderer gets the status (400) and the error code
/// (`ValidationException`) right, but writes the prose into a framework-chosen lowercase
/// `message` key, the same for every restJson1 service. This service models the prose
/// member of each of its error shapes as `Message` (`FailureReason`, `@required`), so
/// that envelope carries no `Message` at all and an SDK surfaces a constraint refusal
/// with an empty reason.
///
/// So this wrapper collects the refusal itself — the very two classes the framework's own
/// walk collects, from the framework's own deserializer and constraint walk — and renders
/// it through the model's `ValidationException` schema. The envelope is built from the
/// structured [`Violation`]s, never from a rendered response. Nothing else is
/// re-implemented: a request this service does not refuse is handed to the generated
/// `ErasedOperation::run`, which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, &violations) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a declared constraint. Empty when the request is valid,
/// and empty for a failure that is NOT a refusal — a body the codec cannot read is a
/// serialization failure, which the framework's own walk answers in its own envelope.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), so this cannot find a different set of
/// violations from the walk that follows it.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        }],
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations`: HTTP 400 `ValidationException` with the prose
/// in the `Message` member the model declares, serialized through that shape's own
/// schema. `None` when there is nothing to refuse — and also when the error cannot be
/// serialized, which leaves the framework's walk to refuse the request in its own
/// envelope rather than answering a 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::ops::types::shared_types::ValidationException {
        message: refusal_message(violations)?,
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::shared_types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `sagemaker` — a signing name it SHARES with every
        // other SageMaker-family service (the control plane, the endpoint runtimes,
        // A2I, Edge, FeatureStore, Metrics, the training-session runtime), while its
        // own endpoint is `job-runtime.sagemaker`. The router resolves the routing key
        // from the SigV4 credential scope first, so all of them see the key
        // `sagemaker`: a service that claims on the signing name ALONE claims every
        // sibling's traffic too, and the router — which refuses rather than guesses —
        // answers `500 AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (`/sample`, `/sample-with-response-stream`,
        // `/complete-rollout`, `/update-reward`), which is what `claims_request` matches
        // against the generated `ROUTE_PATH_SEGMENTS` when a service declares no RPC
        // protocol. The rule is applied to every name that identifies this service, so a
        // request addressed at its own endpoint host rather than signed reaches it too.
        claiming_names(self.entry.identity().signing_name()).any(|name| {
            service_common::claims_request(parts, name, crate::ROUTE_PATH_SEGMENTS, None)
        })
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused before the handler runs is refused
        // in this service's `ValidationException`, whose prose member the model calls
        // `Message`, rather than in the framework's pre-dispatch envelope, which names
        // only a lowercase `message`. A key the model does not name resolves to `None`
        // here exactly as it does in the generated table, so the router's routing-error
        // path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam is what words any refusal the FRAMEWORK still renders.
        // Wording it here from the same violations keeps the two paths from disagreeing:
        // a value outside a declared bound is this service's `ValidationException`
        // either way, with the same sentence (see `refusal_message`).
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: refusal_message(violations)?,
            }),
            // Authentication is the framework's to word: this service models
            // `AccessDeniedException` for a caller it refuses, and the request never
            // reached an operation, so there is nothing of this service's own to say.
            _ => None,
        }
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
