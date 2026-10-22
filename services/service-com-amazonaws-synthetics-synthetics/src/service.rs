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
//! ## The one rendering difference from the framework's own
//!
//! A `@pattern` requirement is stated as the EXPRESSION a value has to satisfy, without
//! the `^` / `$` pair a model writes to turn Smithy's unanchored find into a whole-string
//! match ([`unanchored`]): that anchoring is how the model spells the rule, not part of
//! the rule the service reports, and it is this service's recorded behaviour — a rejected
//! `TagResource` ARN is reported against `arn:(aws[a-zA-Z-]*)?:synthetics:…` and not
//! against `^arn:…$`. The difference is in the REQUIREMENT half only, so a caller still
//! sees its own value echoed and the clause, the count and the envelope stay the
//! framework's. It holds for every pattern-constrained member of this service, because the
//! rendering is single-homed in [`as_stated`] rather than keyed to an operation.

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

/// A `@pattern` expression without the anchors a model may spell it with.
///
/// Smithy's `@pattern` is an unanchored FIND, so a model that means "the whole value
/// matches" writes `^…$` around the expression; the requirement the service reports is
/// the expression itself. The pair is dropped only when BOTH ends carry it AND the pair
/// wraps the WHOLE expression: a `^` alone, or a `$` alone, is an expression anchored at
/// one end — a different language, not an anchored spelling of this one — and in an
/// alternation (`^a$|^b$`, `^a|b$`) the outer `^` and `$` govern only their own branch, so
/// dropping them would state an expression the service never evaluated. A `$` that is
/// ESCAPED is a literal dollar, not an anchor, so it stays.
///
/// The same rule is inlined in many service crates (`grep -rln 'fn unanchored'
/// services/*/src`) because none is reachable from another;
/// `services/common/src/validation.rs` names it under "Still to migrate here" and stages
/// it for one migration of its own, which is to DELETE these copies rather than add
/// another. This one is pinned by the unit tests below and by this crate's
/// `tests/all/server_side_validation.rs`.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) && wraps_whole_expression(inner) => inner,
        _ => pattern,
    }
}

/// Whether a `^`/`$` pair around `expression` anchors the WHOLE expression, which is what
/// a whole-value pattern's pair does.
///
/// It does not when `expression` carries an anchor of its own (so the outer pair belongs
/// to the branches of an alternation, `^a$|^b$`), nor when it alternates at the top level
/// (`^a|b$`, where the `^` governs only the first branch and the `$` only the last). A
/// `^`, `$` or `|` inside a character class is a literal or a negation marker rather than
/// an anchor or an alternation, and one inside a group alternates only within that group
/// (`^(canary|group)$` IS wrapped), so the scan tracks class and group context. Escapes
/// are honoured throughout.
fn wraps_whole_expression(expression: &str) -> bool {
    let mut escaped = false;
    let mut in_class = false;
    let mut depth = 0_usize;
    for character in expression.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            _ if in_class => {}
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '^' | '$' => return false,
            '|' if depth == 0 => return false,
            _ => {}
        }
    }
    true
}

/// Whether `expression` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(expression: &str) -> bool {
    expression.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// The same violation with a `@pattern` requirement stated [`unanchored`]; every other
/// violation class is returned untouched, so the requirement each one words — and the
/// value every clause echoes — stays the framework's.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
            value: violation.value.clone(),
        },
        _ => violation.clone(),
    }
}

/// The `com-amazonaws-synthetics-synthetics` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
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

/// One [`ModeledInputValidation`] gate per modelled operation.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
fn input_validation_gates<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModeledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation the model binds. A constraint is a property of the INPUT SHAPE,
    // so the gate goes in front of all of them, implemented or still scaffolded: a
    // request that violates the model is refused the same way whoever serves it.
    gate!(
        gates;
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
        list_tags_for_resource::ListTagsForResource,
        start_canary::StartCanary,
        start_canary_dry_run::StartCanaryDryRun,
        stop_canary::StopCanary,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_canary::UpdateCanary,
    );
    gates
}

