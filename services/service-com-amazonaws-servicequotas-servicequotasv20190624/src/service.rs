// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-servicequotas-servicequotasv20190624` service: how it claims requests and how it words the
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
//! ## Why this file validates input
//!
//! Service Quotas answers a request that breaks a rule its model declares with
//! `IllegalArgumentException` (`@error("client")`, `@httpError(400)`) — a MODELLED
//! shape, whose one member is `Message`, so the body a client reads carries
//! `$.Message`. The framework's pre-dispatch envelope cannot answer with a modelled
//! shape: at that point the refusal is a protocol-level fact rather than one of this
//! service's error SHAPES, and the only seam over it ([`Service::override_refusal`])
//! chooses an error code and a sentence, not a body — so the sentence goes out under
//! the protocol's generic lowercase `message` key and `$.Message` is absent from the
//! body entirely. A client (or a human reading an SDK exception) that logs
//! `err.message` then gets an empty string and cannot tell WHICH constrained member
//! it got wrong.
//!
//! [`ModelledValidation`] closes that gap: `override_operation` routes every modelled
//! operation through it, it asks the framework's OWN boundary walk — read the request
//! into the operation's input shape, then run the model's constraint traits over what
//! was read (`aws_smithy_schema_server::constraints::validate`, the same call the
//! centralized sequence makes) — what the request violates, and renders the first
//! violation as the refusal the recordings show for the member it names. The rules
//! enforced are therefore exactly the model's; only the WORDS and the envelope are
//! this service's. A request the model accepts is delegated, untouched, to the
//! generated operation in the routing table, so each handler, its state and its 501
//! are exactly as before.
//!
//! ## Three validators, three wordings
//!
//! Service Quotas does not answer every constraint violation with one code or one
//! sentence, and which refusal a member gets is RECORDED behaviour rather than anything
//! the model states — `ValidationException` is not in this service's model at all. The
//! refusal a member gets is chosen in [`FRONT_END_VALIDATED_MEMBERS`] (the
//! request-validation front end, which answers `ValidationException` and names the
//! member as the wire spells it) and [`SERVICE_OWN_VALIDATED_MEMBERS`] (this service's
//! own application layer, which answers the modelled `IllegalArgumentException` with
//! one fixed illegal-value sentence), where the recordings behind each split are cited;
//! every other member keeps the modelled `IllegalArgumentException` with the
//! member-and-rule clause. The message FORM of each recording is what tells the three
//! apart.
//! A refusal a HANDLER makes after reading a request the model accepts — a quota that
//! is not adjustable, a desired value that is not an increase — is a semantic refusal
//! and stays the modelled `IllegalArgumentException` wherever it is raised.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

/// The wire error code (`__type`) of this service's input-validation refusal: the
/// shape the model lists on every operation for "Invalid input was provided", and the
/// same name a handler-returned `IllegalArgumentException` carries.
const ILLEGAL_ARGUMENT_CODE: &str = "IllegalArgumentException";

/// The wire error code (`__type`) of a refusal this service's request-validation FRONT
/// END makes, for the members it owns (see [`FRONT_END_VALIDATED_MEMBERS`]). The shape
/// is NOT in this service's model, which is why it is rendered through the synthetic
/// [`crate::scaffolded::ValidationException`] rather than through an operation's error
/// enum.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The status an input-validation refusal is served on: `IllegalArgumentException`'s
/// own `@httpError(400)`, which is also the status this awsJson front end puts on any
/// client fault (see [`crate::scaffolded::wire_http_status`]) and the status the
/// request validator answers a malformed member with.
const REFUSAL_STATUS: u16 = 400;

