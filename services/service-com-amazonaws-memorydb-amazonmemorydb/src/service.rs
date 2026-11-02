// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-memorydb-amazonmemorydb` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error code MemoryDB states a refused request value with.
///
/// This service's model declares NO `ValidationException` — for any operation — and
/// declares `InvalidParameterValueException` ("The specified parameter value is not
/// valid.", HTTP 400, awsQuery code `InvalidParameterValue`) on every write that takes
/// a value. So a request this service refuses because a VALUE breaks a rule is stated
/// with that shape, whether the rule was caught by the handler or by the boundary
/// validator: a client branching on the error code sees ONE code for one logical
/// failure rather than a modelled one from the handler and a synthetic one from the
/// framework.
///
/// IN SCOPE, and the whole of it: a violation of a rule the MODEL declares on a VALUE —
/// a `@pattern` (`UserName`, `AccessString`), a `@length` (a name, a filter's value
/// list, a tag list), a `@range`, an enum's value set, `@uniqueItems`. Those are the
/// same rules this service enforces itself one notch tighter (see
/// `crate::state::resource_name_rejection` and `crate::state::PageSizeWindow`), which is
/// why
/// one code has to cover both. OUT OF SCOPE: a member that is absent altogether, left
/// to the framework's own wording because nothing in the model or this service's
/// recorded behaviour says which code MemoryDB answers that with; and an
/// authentication refusal, which is the emulator's rather than this service's.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The members a request carries a MemoryDB RESOURCE NAME in, and the kind of resource
/// each one names.
///
/// A violation states its member as the last segment of a dotted path, in lowerCamel
/// (`UserName` → `userName`), so the lookup below matches the segment
/// case-insensitively: how the framework re-cases an acronym-leading member
/// (`ACLName`) is not a fact this list should have to know.
const NAME_MEMBERS: &[(&str, crate::state::ResourceKind)] = &[
    ("userName", crate::state::ResourceKind::User),
    ("aclName", crate::state::ResourceKind::Acl),
    ("clusterName", crate::state::ResourceKind::Cluster),
    (
        "parameterGroupName",
        crate::state::ResourceKind::ParameterGroup,
    ),
    ("snapshotName", crate::state::ResourceKind::Snapshot),
    ("subnetGroupName", crate::state::ResourceKind::SubnetGroup),
    (
        "multiRegionClusterName",
        crate::state::ResourceKind::MultiRegionCluster,
    ),
    (
        "multiRegionParameterGroupName",
        crate::state::ResourceKind::MultiRegionParameterGroup,
    ),
];

/// This service's own wording for a name the MODEL refused, or `None` when the
/// violation is not about how a resource name is spelled.
///
/// MemoryDB has ONE name rule per resource and states it in one sentence
/// (`crate::state::resource_name_rejection`), and the rule the model declares on a name
/// member (`@pattern`, `@length`) is the LOOSER half of that same rule — so a name
/// refused at the boundary has to read exactly as a name refused by the handler does,
/// or a caller would learn which rule it broke only when the name happened to get past
/// the model's half of it. The offending value comes from the violation, so the
/// sentence is still built from the request.
fn name_rule_message(violation: &smithy_server_runtime::Violation) -> Option<String> {
    use smithy_server_runtime::ViolationKind;
    // Only a constraint on how the value is SPELLED: a bound on another kind of
    // member, or an absent one, says nothing about a name rule.
    match violation.kind {
        ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_) => {}
        _ => return None,
    }
    let value = violation.value.as_deref()?;
    let member = violation.path.rsplit('.').next()?;
    let (_, kind) = NAME_MEMBERS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(member))?;
    crate::state::resource_name_rejection(*kind, value)
}

/// The members a request carries a LIST in whose over-long value this service words
/// itself, and the sentence each one takes, with `{max}` standing for the maximum the
/// model declares.
///
/// A violation states its member as the last segment of a dotted path, in lowerCamel, so
/// the lookup below matches the segment case-insensitively, exactly as [`NAME_MEMBERS`]
/// does.
const LIST_LENGTH_MEMBERS: &[(&str, &str)] = &[(
    "clusterNames",
    "Too many cluster names provided. Maximum is {max}.",
)];

