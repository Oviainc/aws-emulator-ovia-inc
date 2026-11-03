// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-synthetics-synthetics` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it: it puts the input-validation gate described
//!   below in front of every modelled operation and lets the generated routing table
//!   serve the request from there.
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
//! Synthetics models `ValidationException` with one member, `Message` (target
//! `ErrorMessage`), and AWS populates it on every 400 it raises at the request boundary:
//! an SDK and the CLI surface the failure through exactly that member. The framework's
//! pre-dispatch envelope cannot set it — it renders a refusal as a generic
//! `{"__type":"ValidationException","message":…}`, because at that point the refusal is
//! a protocol-level fact rather than one of this service's error SHAPES, and the only
//! seam over it ([`Service::override_refusal`]) chooses a code and a sentence, not a
//! body — so a client deserialising the modelled shape finds no `Message` at all.
//!
//! So this service answers a constraint refusal from its own modelled shape, through
//! [`ModeledInputValidation`]: a gate [`Service::override_operation`] puts in front of
//! every modelled operation, which asks the framework's OWN constraint walk
//! (`aws_smithy_schema_server::validate`, the same call the centralized sequence makes)
//! what the request violates and renders those violations with the framework's own
//! `envelope` — the `N validation error(s) detected: Value '…' at '…' failed to satisfy
//! constraint: …` sentence AWS itself returns — into the modelled `Message`. Any request
//! the model accepts is handed straight to the generated operation. The rules enforced
//! are therefore exactly the model's — nothing about a constraint is restated here — and
//! only the BODY of the refusal changes.
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the
//! per-operation telemetry the centralized sequence records — a request the gate
//! refuses never enters that sequence, so it is counted nowhere, exactly as a request
//! refused before dispatch is.
//!
//! ## Nothing about a constraint is restated
//!
//! The refusal body is the framework's own rendering of the framework's own violations:
//! the member path, the submitted value and the violated requirement exactly as the
//! constraint walk produced them, for every constrained member of every operation.
//!
//! A `@pattern` requirement is the one clause whose expression is not always the trait
//! string character for character, because the recorded service words it in TWO dialects
//! and the model says which one an operation takes. Nineteen of the twenty-two operations
//! declare `ValidationException` in their own error set, and their refusals quote the
//! expression AS THE MODEL DECLARES IT, the whole-value `^` / `$` pair included. The three
//! tagging operations — `TagResource`, `UntagResource`, `ListTagsForResource` — declare a
//! different error family entirely (`BadRequestException`, `ConflictException`,
//! `InternalFailureException`, `NotFoundException`, `TooManyRequestsException`) and no
//! `ValidationException` at all, yet a constraint failure on one of them is recorded
//! answering `ValidationException`: the refusal comes from the generic request validator
//! ahead of the operation rather than from the operation's own contract, and that validator
//! quotes the expression it PUBLISHES — the declared one with the model's whole-value pair
//! dropped, since a Smithy `@pattern` is a find and that pair is the model's wrapping
//! rather than part of the grammar a value has to satisfy.
//!
//! So [`PatternWording`] is read off the operation's modelled error set, not off a member
//! or a shape, and the drop itself is [`service_common::validation::unanchored`], this
//! tree's one reading of that rule, called rather than restated. Nothing else about the
//! clause changes, and nothing about ENFORCEMENT changes in either dialect: the walk always
//! evaluates the declared, anchored expression, so the set of values this service accepts is
//! exactly the model's.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{envelope, validate, ProtocolError, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

/// `ValidationException`'s modelled status (`httpError` 400 on
/// `com.amazonaws.synthetics#ValidationException`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// `ValidationException`'s wire error code — the shape name, which restJson1 puts in
/// `x-amzn-errortype` and in the body's `__type`.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The `com-amazonaws-synthetics-synthetics` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Gate>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            input_validation: input_validation_gates::<Eng>(),
        }
    }
}

/// Which of the two recorded dialects an operation words a `@pattern` requirement in.
///
/// The dialect is a property of the OPERATION — read off its modelled error set, see
/// [`input_validation_gates`] — and not of the member, its binding, or the shape whose
/// pattern failed: two operations are quoted differently for the same ARN form bound the
/// same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternWording {
    /// The expression as the model DECLARES it, character for character, the whole-value
    /// `^` / `$` pair included.
    AsDeclared,
    /// The expression the generic request validator PUBLISHES: the declared one with the
    /// model's whole-value pair dropped.
    AsPublished,
}

