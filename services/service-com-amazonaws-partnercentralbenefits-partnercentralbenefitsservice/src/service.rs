// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice` service: how it claims requests and how it words the
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
//!   generated routing table. This service returns `Some(op)` for every modelled
//!   operation, serving each through the [`ModelledValidation`] front so a refused
//!   input is stated in the modelled `ValidationException`; see that type.
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
//! # How this service refuses a request on its input constraints
//!
//! PartnerCentral Benefits answers a member outside a modelled constraint with its own
//! `ValidationException` — `@httpError 400`, with the prose in the `Message` member the
//! model declares beside the `Reason` it also marks @required
//! (`smithy_query(query="shape",shape="ValidationException")`). The framework's
//! pre-dispatch renderer gets the status and the error code right, but writes the prose
//! into a framework-chosen lowercase `message` key that is the same for every awsJson
//! service, so the body carries no `Message` at all and an SDK deserializing the modelled
//! shape surfaces a constraint refusal with no reason.
//!
//! [`ModelledValidation`] closes that gap: every modelled operation is served through it,
//! and a request it finds refusable is answered through the modelled `ValidationException`
//! instead. The violations come from the framework's OWN deserializer and constraint walk,
//! so the front cannot refuse a different set of requests than the walk behind it.

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused, Service,
    ServerHttpProtocol, Violation, ViolationKind,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, which every operation declares with `@httpError 400`
