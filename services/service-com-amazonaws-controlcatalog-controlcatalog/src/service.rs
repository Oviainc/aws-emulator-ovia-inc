// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-controlcatalog-controlcatalog` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use ::aws_smithy_schema::Schema;
use ::service_common::validation::{list_of_element, unanchored};
use ::service_common::validation_message::{element_constraint_set, element_set_requirement};

/// Whether IAM lets a request to this service perform the action it asks for.
pub mod authorization;

/// The error code Control Catalog answers an input-constraint violation with: the
/// modeled `ValidationException`, which every one of this service's six operations
/// declares with `@httpError 400` (the framework's own default code for a refusal,
/// restated here because `override_refusal` replaces code and message together).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The shapes a refused COLLECTION member is looked up in, to state the whole constraint
/// set that member's ELEMENTS have to satisfy.
///
/// Every list of constrained scalars in this service's input surface sits inside a filter
/// STRUCTURE rather than at the top level of an operation's input
/// (`smithy_query(query="closure",operation="GetControl,ListControls,ListControlMappings,\
/// ListDomains,ListObjectives,ListCommonControls")`), and a member schema carries its own
/// traits and not the members of the shape it targets — so the shapes handed over are the
/// five filter structures that declare a list member, which between them own every one:
/// `ControlArns`, `CommonControlArns` and `MappingTypes`, `GovernedProviders`, `Types` and
/// `Identifiers`, `Domains` and `Objectives`. The last two are lists of STRUCTURES, which
/// declare no element constraint set of their own, so a violation inside one of their
/// entries keeps the member path it was reported at.
const FILTER_SHAPES: &[&Schema] = {
    use crate::ops::types;

    &[
        types::CONTROL_MAPPING_FILTER_SCHEMA,
        types::CONTROL_FILTER_SCHEMA,
        types::IMPLEMENTATION_FILTER_SCHEMA,
        types::OBJECTIVE_FILTER_SCHEMA,
        types::COMMON_CONTROL_FILTER_SCHEMA,
    ]
};

/// What Control Catalog answers a lookup whose control ARN names no published control
/// with.
///
/// A refusal this service decides for itself — one the framework's constraint walk
/// cannot report, because nothing about the request's shape is wrong — is worded with
/// ONE fixed sentence per refusal class: the sentence is a property of the class, not
/// of the request, so the same text comes back whatever ARN, member or element count
/// the caller sent. The service's error shapes carry a free-form `Message` (see
/// `ResourceNotFoundException` / `ValidationException` in the model), which is where
/// these sentences go.
pub(crate) const CONTROL_NOT_FOUND_MESSAGE: &str =
    "AWS Control Tower cannot find a control with the ControlArn value that you provided.";

/// What Control Catalog answers a filter it will not accept with: an entry count the
/// filter does not take, or an entry naming no resource.
///
/// One sentence for the whole class, by the rule on [`CONTROL_NOT_FOUND_MESSAGE`]: the
/// filter member at fault and the number of entries the caller sent are both absent
/// from it, so two operations refusing two different filter members answer with
/// identical text.
pub(crate) const INVALID_FILTER_MESSAGE: &str =
    "The provided filter is not valid. Ensure that each part of the filter is valid, and try again.";

/// Render one constraint violation the way Control Catalog states it: the member
/// path and the violated constraint, and NOTHING of the value that was rejected.
///
/// A constraint one of a COLLECTION's elements broke is not stated at that element: it is
/// stated once at the collection member, over the element shape's whole declared constraint
/// set ([`element_set_clause`]).
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence for it ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
pub(crate) fn constraint_clause(violation: &Violation) -> Option<String> {
    if let Some(clause) = element_set_clause(&violation.path) {
        return Some(clause);
    }
    let requirement = requirement(&violation.kind)?;
    // An absent member has no submitted value at all, so it is reported as the null
    // it was: that is a fact of the request's shape, not a rendering of a value the
    // caller sent. Every other violation names the member only.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The one requirement sentence this service states a constraint with, or `None` for a
/// class it has no wording for.
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
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
        // The requirement reported is the expression itself, without the `^` / `$`
        // the model spells it with: AWS states the character class a value must
        // satisfy, not the anchoring the model uses to make the match total.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => format!(
            "Member must satisfy enum value set: [{}]",
            stated_enum_values(allowed).join(", ")
        ),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// An enumerated type's admissible values as a refusal of this service LISTS them: in the
/// REVERSE of the order the model declares them in.
///
/// A validation clause states the value set as the remediation hint a caller is shown, and
/// the order is the service's rather than the model's. No AWS document fixes it, and the
/// evidence for it is ONE data point, which is all this service's input surface can offer:
/// `MappingType` is its only enumerated member (`smithy_query(query="shape",
/// shape="MappingType")` declares `[FRAMEWORK, COMMON_CONTROL, RELATED_CONTROL]`) and is
/// recorded stated as `[RELATED_CONTROL, COMMON_CONTROL, FRAMEWORK]`. Reversing the declared
/// order is the rule that states that, and it is COMPUTED from the declared set rather than
/// tabulated for the one type, so an enumerated member the model grows is listed by the same
/// rule instead of falling back to a membership nothing recorded. Sibling services differ
/// here — the order is part of each service's validation-message dialect, not a fleet-wide
/// one — so this stays beside this service's own recording.
fn stated_enum_values(declared: &[String]) -> Vec<String> {
    declared.iter().rev().cloned().collect()
}

/// The clause a constraint broken by a COLLECTION's ELEMENTS is stated at — the collection
/// member, carrying the whole constraint set the element shape declares — or `None` when
/// `path` is not a scalar element's.
///
/// AWS reports such a violation once at the collection and lists the element shape's ENTIRE
/// set, not the one constraint a value happened to break, so the clause is the same whatever
/// an element got wrong and however many elements got it wrong. The set is read off the
/// model (the refusal carries only what failed) and the `Member must satisfy constraint:
/// [ … ]` wrapper comes from the shared rendering, so this chooses only WHICH shapes the
/// member is looked up in and how each requirement is worded ([`requirement`]).
fn element_set_clause(path: &str) -> Option<String> {
    let collection = list_of_element(path)?;
    // The member declares its constraints in the shape that owns it, under its own name:
    // the leading segments of the path are the structures it is nested in.
    let member = collection
        .rsplit_once('.')
        .map_or(collection, |(_, member)| member);
    let declared = element_constraint_set(FILTER_SHAPES, member)?;
    let requirements: Vec<String> = declared.iter().filter_map(requirement).collect();
    // A set this service has no wording for every member of is not stated at all, which
    // keeps the framework's own per-element clause rather than an incomplete set.
    if requirements.len() != declared.len() {
        return None;
    }
    Some(format!(
        "Value at '{collection}' failed to satisfy constraint: {}",
        element_set_requirement(&requirements)
    ))
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// A clause already stated is not stated twice: a collection whose elements broke a
/// constraint is named ONCE however many of its elements the framework's walk reported,
/// because every one of them produces the same clause. The scan is quadratic in the clause
/// count, which the framework's own walk caps.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
pub(crate) fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let clause = constraint_clause(violation)?;
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    }
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-controlcatalog-controlcatalog` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Control Catalog states a constraint violation WITHOUT the value that
        // violated it: the message names the member path and the constraint only,
        // so a caller's input is never echoed back into a client's logs. The code
        // stays the modeled `ValidationException` (HTTP 400), which is what the
        // framework would have answered — only the wording changes.
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