/// One operation's entry in the gate table: the dialect that operation words a `@pattern`
/// requirement in, and the gate itself.
///
/// A pair rather than a named struct because nothing reads both at once — the dialect is
/// what `the_pattern_wording_of_each_operation_is_the_models_own` asks the table for by
/// operation name, since the erased gate cannot hand it back, and the gate is what
/// [`Service::override_operation`] hands the router.
type Gate = (PatternWording, Box<dyn ErasedOperation>);

/// One [`ModeledInputValidation`] gate per modelled operation, with the `@pattern` dialect
/// that operation is recorded wording a constraint refusal in.
///
/// Each gate carries the operation TYPE, which is what names the input shape and its
/// schema, plus that one dialect. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
///
/// THE SPLIT BELOW IS THE MODEL'S: an operation that declares `ValidationException` in its
/// own error set is refused by this service's own validator and quotes the expression as
/// declared; one that does not — the three tagging operations, which declare the
/// `BadRequestException` family instead and are still recorded answering
/// `ValidationException` for a constraint failure — is refused by the generic request
/// validator ahead of it and quotes the expression that validator publishes. The operation
/// names are written out because naming the TYPE is the only way to reach its input schema
/// from Rust; which side each lands on is pinned against the shipped model by
/// `the_pattern_wording_of_each_operation_is_the_models_own`, so an operation added to the
/// model cannot take the wrong dialect silently.
fn input_validation_gates<Eng: Send + Sync + 'static>() -> HashMap<&'static str, Gate> {
    let mut gates: HashMap<&'static str, Gate> = HashMap::new();
    macro_rules! gate {
        ($map:ident, $wording:expr; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    (
                        $wording,
                        Box::new(
                            ModeledInputValidation::<crate::ops::$module::$operation<Eng>>::new(
                                $wording,
                            ),
                        ) as Box<dyn ErasedOperation>,
                    ),
                );
            )+
        };
    }
    // Every operation the model binds. A constraint is a property of the INPUT SHAPE,
    // so the gate goes in front of all of them, implemented or still scaffolded: a
    // request that violates the model is refused the same way whoever serves it.
    //
    // The operations that declare `ValidationException` themselves.
    gate!(
        gates, PatternWording::AsDeclared;
        associate_resource::AssociateResource,
        create_canary::CreateCanary,
        create_group::CreateGroup,
        delete_canary::DeleteCanary,
        delete_group::DeleteGroup,
        describe_canaries::DescribeCanaries,
        describe_canaries_last_run::DescribeCanariesLastRun,
        describe_runtime_versions::DescribeRuntimeVersions,
        disassociate_resource::DisassociateResource,
        get_canary::GetCanary,
        get_canary_runs::GetCanaryRuns,
        get_group::GetGroup,
        list_associated_groups::ListAssociatedGroups,
        list_group_resources::ListGroupResources,
        list_groups::ListGroups,
        start_canary::StartCanary,
        start_canary_dry_run::StartCanaryDryRun,
        stop_canary::StopCanary,
        update_canary::UpdateCanary,
    );
    // And the tagging operations, whose error set declares no `ValidationException`.
    gate!(
        gates, PatternWording::AsPublished;
        list_tags_for_resource::ListTagsForResource,
        tag_resource::TagResource,
        untag_resource::UntagResource,
    );
    gates
}

/// The modelled-`ValidationException` gate in front of one operation `Op`.
///
/// `Op` is a type parameter only, naming the input shape and the schema the constraint walk
/// runs against; the only datum carried is the dialect this operation words a `@pattern`
/// requirement in. It holds no engine and no state, so the operation it guards stays the
/// single owner of the request's behaviour.
struct ModeledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
    wording: PatternWording,
}