/// The input members Service Quotas' request-validation FRONT END owns, rather than its
/// own application layer.
///
/// Which error code a constraint violation carries is recorded behaviour, and two
/// recordings fix the split for this service. The FORM of each recorded message is what
/// tells the two validators apart:
///
/// * a `RequestId` outside its `@pattern` is refused with [`VALIDATION_EXCEPTION_CODE`]
///   and a message in the request validator's form — `Value at '<member path>' failed
///   to satisfy constraint: …`, naming the member as the WIRE spells it (recorded for
///   `GetRequestedServiceQuotaChange`);
/// * a `QuotaCode` or `ServiceCode` outside theirs is refused with the MODELLED
///   [`ILLEGAL_ARGUMENT_CODE`] and a message in the service's own form — `QuotaCode
///   failed to satisfy constraint: …`, with no `Value at` clause and no
///   `N validation error(s) detected` envelope (recorded for `GetServiceQuota`, and
///   pinned by the resolved issue
///   `20261022T115709Z-servicequotas-service-quotas-error-paths-answer-404-instead-of`,
///   cause 5).
///
/// Listed by the member name the MODEL spells, so the split follows that member
/// wherever this service declares it — `GetRequestedServiceQuotaChange` and
/// `CreateSupportCase` both take a `RequestId` — instead of being keyed to one
/// operation or to one recorded request.
const FRONT_END_VALIDATED_MEMBERS: &[&str] = &["RequestId"];

/// The clause joining the member a caller got wrong to the rule it broke, as AWS
/// words it: `QuotaCode failed to satisfy constraint: Member must satisfy regular
/// expression pattern: …`. Shared by the message builder and the reader that lifts
/// the requirement half out of the framework's own sentence, so the two cannot drift.
const CONSTRAINT_CLAUSE: &str = " failed to satisfy constraint: ";

/// The input members Service Quotas checks in its OWN application layer, which states
/// one sentence for an illegal value ([`ILLEGAL_VALUE_MESSAGE`]) and names
/// neither the member nor the rule — the third validator in the split above.
///
/// RECORDED behaviour (the corpus unit
/// `service-quotas.list-tags-for-resource-invalid-arn-pattern-rejected`): a tagging
/// call carrying a `ResourceARN` outside its constraints is answered HTTP 400
/// `IllegalArgumentException` with that sentence rather than with the member-and-rule
/// clause every other modelled member is refused by.
///
/// Listed the way [`FRONT_END_VALIDATED_MEMBERS`] is — by the member name the MODEL
/// spells — so the split follows that member wherever this service declares it
/// (`ListTagsForResource`, `TagResource` and `UntagResource` all take a `ResourceARN`)
/// instead of being keyed to one operation or to one recorded request. Matched by
/// member NAME because that is the model fact a violation gives access to: a member
/// `Schema` carries its own name, its index and the TYPE of its target, not the
/// target's shape id, so the `AmazonResourceName` these members target is not
/// something the walk's output can be compared against.
const SERVICE_OWN_VALIDATED_MEMBERS: &[&str] = &["ResourceARN"];

/// How Service Quotas words the refusal of a value its own application layer will not
/// accept — one sentence for an illegal value, naming neither the member nor the rule.
/// Recorded for the resource ARN; it covers every member in
/// [`SERVICE_OWN_VALIDATED_MEMBERS`].
///
/// Reached from two places, because that layer checks more than the model's constraint
/// traits: the boundary walk below, for a value outside the constraints the model
/// declares, and the handlers that read the same member themselves (the tagging
/// operations, whose `ResourceARN` has to be an ARN of one of this service's own
/// resources). Which validator answers is a property of the MEMBER, so both state this
/// one sentence rather than letting the VALUE decide the wording.
pub const ILLEGAL_VALUE_MESSAGE: &str =
    "The request failed because you provided an illegal value for a parameter.";