/// The modelled-`ValidationException` gate in front of one operation `Op`.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape and the schema the
/// constraint walk runs against. It holds no engine and no state, so the operation it
/// guards stays the single owner of the request's behaviour.
struct ModeledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModeledInputValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
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
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req) {
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
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violations = model_violations::<Op>(protocol, req)?;
    // The envelope AWS itself returns for a constraint failure, built from the member
    // path, the offending value and the violated constraint by the framework's own
    // renderer — the same text the generic 400 carried, now in the modelled member. The
    // one difference is `as_stated`: a `@pattern` is quoted as the expression a value has
    // to satisfy, without the anchors the model spells it with.
    let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
    let error = crate::ops::types::ValidationException {
        message: envelope(&stated),
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
        let gate = self.input_validation.get(key)?;
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

    /// A whole-expression `^…$` pair is dropped and nothing else is, so an expression a
    /// model anchors at one end — or that anchors each branch of an alternation — is
    /// reported exactly as the model spells it. The anchored rows span this service's own
    /// patterns (`ResourceArn`, `CanaryName`) plus the forms that must survive untouched.
    #[test]
    fn a_pattern_loses_only_a_whole_expression_anchor_pair() {
        let cases = [
            (
                "the resource ARN pattern",
                r"^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+$",
                r"arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+",
            ),
            ("an anchored character class", r"^[0-9a-z_\-]+$", r"[0-9a-z_\-]+"),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", r"[0-9]+$", r"[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            // Each branch carries its own pair, so the outer `^`/`$` are not the whole
            // expression's: stripping them would change the language.
            ("a fully anchored alternation", "^a$|^b$", "^a$|^b$"),
            ("a half-anchored alternation", "^a|b$", "^a|b$"),
            // An escaped dollar is a literal character, not the end anchor.
            ("an escaped trailing dollar", r"^costs\$", r"^costs\$"),
            // Two backslashes escape the backslash, so the `$` is still the anchor.
            ("an escaped backslash before the anchor", r"^costs\\$", r"costs\\"),
            // A `|` inside a group alternates only within it, so the pair still wraps.
            ("an alternation inside a group", "^(canary|group)$", "(canary|group)"),
            // A `^` inside a character class is a negation, not an anchor.
            ("a negated class", "^[^/]+$", "[^/]+"),
        ];
        for (case, modelled, expected) in cases {
            let actual = super::unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Only the `@pattern` class is restated: every other requirement, and every clause's
    /// echoed value and member path, stays exactly what the framework produced.
    #[test]
    fn only_a_pattern_requirement_is_restated() {
        let pattern = Violation {
            path: "resourceArn".to_string(),
            kind: ViolationKind::Pattern(r"^arn:aws:synthetics:.+$".to_string()),
            value: Some("not-an-arn".to_string()),
        };
        let stated = super::as_stated(&pattern);
        assert_eq!(
            stated.kind,
            ViolationKind::Pattern(r"arn:aws:synthetics:.+".to_string()),
            "expected the pattern requirement unanchored, got {:?}",
            stated.kind
        );
        assert_eq!(
            stated.value.as_deref(),
            Some("not-an-arn"),
            "expected the submitted value to be echoed unchanged, got {:?}",
            stated.value
        );
        assert_eq!(
            stated.path, pattern.path,
            "expected the member path unchanged, got {:?}",
            stated.path
        );

        for other in [
            ViolationKind::MaxValue("20".to_string()),
            ViolationKind::MinLength(1),
            ViolationKind::MissingRequiredMember,
        ] {
            let violation = Violation {
                path: "maxResults".to_string(),
                kind: other.clone(),
                value: Some("21".to_string()),
            };
            assert_eq!(
                super::as_stated(&violation),
                violation,
                "case {other:?}: expected the framework's own violation, unchanged"
            );
        }
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
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/smithy-model/com-amazonaws-synthetics-synthetics.json"
        );
        let model = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the Smithy model is JSON");
        let shapes = model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes");
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