impl<Op> ModeledInputValidation<Op> {
    const fn new(wording: PatternWording) -> Self {
        Self {
            operation: PhantomData,
            wording,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModeledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req, self.wording) {
                return refused;
            }
            // Valid: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// `Some(response)` when `Op`'s input violates a rule the model declares: this service's
/// modelled `ValidationException`, 400, with its `Message` member carrying the
/// constraint-violation envelope.
///
/// `None` when the request satisfies the model, or when it failed to READ for a reason
/// that is not a constraint violation — an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
    wording: PatternWording,
) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violations: Vec<Violation> = model_violations::<Op>(protocol, req)?
        .into_iter()
        .map(|violation| reported(violation, wording))
        .collect();
    // The envelope AWS itself returns for a constraint failure, built from the member
    // path, the offending value and the violated constraint by the framework's own
    // renderer — the same text the generic 400 carried, now in the modelled member. No
    // requirement is restated here: the only thing `reported` changes is which of the two
    // recorded dialects a `@pattern` expression is quoted in.
    let error = crate::ops::types::ValidationException {
        message: envelope(&violations),
    };
    protocol
        .serialize_error(
            &error,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A modelled error that will not serialize leaves the request to the framework,
        // which refuses it in the generic envelope — the very divergence this gate
        // removes, so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                operation = Op::NAME,
                %error,
                "the modelled ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// `violation` as the operation this gate guards reports it.
///
/// Every kind but `@pattern` is reported exactly as the constraint walk produced it,
/// whatever the dialect. A `@pattern` violation names either the declared expression or the
/// one the generic validator publishes, per `wording`; the drop itself, and the reading of
/// an escaped trailing `$`, belong to [`service_common::validation::unanchored`], this
/// tree's one home for them, so they are called here and not restated.
fn reported(mut violation: Violation, wording: PatternWording) -> Violation {
    match wording {
        PatternWording::AsDeclared => violation,
        PatternWording::AsPublished => {
            if let ViolationKind::Pattern(declared) = &violation.kind {
                violation.kind =
                    ViolationKind::Pattern(service_common::validation::unanchored(declared));
            }
            violation
        }
    }
}

/// Every rule of the model `Op`'s input breaks, or `None` when it breaks none.
///
/// Both refusal classes the framework describes structurally are collected: a required
/// member absent (found while reading the request) and a value outside a declared
/// constraint (found while walking the value that was read).
fn model_violations<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Vec<Violation>> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => {
                    let violations = validate(&input, Op::INPUT_SCHEMA);
                    if violations.is_empty() {
                        return None;
                    }
                    ProtocolError::Invalid(violations)
                }
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error)? {
        Refusal::Invalid(violations) => Some(violations),
        // Authentication is settled by the router before any operation runs, so an
        // `Unauthenticated` refusal cannot arrive here — and it is not a constraint
        // violation in any case.
        _ => None,
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
        // Put this service's input-validation gate in front of the modelled operation
        // the request names, so a value outside a constraint the model declares — or a
        // required member left out — is refused with the MODELLED `ValidationException`,
        // whose `Message` member carries the constraint envelope, instead of the
        // framework's generic 400 body. The gate then hands every request the model
        // accepts to the generated operation, which the routing table still owns; a
        // request that names no modelled operation resolves as it always did.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let (_, gate) = self.input_validation.get(key)?;
        Some(gate.as_ref())
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

#[cfg(test)]
mod tests {
    use aws_smithy_schema_server::{Violation, ViolationKind};

    use super::PatternWording;

    /// The `@pattern` expression each dialect names, over the forms a declared expression
    /// takes: with the model's whole-value pair, with a dollar the expression escapes before
    /// it, with only one end anchored, and with neither. The DROP itself is
    /// [`service_common::validation::unanchored`]'s and has its own table beside it; these
    /// rows pin the WIRING — that `AsPublished` reaches it and `AsDeclared` leaves every one
    /// of the same expressions exactly as the model spells it.
    #[test]
    fn each_dialect_names_the_expression_it_is_recorded_naming() {
        let cases: [(&str, &str, &str); 4] = [
            (
                "the whole-value pair",
                r"^[0-9a-z_\-]+$",
                r"[0-9a-z_\-]+",
            ),
            // An escaped dollar is a literal dollar the value has to contain, so the pair
            // is the one that follows it.
            ("an escaped dollar, then the pair", r"^price\$$", r"price\$"),
            // One end only is a different expression, not an anchored spelling of this one,
            // so the shared rule leaves it alone in BOTH dialects.
            ("anchored at the start only", r"^arn:aws:", r"^arn:aws:"),
            ("anchored at neither end", r"[a-z]+", r"[a-z]+"),
        ];
        for (case, declared, published) in cases {
            for (wording, expected) in [
                (PatternWording::AsDeclared, declared),
                (PatternWording::AsPublished, published),
            ] {
                let reported = super::reported(
                    Violation {
                        path: "name".to_string(),
                        kind: ViolationKind::Pattern(declared.to_string()),
                        value: Some("whatever".to_string()),
                        sensitive: false,
                    },
                    wording,
                );
                assert_eq!(
                    reported.kind,
                    ViolationKind::Pattern(expected.to_string()),
                    "case {case}/{wording:?}: expected the reported pattern {expected:?}, \
                     got {:?}",
                    reported.kind
                );
            }
        }
    }

    /// A violation of any other kind is reported exactly as the constraint walk produced
    /// it, in both dialects: only a `@pattern` clause names an expression at all.
    #[test]
    fn a_violation_of_another_kind_is_reported_unchanged() {
        let kinds = [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(255),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("1000".to_string()),
            ViolationKind::EnumValue(vec!["RUNNING".to_string()]),
            ViolationKind::UniqueItems,
        ];
        for kind in kinds {
            let violation = Violation {
                path: "name".to_string(),
                kind: kind.clone(),
                value: Some("whatever".to_string()),
                sensitive: false,
            };
            for wording in [PatternWording::AsDeclared, PatternWording::AsPublished] {
                let reported = super::reported(violation.clone(), wording);
                assert_eq!(
                    reported, violation,
                    "case {kind:?}/{wording:?}: expected the violation to be reported \
                     unchanged, got {reported:?}"
                );
            }
        }
    }

    /// Each operation's `@pattern` dialect is the one its own modelled error set decides.
    ///
    /// The split is written out by name in `input_validation_gates`, because naming the
    /// operation TYPE is the only way to reach its input schema from Rust. This is what
    /// keeps that list honest: an operation whose error set declares `ValidationException`
    /// is refused by this service's own validator and quotes the expression as declared, and
    /// one that does not is refused by the generic request validator ahead of it and quotes
    /// the expression that validator publishes \u2014 so an operation added to the model, or one
    /// whose error set changes, cannot keep the wrong dialect silently.
    #[test]
    fn the_pattern_wording_of_each_operation_is_the_models_own() {
        let shapes = shipped_model();
        let gates = super::input_validation_gates::<engine::InMemoryEngine>();
        let mut checked = 0;
        for (id, shape) in &shapes {
            if shape.get("type").is_none_or(|kind| kind != "operation") {
                continue;
            }
            let operation = id.rsplit('#').next().unwrap_or_default();
            let declares_validation_exception = shape
                .get("errors")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|errors| {
                    errors.iter().any(|error| {
                        error.get("target").and_then(serde_json::Value::as_str)
                            == Some("com.amazonaws.synthetics#ValidationException")
                    })
                });
            let expected = if declares_validation_exception {
                PatternWording::AsDeclared
            } else {
                PatternWording::AsPublished
            };
            let gate = gates
                .get(operation)
                .unwrap_or_else(|| panic!("operation {operation} has no input-validation gate"));
            assert_eq!(
                gate.0, expected,
                "case {operation}: its modelled error set {} ValidationException, so the \
                 expected dialect is {expected:?}, got {:?}",
                if declares_validation_exception {
                    "declares"
                } else {
                    "does not declare"
                },
                gate.0
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "the model declares no operations, so this test proves nothing"
        );
    }

    /// The `shapes` object of the Smithy model this crate ships.
    fn shipped_model() -> serde_json::Map<String, serde_json::Value> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/smithy-model/com-amazonaws-synthetics-synthetics.json"
        );
        let text = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
        let model: serde_json::Value =
            serde_json::from_str(&text).expect("the Smithy model is JSON");
        model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes")
            .clone()
    }

    /// Every operation the model declares has an input-validation gate.
    ///
    /// The gate set is written out by name in `input_validation_gates` — the operation
    /// TYPE is what names the input shape and its schema, and the erased routing table
    /// cannot hand that back — so this is what keeps the two in step: an operation added
    /// to the model (and so to `src/ops/`) without a gate would quietly go back to
    /// answering a constraint refusal in the framework's generic envelope, and this test
    /// fails naming it.
    #[test]
    fn every_modeled_operation_has_an_input_validation_gate() {
        let shapes = shipped_model();
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape.get("type").is_some_and(|kind| kind == "operation"))
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            !operations.is_empty(),
            "the model declares no operations, so this test proves nothing"
        );
        let gates = super::input_validation_gates::<engine::InMemoryEngine>();
        for operation in &operations {
            assert!(
                gates.contains_key(*operation),
                "operation {operation} has no input-validation gate, so a request that \
                 violates its model would be refused without the modelled Message"
            );
        }
        assert_eq!(
            gates.len(),
            operations.len(),
            "the gate set and the model's operations must match exactly, got {} gates for \
             {} operations",
            gates.len(),
            operations.len()
        );
    }

}