/// The `com-amazonaws-servicequotas-servicequotasv20190624` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ModelledValidation`] per modelled operation, keyed by the routing key the
    /// model names that operation by ([`Operation::NAME`], the same key the generated
    /// routing table uses). Built once here, never per request; each entry is
    /// zero-sized, so this is a table of names, not a second set of operations.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            refusals: modelled_validation_table::<Eng>(),
        }
    }
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// A constraint is a property of the INPUT SHAPE and every operation of this service
/// declares the same `IllegalArgumentException`, so the wrapper goes in front of all
/// of them — implemented or still scaffolded. A stub answers 501 for a request it
/// could serve and the modelled 400 for one no operation could, which is the order
/// the real service evaluates them in.
fn modelled_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                refusals.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(ModelledValidation::<crate::ops::$module::$operation<Eng>>::new())
                        as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    register!(
        associate_service_quota_template::AssociateServiceQuotaTemplate,
        create_support_case::CreateSupportCase,
        delete_service_quota_increase_request_from_template::DeleteServiceQuotaIncreaseRequestFromTemplate,
        disassociate_service_quota_template::DisassociateServiceQuotaTemplate,
        get_association_for_service_quota_template::GetAssociationForServiceQuotaTemplate,
        get_auto_management_configuration::GetAutoManagementConfiguration,
        get_aws_default_service_quota::GetAWSDefaultServiceQuota,
        get_quota_utilization_report::GetQuotaUtilizationReport,
        get_requested_service_quota_change::GetRequestedServiceQuotaChange,
        get_service_quota::GetServiceQuota,
        get_service_quota_increase_request_from_template::GetServiceQuotaIncreaseRequestFromTemplate,
        list_aws_default_service_quotas::ListAWSDefaultServiceQuotas,
        list_requested_service_quota_change_history::ListRequestedServiceQuotaChangeHistory,
        list_requested_service_quota_change_history_by_quota::ListRequestedServiceQuotaChangeHistoryByQuota,
        list_service_quota_increase_requests_in_template::ListServiceQuotaIncreaseRequestsInTemplate,
        list_service_quotas::ListServiceQuotas,
        list_services::ListServices,
        list_tags_for_resource::ListTagsForResource,
        put_service_quota_increase_request_into_template::PutServiceQuotaIncreaseRequestIntoTemplate,
        request_service_quota_increase::RequestServiceQuotaIncrease,
        start_auto_management::StartAutoManagement,
        start_quota_utilization_report::StartQuotaUtilizationReport,
        stop_auto_management::StopAutoManagement,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_auto_management::UpdateAutoManagement,
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
        // Serve every modelled operation through its [`ModelledValidation`] wrapper, so
        // a request refused before the handler runs is stated in this service's OWN
        // modelled `IllegalArgumentException` — whose `Message` member is what puts
        // `$.Message` in the body — instead of the framework's generic envelope. The
        // wrapper delegates a request the model accepts to the generated operation in
        // the routing table, which stays the only thing that serves it.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Service Quotas has its OWN names for these two refusals, and they are the
        // ones its model declares — it declares no `ValidationException`, which is
        // what the framework would otherwise answer:
        //
        // * `IllegalArgumentException` (HTTP 400, "Invalid input was provided") is
        //   listed on every operation in the model and is the code a Service Quotas
        //   client branches on for a malformed or absent input member — a
        //   `ServiceCode` outside `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`, a `MaxResults`
        //   outside 1..100, an absent required member;
        // * `AccessDeniedException` (HTTP 403, "You do not have sufficient
        //   permission to perform this action") is the model's refusal of a caller.
        //
        // The framework keeps the status and the envelope; only the code and the
        // message are chosen here, and the message is the framework's own clause,
        // which names the member and the constraint it failed.
        //
        // This is the FALLBACK path now: a refusal of a request that named a modelled
        // operation is answered by [`ModelledValidation`] above, in the modelled
        // shape. What still arrives here is a refusal with no modelled operation to
        // read the request against — an unparseable body, or a caller the emulator
        // would not authenticate.
        //
        // A violation naming a member the request validator owns
        // ([`FRONT_END_VALIDATED_MEMBERS`]) keeps that validator's code and clause
        // here too, so one member is not refused under two codes depending on which
        // path answered it. The member is placed WITHOUT a schema on this path, by
        // lowerCamel-ing each listed name onto the violation's own path, which is the
        // spelling the framework's walk reports.
        match refusal {
            Refusal::Invalid(violations) if violations.first().is_some_and(front_end_owned) => {
                let first = violations.first()?;
                Some(Refused {
                    code: VALIDATION_EXCEPTION_CODE.to_string(),
                    message: front_end_message(first),
                })
            }
            Refusal::Invalid(violations) => Some(Refused {
                code: ILLEGAL_ARGUMENT_CODE.to_string(),
                message: violations
                    .iter()
                    .map(smithy_server_runtime::Violation::message)
                    .collect::<Vec<_>>()
                    .join("; "),
            }),
            Refusal::Unauthenticated => Some(Refused {
                code: "AccessDeniedException".to_string(),
                message: "You do not have sufficient permission to perform this action"
                    .to_string(),
            }),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has no name of
            // its own for keeps the framework's wording.
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

/// One modelled operation, wrapped so a request this service refuses BEFORE the handler
/// runs is answered in its own modelled `IllegalArgumentException` rather than the
/// framework's generic envelope.
///
/// Zero-sized: the wrapper holds no engine and no operation. It carries the operation
/// TYPE, which is all it needs — the input shape to read the request into and the input
/// `Schema` to validate it against — and delegates a request the model accepts to the
/// one operation instance the generated routing table built from the shared engine.
/// There is therefore no second instance of any operation and no second store.
struct ModelledValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModelledValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let protocol = svc.definition().protocol();
            if let Some(refusal) = modelled_refusal::<Op>(protocol, req) {
                // Serialized through the service's own protocol, exactly as a
                // handler-returned error is: the same body member name, the same
                // `__type`, and the status this front end serves a client fault on.
                match refusal.serialize(protocol) {
                    Ok(response) => {
                        record_refusal::<Op>(ctx, refusal.code(), started.elapsed());
                        return response;
                    }
                    // An error that will not serialize leaves the request to the
                    // framework, which refuses it in the generic envelope — the very
                    // divergence this wrapper removes, so it is logged rather than
                    // passed over silently.
                    Err(error) => tracing::warn!(
                        operation = Op::NAME,
                        code = refusal.code(),
                        %error,
                        "this service's constraint refusal would not serialize; it falls \
                         back to the framework's generic envelope"
                    ),
                }
            }
            // Nothing to reword: the request is served by the generated operation,
            // which runs the whole framework sequence (deserialize, validate, handle,
            // serialize) over the same bytes.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as
/// the framework gates it.
fn record_refusal<Op: Operation>(
    ctx: &OperationContext<'_>,
    code: &str,
    latency: std::time::Duration,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            REFUSAL_STATUS,
            code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

/// One refusal of a request that broke a rule this service's model declares, in the
/// error shape the recordings show for the MEMBER it names.
///
/// Both arms carry the same status and the same two halves of a sentence (the member and
/// the requirement); they differ in the error code a client branches on, in how the
/// member is spelled, and in whether the shape is one the model declares — see
/// [`FRONT_END_VALIDATED_MEMBERS`].
enum ConstraintRefusal {
    /// The request-validation front end's answer, in the synthetic
    /// [`crate::scaffolded::ValidationException`]: a shape this service's model does not
    /// declare, so it cannot be an arm of any operation's error enum.
    FrontEnd(crate::scaffolded::ValidationException),
    /// The service's own answer, in the MODELLED `IllegalArgumentException` — whose
    /// `Message` member is what puts `$.Message` in the body.
    Modelled(crate::ops::types::IllegalArgumentException),
}

impl ConstraintRefusal {
    /// The refusal `violation` gets: the front end's for a member it owns, this
    /// service's own for every other.
    fn of(violation: &Violation, input_schema: &Schema) -> Self {
        match modelled_member_name(&violation.path, input_schema) {
            Some(member) if FRONT_END_VALIDATED_MEMBERS.contains(&member.as_str()) => {
                Self::FrontEnd(crate::scaffolded::ValidationException::new(
                    front_end_message(violation),
                ))
            }
            _ => Self::Modelled(illegal_argument(refusal_message(violation, input_schema))),
        }
    }

    /// The wire error code (`__type`) this refusal carries.
    fn code(&self) -> &'static str {
        match self {
            Self::FrontEnd(_) => VALIDATION_EXCEPTION_CODE,
            Self::Modelled(_) => ILLEGAL_ARGUMENT_CODE,
        }
    }

    /// Render this refusal through the service's own protocol, so its body carries the
    /// member name, the `__type` and the status the recordings show.
    fn serialize(
        &self,
        protocol: &dyn ServerHttpProtocol,
    ) -> Result<aws_smithy_runtime_api::http::Response, aws_smithy_schema_server::ProtocolError>
    {
        match self {
            Self::FrontEnd(exception) => protocol.serialize_error(
                exception,
                &crate::scaffolded::VALIDATION_EXCEPTION_SCHEMA,
                VALIDATION_EXCEPTION_CODE,
                REFUSAL_STATUS,
            ),
            Self::Modelled(exception) => protocol.serialize_error(
                exception,
                &crate::ops::types::SCHEMA_ILLEGAL_ARGUMENT_EXCEPTION,
                ILLEGAL_ARGUMENT_CODE,
                REFUSAL_STATUS,
            ),
        }
    }
}

/// How this service states the request `Op` cannot be read from, or `None` when it has
/// nothing of its own to say — in which case the request goes on to the generated
/// operation and the framework answers as it always did.
///
/// This is the boundary walk the framework runs, in the framework's order and with the
/// framework's own code: read the request into the operation's input shape, then run
/// constraint-trait validation over what was read. Only the WORDS and the envelope are
/// this service's.
fn modelled_refusal<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<ConstraintRefusal> {
    let input = {
        let mut deserializer = protocol.deserialize_input(req, Op::INPUT_SCHEMA).ok()?;
        match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
            Ok(input) => input,
            // A member the shape requires is absent. It is found while READING the
            // request rather than by walking what was read, but to a caller it is the
            // same refusal — so it is stated the same way, naming the member and the
            // rule it broke.
            Err(aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
                let absent = Violation {
                    // The validating walk spells a member's path in lowerCamel; the
                    // codec hands over the Smithy name, so it is cased here to the one
                    // spelling the message builder below resolves against the model.
                    path: constraints::path::member(&member_name),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    // There is no value to withhold: the member was absent, so the
                    // clause names the path alone whether or not the model marks it
                    // `@sensitive`.
                    sensitive: false,
                };
                return Some(ConstraintRefusal::of(&absent, Op::INPUT_SCHEMA));
            }
            // Any other codec failure is not a refusal this service rewords.
            Err(_) => return None,
        }
    };
    let violations = constraints::validate(&input, Op::INPUT_SCHEMA);
    // AWS names ONE member per refusal, and the walk visits members in schema order,
    // so "the first" is a property of the input shape rather than of the request.
    let first = violations.first()?;
    Some(ConstraintRefusal::of(first, Op::INPUT_SCHEMA))
}

/// Whether `violation` names a member the request-validation front end owns, placed
/// without an input schema: each listed member name is lowerCamel-ed onto the path the
/// framework's walk reports, so the model is still the source of the spelling.
fn front_end_owned(violation: &Violation) -> bool {
    FRONT_END_VALIDATED_MEMBERS
        .iter()
        .any(|member| constraints::path::member(member) == violation.path)
}

/// Whether `violation` is an illegal VALUE of a member this service checks in its own
/// application layer.
///
/// The member is identified the way [`ConstraintRefusal::of`] identifies the front
/// end's — through [`modelled_member_name`], so the model's own spelling is what the
/// list is matched against — and one more layer does not mean one more matching style.
/// A `MissingRequiredMember` violation is not one: there was no value for that layer to
/// check, so an absent member keeps the member-and-rule clause.
fn service_own_owned(violation: &Violation, input_schema: &Schema) -> bool {
    !matches!(violation.kind, ViolationKind::MissingRequiredMember)
        && modelled_member_name(&violation.path, input_schema)
            .is_some_and(|member| SERVICE_OWN_VALIDATED_MEMBERS.contains(&member.as_str()))
}

/// How the request-validation front end words a value that broke a rule the model
/// declares: the member path as the WIRE spells it (the validating walk's lowerCamel,
/// which is the spelling the recorded message carries) and the rule it broke —
/// `Value at 'requestId' failed to satisfy constraint: Member must satisfy regular
/// expression pattern: …`.
///
/// The rejected value is NOT quoted back: a refusal states the rule, and echoing input
/// into a 400 body puts caller-supplied bytes in every client log. The one value a
/// clause does report is the `null` of a member the request omits, which is a fact of
/// the request's SHAPE rather than something the caller sent.
///
/// A violation this cannot render a requirement for keeps the framework's own sentence,
/// which already names both the path and the rule.
fn front_end_message(violation: &Violation) -> String {
    let Some(requirement) = requirement(violation) else {
        return violation.message();
    };
    let subject = if matches!(violation.kind, ViolationKind::MissingRequiredMember) {
        "Value null"
    } else {
        "Value"
    };
    format!(
        "{subject} at '{}'{CONSTRAINT_CLAUSE}{requirement}",
        violation.path
    )
}

/// This service's modelled refusal carrying `message` — the one place its
/// `IllegalArgumentException` is built from a sentence.
fn illegal_argument(message: String) -> crate::ops::types::IllegalArgumentException {
    crate::ops::types::IllegalArgumentException {
        message: Some(message),
    }
}

/// How this service words a value that broke a rule its model declares.
///
/// AWS builds this `Message` from the request: it names the MEMBER the caller got wrong
/// and the constraint that member failed — `QuotaCode failed to satisfy constraint:
/// Member must satisfy regular expression pattern: …`. Both halves come from the model:
/// the member is spelled as the MODEL spells it (`QuotaCode`, not the validating walk's
/// lowerCamel `quotaCode`, which is a convention of the framework's envelope rather
/// than a fact of the model), and the requirement is [`requirement`]'s rendering of the
/// constraint the walk reported.
///
/// A violation whose member this cannot name — a path into a nested shape — keeps the
/// framework's own sentence, which already names both the path and the rule, rather
/// than being reworded into half of one.
///
/// The ONE group of members this does not hold for is the one this service checks in
/// its own application layer, whose illegal VALUES it refuses in its own fixed
/// sentence; see [`SERVICE_OWN_VALIDATED_MEMBERS`].
fn refusal_message(violation: &Violation, input_schema: &Schema) -> String {
    // A member this service checks itself is refused in its own words, for every
    // constraint that member carries — a pattern or either length bound — because what
    // the sentence states is that the VALUE is not a legal one rather than which rule
    // it broke.
    if service_own_owned(violation, input_schema) {
        return ILLEGAL_VALUE_MESSAGE.to_string();
    }
    match (
        modelled_member_name(&violation.path, input_schema),
        requirement(violation),
    ) {
        (Some(member), Some(requirement)) => {
            format!("{member}{CONSTRAINT_CLAUSE}{requirement}")
        }
        _ => violation.message(),
    }
}

/// The rule half of the clause: what the member had to satisfy.
///
/// Every constraint but `@pattern` keeps the framework's own rendering, lifted out of
/// its sentence, so this file carries no second copy of those words. A `@pattern` is
/// rendered here because the model anchors every pattern it declares (`^…$`) — Smithy's
/// spelling of the full-string match the service applies — while the service states the
/// bare expression it was declared from.
fn requirement(violation: &Violation) -> Option<String> {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => {
            let bare = pattern.strip_prefix('^').unwrap_or(pattern);
            let bare = bare.strip_suffix('$').unwrap_or(bare);
            Some(format!(
                "Member must satisfy regular expression pattern: {bare}"
            ))
        }
        _ => violation
            .message()
            .split_once(CONSTRAINT_CLAUSE)
            .map(|(_, requirement)| requirement.to_string()),
    }
}

/// The input member a violation path starts at, named as the model names it.
///
/// Matched by lowerCamel-ing each candidate member rather than by un-casing the path,
/// so the model is the source of the spelling. `None` for a path into a nested shape,
/// which no top-level member lookup can name.
fn modelled_member_name(path: &str, input_schema: &Schema) -> Option<String> {
    if path.contains('.') {
        return None;
    }
    input_schema
        .members()
        .iter()
        .copied()
        .find_map(|member| match member.member_name() {
            Some(name) if constraints::path::member(name) == path => Some(name.to_string()),
            _ => None,
        })
}