/// This service's own wording for a list the MODEL refused as too long, or `None` when
/// the violation is not about a list this service words itself.
///
/// A `@length` maximum on a list a request carries is a SERVICE limit, and the service
/// states it in its own sentence: what there were too many of, and the maximum. That
/// maximum comes from the violation — i.e. from the model's own bound — so the sentence
/// cannot name a limit the validator does not enforce, and the over-long value itself is
/// not echoed.
fn list_length_rule_message(violation: &smithy_server_runtime::Violation) -> Option<String> {
    use smithy_server_runtime::ViolationKind;
    let ViolationKind::MaxLength(max) = violation.kind else {
        return None;
    };
    let member = violation.path.rsplit('.').next()?;
    LIST_LENGTH_MEMBERS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(member))
        .map(|(_, sentence)| sentence.replace("{max}", &max.to_string()))
}

/// This service's own wording for a value the MODEL refused because it is outside an
/// enumerated type's value set, or `None` for an enum this service words no sentence of
/// its own for.
///
/// MemoryDB states a rejected enum value in ITS OWN sentence per enumerated type, not in
/// the generic constraint clause: the sentence is a property of the TYPE the member
/// targets, so the violation is matched on the value set the model declares for it (the
/// generated catalogs in `crate::ops::types`) rather than on a member path, and every
/// member targeting that type is refused in the same words however deep in the request
/// it sat. An enum with no sentence of this service's own keeps the framework's clause.
fn enum_rule_message(violation: &smithy_server_runtime::Violation) -> Option<String> {
    use smithy_server_runtime::ViolationKind;
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return None;
    };
    // The generated catalogs are sorted, the violation lists the model's own order, so
    // the two are compared as the SETS they are.
    let targets = |catalog: &[&str]| {
        let mut declared: Vec<&str> = allowed.iter().map(String::as_str).collect();
        declared.sort_unstable();
        declared == catalog
    };
    if targets(crate::ops::types::SOURCE_TYPE_VALUES) {
        return Some(crate::state::INVALID_EVENT_SOURCE_TYPE_MESSAGE.to_string());
    }
    if targets(crate::ops::types::SERVICE_UPDATE_STATUS_VALUES) {
        // This one reports the rejected status back, so it needs the value the request
        // sent; an enum member is only refused when it HELD one.
        return Some(crate::state::invalid_service_update_status_message(
            violation.value.as_deref()?,
        ));
    }
    None
}

/// The `com-amazonaws-memorydb-amazonmemorydb` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // A value that failed a rule the MODEL declares is the same logical failure as
        // one that failed a rule only the service enforces — an out-of-range name, a
        // value outside a declared pattern — so both are stated with
        // [`INVALID_PARAMETER_VALUE`], the shape this service models for exactly that
        // (see the constant). An absent required member is left to the framework's
        // default wording: nothing in the model or the service's traces says which code
        // MemoryDB answers that with, and guessing one would be worse than the
        // framework's.
        match refusal {
            Refusal::Invalid(violations) => {
                let value_violations: Vec<&smithy_server_runtime::Violation> = violations
                    .iter()
                    .filter(|violation| {
                        !matches!(
                            violation.kind,
                            smithy_server_runtime::ViolationKind::MissingRequiredMember
                        )
                    })
                    .collect();
                if value_violations.is_empty() {
                    return None;
                }
                Some(Refused {
                    code: INVALID_PARAMETER_VALUE.to_string(),
                    // Each violation already states the member it refused and the
                    // requirement that member broke, so the message is those clauses
                    // and nothing invented here — except a RESOURCE NAME, which this
                    // service states in its own one-sentence rule (see
                    // [`name_rule_message`]) so that the boundary and the handler
                    // refuse a name with the same words, and a value outside an
                    // enumerated type this service words itself (see
                    // [`enum_rule_message`]).
                    message: value_violations
                        .iter()
                        .map(|violation| {
                            name_rule_message(violation)
                                .or_else(|| enum_rule_message(violation))
                                .or_else(|| list_length_rule_message(violation))
                                .unwrap_or_else(|| violation.message())
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                })
            }
            // Authentication is the emulator's, not this service's, to word.
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
