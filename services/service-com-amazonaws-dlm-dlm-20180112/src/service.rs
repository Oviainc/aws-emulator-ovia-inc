// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dlm-dlm-20180112` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the key
//!   and the handler stay the generated table's, but every operation that models
//!   `InvalidRequestException` is served through [`InModelledEnvelope`], which
//!   (a) authorizes `iam:PassRole` for an `ExecutionRoleArn` the request names,
//!   ahead of the operation — the order the real front door works in, and the only
//!   place a refusal can precede the member's own `@pattern` — and (b) refuses a
//!   request outside a modelled constraint with that operation's own
//!   `InvalidRequestException` instead of the framework's pre-dispatch envelope.
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
//! # How this service refuses a request
//!
//! A request meets three layers before a handler runs, in this order:
//!
//! 1. **`iam:PassRole` for the `ExecutionRoleArn` it names.** Amazon Data
//!    Lifecycle Manager assumes that role, so the authorization precedes even the
//!    member-constraint walk: an ARN that denotes a principal the caller may not
//!    pass is `AccessDeniedException` (403), not a shape refusal.
//! 2. **The modelled input constraints.** A value outside a declared `@pattern`,
//!    `@length` or `@range`, or a `@required` member absent, is this service's own
//!    `InvalidRequestException` (400) — the error every operation that can refuse
//!    a parameter at all declares — carrying the `Code` of that exception class
//!    and a `Message` naming the parameters it refused. The framework's
//!    pre-dispatch envelope would answer the right status under the error type
//!    `ValidationException`, a shape this service's model does not declare, with
//!    the prose in a lowercase `message` key the modelled shape does not have.
//!    The two operations whose model declares no client error
//!    (`GetLifecyclePolicy`, `DeleteLifecyclePolicy`) are left to that envelope:
//!    an operation answers only the errors it models.
//! 3. **The cross-member rules the model cannot express** — which `PolicyDetails`
//!    members may be combined, and which tag keys may be set. Those live in the
//!    handlers, which already hold the state the rule is about.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code of the client error this service refuses a bad request with:
/// the shape `com.amazonaws.dlm#InvalidRequestException`, which every operation
/// that can refuse a parameter declares
/// (`smithy_query(query="shape", shape="InvalidRequestException")`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The HTTP status that shape's `@httpError` declares.
const INVALID_REQUEST_EXCEPTION_STATUS: u16 = 400;

/// The path segment a validation path puts between a collection and its element
/// (`schedules.1.member.createRule`). It names no parameter of the request, so a
/// parameter name drops it.
const COLLECTION_PATH_STEP: &str = "member";

/// The `message` member of the `AccessDeniedException` this service answers a
/// denied `iam:PassRole` with.
static ACCESS_DENIED_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Schema for the `AccessDeniedException` shape.
///
/// Deliberately in the `smithy.framework` namespace, as the scaffold's own
/// synthetic shapes are: the DLM model declares no `AccessDeniedException`
/// (`iam:PassRole` is authorized in the front door, ahead of the operation), so
/// this shape must not collide with a modelled one. The wire `__type` and the
/// `x-amzn-errortype` header carry [`crate::state::ACCESS_DENIED_EXCEPTION`],
/// which `serialize_error` is given explicitly.
static ACCESS_DENIED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException",
        "smithy.framework",
        "AccessDeniedException",
    ),
    ShapeType::Structure,
    &[&ACCESS_DENIED_MESSAGE],
);

/// The payload of an `AccessDeniedException`: why the caller may not pass the
/// role it named.
#[derive(Debug, Clone)]
struct AccessDeniedException {
    message: String,
}