/// (`smithy_query(query="shape",shape="ValidationException")`). It is also the code the
/// framework would have chosen, restated here because `override_refusal` names the code
/// and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The requirement one violation class states, in this service's wording.
///
/// Separate from [`constraint_clause`] so a handler refusing an unmodelled constraint of
/// the same CLASS (a length an operation enforces itself, say) states it with the same
/// words the centralized walk uses, rather than a copy that can drift.
///
/// `None` for a class this service has no wording for — see [`constraint_clause`].
pub fn constraint_requirement(kind: &ViolationKind) -> Option<String> {
    let requirement = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // The requirement reported is the expression itself, without the `^` / `$` the
        // model spells it with: the service states the character class a value must
        // satisfy, not the anchoring the model uses to make the match total.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        // A value set is stated in ASCENDING order, not in the order the model happens
        // to declare its enum members in: the recorded refusals list a set sorted, and
        // this crate's own model-derived `*_VALUES` constants are emitted sorted too, so
        // the set a caller is shown does not depend on the model's member order.
        ViolationKind::EnumValue(allowed) => {
            let mut sorted: Vec<&str> = allowed.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            format!("Member must satisfy enum value set: [{}]", sorted.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(requirement)
}

/// One clause of a refusal: the member path, then the requirement it failed. The one
/// construction both the walk's refusals and a handler's own use, so the two cannot
/// drift.
///
/// `subject_null` renders the subject for an absent member, which has no submitted value
/// at all: that is a fact of the request's shape, not a rendering of a value the caller
/// sent.
pub fn member_clause(path: &str, requirement: &str, subject_null: bool) -> String {
    let subject = if subject_null { "Value null" } else { "Value" };
    format!("{subject} at '{path}' failed to satisfy constraint: {requirement}")
}

/// The member path a refusal states, spelled the way the centralized walk spells one.
///
/// `member` is an input member as the WIRE spells it, optionally followed by the
/// `.`-separated steps a handler addressed below it (a position inside a document, a list
/// element). Only the leading INPUT MEMBER is re-cased, and through the framework's own
/// `aws_smithy_schema_server::constraints::path::member` — the single spelling the walk
/// reports — so one input member has ONE path however it was refused: by a modelled
/// constraint the walk enforces or by a rule a handler enforces itself. The steps below it
/// are keys the CALLER chose and are echoed exactly as sent, the way the walk echoes a map
/// entry's key.
///
/// Idempotent on a path already spelled this way, so a handler may hand over either
/// spelling and still name the member the walk names.
pub fn walk_member_path(member: &str) -> String {
    let spell = ::aws_smithy_schema_server::constraints::path::member;
    match member.split_once('.') {
        Some((head, below)) => format!("{}.{below}", spell(head)),
        None => spell(member),
    }
}

/// Render one constraint violation the way PartnerCentral states it: the member path
/// and the constraint it failed, and NOTHING of the value that was rejected.
///
/// A violation on an ELEMENT of a collection is reported against the COLLECTION MEMBER
/// itself — no element index and no `member` step — with the element's requirement
/// nested inside `Member must satisfy constraint: [ … ]`, which is what tells a caller
/// the violation is on an element rather than on the collection. A violation of the
/// collection ITSELF (its `@length`, its `@uniqueItems`) is not nested, and neither is
/// one on a member INSIDE a structure element, which keeps its own full path.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence ([`ViolationKind`] is `#[non_exhaustive]`, so a class
/// can be added upstream).
pub fn constraint_clause(violation: &Violation) -> Option<String> {
    reported(violation).map(|(_, _, clause)| clause)
}

/// One violation as the clause it states, with the MEMBER it is reported against and the
/// number of collection steps its path collapsed through (0 for a violation of the member
/// itself, 1 for one of its elements, 2 for an element of those, …).
///
/// [`constraint_envelope`] orders the clauses of one member by that depth, so the
/// requirement on a collection is stated before the requirements on its elements.
fn reported(violation: &Violation) -> Option<(String, usize, String)> {
    let mut requirement = constraint_requirement(&violation.kind)?;
    // Walk out of every collection step the path ends with, nesting the requirement once
    // per step: a list of lists nests twice, for the same reason it nests once.
    let mut path = violation.path.as_str();
    let mut depth = 0usize;
    while let Some(collection) = enclosing_collection(path) {
        requirement = element_requirement(&requirement);
        path = collection;
        depth += 1;
    }
    // An absent member has no submitted value at all, so it is reported as the null it
    // was: that is a fact of the request's shape, not a rendering of a value the caller
    // sent. Every other violation names the member only.
    let absent = matches!(violation.kind, ViolationKind::MissingRequiredMember);
    Some((
        path.to_string(),
        depth,
        member_clause(path, &requirement, absent),
    ))
}

/// The segment the framework's validation path carries for the step from a collection
/// into one of its elements: `aws_smithy_schema_server::constraints::path::element`
/// spells an element as its 1-based position followed by this segment
/// (`fulfillmentTypes.1.member`).
const COLLECTION_STEP: &str = ".member";

/// The requirement every element of a collection carries beyond the constraint it
/// failed: none of this service's lists is `@sparse` (the model declares the trait
/// nowhere), so an element may not be null, and the element's requirement set states it.
const ELEMENT_NOT_NULL: &str = "Member must not be null";

/// One requirement as it reads for a collection ELEMENT: the element's requirement set,
/// bracketed — the constraint it failed, then the requirement that it be present at all.
pub fn element_requirement(requirement: &str) -> String {
    format!("Member must satisfy constraint: [{requirement}, {ELEMENT_NOT_NULL}]")
}

/// The path of the collection a violation's path points INTO, when the violation is on
/// one of that collection's elements.
///
/// An element path ends with the collection step preceded by the element's 1-based
/// position (`fulfillmentTypes.1.member` -> `fulfillmentTypes`). The position is what
/// identifies the step, so neither a map entry (`tags.someKey.member`) nor a member that
/// happens to be NAMED `member` is mistaken for one.
fn enclosing_collection(path: &str) -> Option<&str> {
    let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
    if position.is_empty() || !position.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(head)
}

/// A `@pattern` expression without the anchors the model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not the end
/// anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `ValidationException` message for a refused request: the AWS validation envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// Two elements of one collection failing the same constraint state the SAME clause once
/// their paths collapse onto the collection member, so an identical clause is reported
/// once: the member is named once however many of its elements were refused, and the
/// count prefix counts the clauses the caller is shown.
///
/// The clauses of one member are stated OUTERMOST FIRST — the requirement on the member
/// itself (its `@length`, its `@uniqueItems`) before the requirements on its elements —
/// whichever order the walk happened to find them in; members themselves keep the order
/// the request declares them in.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
pub fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    // (the member's position in the request, the collapse depth, the clause).
    let mut members: Vec<String> = Vec::new();
    let mut reported_clauses: Vec<(usize, usize, String)> = Vec::with_capacity(violations.len());
    for violation in violations {
        let (member, depth, clause) = reported(violation)?;
        let position = match members.iter().position(|seen| *seen == member) {
            Some(position) => position,
            None => {
                members.push(member);
                members.len() - 1
            }
        };
        reported_clauses.push((position, depth, clause));
    }
    // Stable, so the walk's order is kept among clauses of the same member and depth.
    reported_clauses.sort_by_key(|(position, depth, _)| (*position, *depth));
    let mut clauses: Vec<String> = Vec::with_capacity(reported_clauses.len());
    for (_, _, clause) in reported_clauses {
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    }
    validation_envelope(&clauses)
}

/// The AWS validation envelope (`N validation error(s) detected: `) over clauses already
/// rendered by [`member_clause`]. `None` for no clause at all.
///
/// A handler refusing an input member of its own states it through here, so an
/// in-handler refusal reads exactly like one the centralized walk produced.
pub fn validation_envelope(clauses: &[String]) -> Option<String> {
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The HTTP status a `ValidationException` carries: the `@httpError 400` the model
/// declares on it (`smithy_query(query="shape",shape="ValidationException")`), restated
/// here because [`ModelledValidation`] serializes the error itself rather than returning
/// it through an operation's error channel.
const VALIDATION_STATUS: u16 = 400;

/// This service's own modelled `ValidationException` for a request the boundary refused:
/// the AWS validation envelope over one clause per violation in `Message`, and the
/// `Reason` the model requires beside it.
///
/// The model declares `Message` and `Reason` @required and `FieldList` optional, so both
/// required members are always stated. `FieldList` is left absent: the model documents it
/// as "a list of fields that failed validation" but nothing says what the service puts
/// there for a shape-constraint violation, and the member path a client needs is already
/// in `Message`. A handler refusing a rule the model cannot express fills it itself (see
/// `crate::state::validation_field`), because there it knows the field and the code.
///
/// `None` declines the refusal — see [`constraint_clause`].
pub fn constraint_exception(
    violations: &[Violation],
) -> Option<crate::ops::types::ValidationException> {
    Some(crate::ops::types::ValidationException {
        message: constraint_envelope(violations)?,
        reason: crate::state::VALIDATION_REASON_FIELD.to_string(),
        field_list: None,
    })
}

/// Answers a request whose input violates a modelled constraint in the operation's own
/// modelled `ValidationException`, then hands every other request to the generated walk.
///
/// WHY this is a seam and not a line in a handler: the centralized walk validates every
/// modelled constraint BEFORE a handler runs, and renders a violation through the
/// protocol's GENERIC refusal envelope — a body carrying a lowercase `message` and
/// nothing else — which cannot carry this service's `ValidationException` members at all.
/// `Service::override_refusal` chooses only the error CODE and the message text, not the
/// body's shape, so the modelled `Message` / `Reason` members can only be put on the wire
/// from in FRONT of the walk, where the violations the boundary found are still in hand.
/// The wording itself is unchanged: it comes from the same [`constraint_envelope`] the
/// refusal seam uses.
///
/// Generic over the OPERATION, not over this service: the input shape to read and the
/// schema to validate against are the operation's own, read off its `impl Operation`
/// (`O::INPUT_SCHEMA`, `O::NAME`) rather than spelled out here, so this seam validates
/// exactly what the generated walk would and cannot drift from the model. One instance
/// per operation lives in [`EmulatedService::validation_front`].
struct ModelledValidation<O>(::std::marker::PhantomData<O>);

impl<O: Operation> ModelledValidation<O> {
    /// The violations in `req`, read the way the framework's own walk reads them:
    /// deserialize the input against the operation's schema, then validate the built
    /// value against it. `None` means the request carries no structured refusal — either
    /// it is valid, or it failed in a way that has no member-level form (a body the codec
    /// cannot read at all), which the generated walk renders itself.
    fn violations(
        protocol: &dyn ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<Vec<Violation>> {
        let schema = O::INPUT_SCHEMA;
        let error = match protocol.deserialize_input(req, schema) {
            Ok(mut deser) => match <O::Input as DeserializableStruct>::deserialize(&mut *deser) {
                // A built input: every modelled constraint, in one walk.
                Ok(input) => {
                    let violations = aws_smithy_schema_server::constraints::validate(&input, schema);
                    return (!violations.is_empty()).then_some(violations);
                }
                // Absence is found while READING the request, so it arrives as a codec
                // error; `refusal_of` gives it the same structured form a constraint
                // violation has.
                Err(e) => aws_smithy_schema_server::ProtocolError::Serde(e),
            },
            Err(e) => e,
        };
        match smithy_server_runtime::refusal_of(&error) {
            Some(Refusal::Invalid(violations)) if !violations.is_empty() => Some(violations),
            _ => None,
        }
    }
}

impl<O: Operation> ErasedOperation for ModelledValidation<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            if let Some(violations) = Self::violations(protocol, req) {
                if let Some(exception) = constraint_exception(&violations) {
                    return protocol
                        .serialize_error(
                            &exception,
                            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                            crate::ops::types::ValidationException::shape_name(),
                            VALIDATION_STATUS,
                        )
                        .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
                }
            }
            // Nothing this seam states: run the generated deserialize -> validate ->
            // handle -> serialize walk, which also renders a refusal this seam declined
            // (a violation class with no wording, a body the codec cannot read) through
            // `override_refusal` below. Reached through the routing table rather than
            // through `override_operation`, which would route straight back here. The
            // walk reads and validates the body again — the price of answering a refusal
            // in the modelled shape, paid once per request on a body the transport has
            // already bounded.
            match svc.definition().routing_table().get(O::NAME) {
                Some(op) => op.run(svc, req, ctx).await,
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The `com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ModelledValidation`] per operation this service declares, keyed by the
    /// routing key the model names the operation by, so a refused input is answered in
    /// the modelled `ValidationException` whichever operation it was sent to.
    validation_front: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

/// Put one [`ModelledValidation`] per operation into the front, keyed by that
/// operation's own modelled `NAME`.
///
/// A macro so the list is one line per operation and every entry is built the same way:
/// the key is read off the operation's `impl Operation`, never spelled out, so an
/// operation cannot be registered under a key the router will not resolve.
macro_rules! validation_front {
    ($($op:ty),+ $(,)?) => {{
        let mut front: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
        $(
            front.insert(
                <$op as Operation>::NAME,
                Box::new(ModelledValidation::<$op>(::std::marker::PhantomData))
                    as Box<dyn ErasedOperation>,
            );
        )+
        front
    }};
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            // EVERY operation the model binds: an input constraint is a shape fact, so
            // the operation a refused request happened to name changes nothing about how
            // the refusal is stated.
            validation_front: validation_front![
                crate::ops::amend_benefit_application::AmendBenefitApplication<Eng>,
                crate::ops::associate_benefit_application_resource::AssociateBenefitApplicationResource<Eng>,
                crate::ops::cancel_benefit_application::CancelBenefitApplication<Eng>,
                crate::ops::create_benefit_application::CreateBenefitApplication<Eng>,
                crate::ops::disassociate_benefit_application_resource::DisassociateBenefitApplicationResource<Eng>,
                crate::ops::get_benefit::GetBenefit<Eng>,
                crate::ops::get_benefit_allocation::GetBenefitAllocation<Eng>,
                crate::ops::get_benefit_application::GetBenefitApplication<Eng>,
                crate::ops::list_benefit_allocations::ListBenefitAllocations<Eng>,
                crate::ops::list_benefit_applications::ListBenefitApplications<Eng>,
                crate::ops::list_benefits::ListBenefits<Eng>,
                crate::ops::list_tags_for_resource::ListTagsForResource<Eng>,
                crate::ops::recall_benefit_application::RecallBenefitApplication<Eng>,
                crate::ops::submit_benefit_application::SubmitBenefitApplication<Eng>,
                crate::ops::tag_resource::TagResource<Eng>,
                crate::ops::untag_resource::UntagResource<Eng>,
                crate::ops::update_benefit_application::UpdateBenefitApplication<Eng>,
            ],
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
        // Every modelled operation is served through its [`ModelledValidation`] front, so
        // a request whose input violates a modelled constraint is refused in this
        // service's own `ValidationException` — with the `Message` and `Reason` the model
        // marks @required — instead of the framework's generic refusal body, which
        // carries neither. A request with no violation falls through the front to the
        // generated walk unchanged; a routing key this service does not model resolves
        // here to `None`, as it must, so the routing error is still the entry's.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        self.validation_front
            .get(routing_key)
            .map(::std::convert::AsRef::as_ref)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // PartnerCentral states a constraint violation WITHOUT the value that violated
        // it: the message names the member path and the constraint only, so a caller's
        // input is never echoed back into a client's logs, and a `@pattern` requirement
        // is quoted unanchored. The code stays the modelled `ValidationException`
        // (HTTP 400), which is what the framework would have answered — only the wording
        // changes.
        //
        // This is the second line of defence, not the main one: a violation found on a
        // request the model names an operation for is answered by
        // [`ModelledValidation`] in the modelled shape before the walk gets here, because
        // `Refused` is a code and a message and so cannot carry the modelled members at
        // all. What still reaches here is a refusal that seam declined — a violation
        // class this service has no wording for, which `constraint_envelope` declines
        // again below — so the wording stays single-homed either way.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
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