impl SerializableStruct for AccessDeniedException {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&ACCESS_DENIED_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// An operation input of this service, as the request layer above the handlers
/// reads it.
///
/// Only one fact is asked of it: the `ExecutionRoleArn` it names, when it names
/// one. Amazon Data Lifecycle Manager assumes that role, so the two operations
/// that take one must have `iam:PassRole` for it before they run; every other
/// operation names none, which is this trait's default.
trait DlmRequest {
    /// The `ExecutionRoleArn` this request names, when it names one.
    fn execution_role_arn(&self) -> Option<&str> {
        None
    }
}

impl DlmRequest for crate::ops::types::CreateLifecyclePolicyInput {
    fn execution_role_arn(&self) -> Option<&str> {
        Some(self.execution_role_arn.as_str())
    }
}

impl DlmRequest for crate::ops::types::UpdateLifecyclePolicyInput {
    fn execution_role_arn(&self) -> Option<&str> {
        self.execution_role_arn.as_deref()
    }
}

impl DlmRequest for crate::ops::types::GetLifecyclePoliciesInput {}
impl DlmRequest for crate::ops::types::ListTagsForResourceInput {}
impl DlmRequest for crate::ops::types::TagResourceInput {}
impl DlmRequest for crate::ops::types::UntagResourceInput {}

/// The `com-amazonaws-dlm-dlm-20180112` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations that model `InvalidRequestException`, each the generated
    /// table's own operation wrapped in [`InModelledEnvelope`] and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store.
    /// Keyed by the routing key the protocol resolves, which is the key the
    /// generated table uses.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
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
        // Every operation whose model declares `InvalidRequestException`
        // (`smithy_query(query="digest")`). `GetLifecyclePolicy` and
        // `DeleteLifecyclePolicy` declare no client error, so they are not here:
        // an operation answers only the errors it models.
        wrap_operations!(
            in_modelled_envelope,
            engine,
            create_lifecycle_policy::CreateLifecyclePolicy,
            get_lifecycle_policies::GetLifecyclePolicies,
            list_tags_for_resource::ListTagsForResource,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_lifecycle_policy::UpdateLifecyclePolicy,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals that precede its handler
/// stated the way Amazon Data Lifecycle Manager states them.
///
/// Two layers, in the order the real front door runs them:
///
/// 1. `iam:PassRole` for the `ExecutionRoleArn` the request names. That is ahead
///    of the member-constraint walk, so the denial reaches the caller even for an
///    ARN the `ExecutionRoleArn` `@pattern` would otherwise refuse as malformed —
///    and, because nothing has run yet, a denied create leaves no policy behind
///    and a denied update leaves the stored policy as it was.
/// 2. The modelled input constraints, refused with this service's own
///    `InvalidRequestException`. The framework's pre-dispatch renderer gets the
///    status (400) right, but names the error type `ValidationException` — a shape
///    this model does not declare, so an SDK never deserializes it into the
///    modelled error — and writes the prose into a lowercase `message` key, while
///    this service's error shapes carry `Message` and `Code`.
///
/// The violations are the very two classes the framework's own walk collects,
/// from the framework's own deserializer and constraint walk, so this cannot find
/// a different set from the walk that follows it. Nothing else is
/// re-implemented: a request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate ->
/// handle -> serialize, telemetry and every modelled error.
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
    O::Input: DlmRequest,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition`
            // is not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            if let Some(refused) = refused_before_handling::<O>(protocol, req) {
                return refused;
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

/// This service's answer to `req`, when it is one of the two refusals that
/// precede `O`'s handler; `None` when the request reaches the handler.
///
/// `None` also when the request cannot be read at all — a body the codec rejects
/// is neither an authorization answer nor a constraint refusal, and the
/// framework's own walk words it — and when the chosen error cannot be
/// serialized, which leaves that walk to refuse the request rather than answering
/// with no reason at all.
///
/// The request is read with the framework's own deserializer (the same one the
/// walk uses) and WITHOUT the constraint walk, because `iam:PassRole` is
/// authorized ahead of the member constraints: an ARN that does not denote a
/// role the caller may pass is denied rather than refused as malformed.
///
/// A request this does not refuse is deserialized and walked a SECOND time by
/// the generated `ErasedOperation::run`. That is the cost of stating the refusal
/// in the modelled shape today: the framework's refusal seam carries only a
/// [`Refused`] code and message, which cannot hold the `Code`,
/// `RequiredParameters` and `MutuallyExclusiveParameters` members this service's
/// error declares. Should `Refused` ever grow a structured form, this layer can
/// be dropped for `override_refusal` and the second walk goes away with it.
fn refused_before_handling<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse>
where
    O::Input: DlmRequest,
{
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let violations = match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => {
            if let Some(denied) = pass_role_denied(protocol, input.execution_role_arn()) {
                return Some(denied);
            }
            constraints::validate(&input, O::INPUT_SCHEMA)
        }
        // Absence is found while READING the request and a bad value while
        // walking what was read; to a caller they are one refusal, so they are
        // one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        }],
        Err(_) => Vec::new(),
    };
    invalid_request(protocol, &violations)
}

/// This service's `InvalidRequestException` for `violations`: HTTP 400 with the
/// `Code` of the exception class, a `Message` naming the parameters it refused,
/// and `RequiredParameters` when any of them was omitted rather than invalid.
/// `None` when there is nothing to refuse.
fn invalid_request(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    if violations.is_empty() {
        return None;
    }
    let (omitted, invalid): (Vec<&Violation>, Vec<&Violation>) = violations
        .iter()
        .partition(|violation| violation.kind == ViolationKind::MissingRequiredMember);
    let omitted: Vec<String> = omitted
        .iter()
        .map(|violation| parameter_name(&violation.path))
        .collect();
    let invalid: Vec<String> = invalid.iter().copied().map(stated).collect();

    let mut clauses: Vec<String> = Vec::new();
    if !invalid.is_empty() {
        clauses.push(format!(
            "The following parameter(s) are invalid: {}",
            invalid.join(", ")
        ));
    }
    if !omitted.is_empty() {
        clauses.push(format!(
            "The following parameter(s) are required: {}",
            omitted.join(", ")
        ));
    }
    let message = clauses.join(" ");
    let refused = if omitted.is_empty() {
        crate::state::invalid_request(message)
    } else {
        crate::state::required_parameters_omitted(message, omitted)
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::shared_types::SCHEMA_INVALID_REQUEST_EXCEPTION,
            INVALID_REQUEST_EXCEPTION,
            INVALID_REQUEST_EXCEPTION_STATUS,
        )
        .ok()
}

/// One refused parameter as the message states it: the parameter, then the value
/// it held in braces.
fn stated(violation: &Violation) -> String {
    format!(
        "{} {{{}}}",
        parameter_name(&violation.path),
        violation.value.as_deref().unwrap_or_default()
    )
}

/// The parameter a validation path names, as the model spells it.
///
/// A validation path is lowerCamel (`description`, `policyDetails.schedules.1.member.name`)
/// because that is the casing AWS validation paths use; a REQUEST parameter is
/// named as the model declares the member, so each segment's first character is
/// restored and the collection step is dropped.
fn parameter_name(path: &str) -> String {
    path.split('.')
        .filter(|segment| *segment != COLLECTION_PATH_STEP)
        .map(upper_camel)
        .collect::<Vec<String>>()
        .join(".")
}

/// `segment` with its first character upper-cased, which is exactly the inverse
/// of the lowerCamel a path segment is built with.
fn upper_camel(segment: &str) -> String {
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The `AccessDeniedException` response for the `ExecutionRoleArn` a request
/// names, when it is one the caller may not pass. `None` when it may be passed,
/// and when the request names none.
fn pass_role_denied(
    protocol: &dyn ServerHttpProtocol,
    execution_role_arn: Option<&str>,
) -> Option<RtResponse> {
    let message = crate::state::pass_role_denial(execution_role_arn?)?;
    protocol
        .serialize_error(
            &AccessDeniedException { message },
            &ACCESS_DENIED_SCHEMA,
            crate::state::ACCESS_DENIED_EXCEPTION,
            crate::state::ACCESS_DENIED_STATUS,
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` for every operation that models
        // `InvalidRequestException`, so that operation's refusals are stated the
        // way this service states them and an `ExecutionRoleArn`'s `iam:PassRole`
        // is authorized ahead of the operation (and so ahead of the
        // member-constraint walk), as the real front door does it. Every other
        // key resolves to `None` here exactly as it does for an unedited service,
        // leaving the generated table to serve it.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
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
