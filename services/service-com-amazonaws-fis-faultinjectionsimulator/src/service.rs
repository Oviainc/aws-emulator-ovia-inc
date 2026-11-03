// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-fis-faultinjectionsimulator` service: how it claims requests and how it words the
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

/// The error code FIS answers a refused request with: its model declares
/// `ValidationException` on every operation that takes caller input, and that is the code
/// a client branches on.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `com-amazonaws-fis-faultinjectionsimulator` emulated service: the generated [`ServiceDefinition`] plus
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
        // Four classes FIS words itself:
        //
        // * a value longer than its member's `@length` maximum, stated as
        //   "Maximum length exceeded: <member>" — see [`max_length_message`];
        // * a `maxResults` outside the `@range` its member declares, stated as the member
        //   and BOTH bounds — see [`crate::state::max_results_out_of_range`];
        // * a `resourceType` outside `TargetResourceTypeId`'s `@pattern`, which FIS does
        //   not report as a pattern violation at all: it judges the value against its
        //   published catalog, so a value no entry can match is refused as the type that
        //   was not found — see [`crate::state::target_resource_type_not_found`];
        // * a safety-lever state naming a status the input enum does not admit, stated as
        //   "Invalid status: <value>" — see
        //   [`crate::state::invalid_safety_lever_status_message`].
        //
        // Every other refusal class keeps the framework's wording, because FIS publishes
        // no rule for how it words those.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        // The FIRST violation among them that FIS words itself, wherever it is reported: a
        // refusal that carries one behind another class is still that refusal to the
        // caller. Length comes first, because it is the one rule that holds of every
        // member rather than of one.
        let message = violations
            .iter()
            .find_map(|violation| {
                matches!(
                    violation.kind,
                    smithy_server_runtime::ViolationKind::MaxLength(_)
                )
                .then(|| max_length_message(&violation.path))
            })
            .or_else(|| violations.iter().find_map(max_results_out_of_range))
            .or_else(|| violations.iter().find_map(unmatchable_resource_type))
            .or_else(|| violations.iter().find_map(invalid_safety_lever_status))?;
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message,
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

/// The dotted path of the member whose status a safety-lever state names
/// (`UpdateSafetyLeverStateRequest.state.status`, targeting `SafetyLeverStatusInput`).
///
/// SOURCE: the model's `com.amazonaws.fis#UpdateSafetyLeverStateRequest`, whose `state`
/// member targets `UpdateSafetyLeverStateInput` and its `status`.
const SAFETY_LEVER_STATUS_PATH: &str = "state.status";

/// The member every paginated list of this service sizes its page with
/// (`@range(min: 1, max: 100)` on each list's own `maxResults` target).
const MAX_RESULTS_PATH: &str = "maxResults";

/// The member `GetTargetResourceType` names a resource type with
/// (`GetTargetResourceTypeRequest.resourceType`, targeting `TargetResourceTypeId`:
/// `@length(min: 0, max: 128)`, `pattern ^[\S]+$`). A `resourceType` nested inside a
/// template's `targets` map is reported at a path with that map in front of it, so this
/// bare path names the catalog lookup's own member and nothing else.
const RESOURCE_TYPE_PATH: &str = "resourceType";

/// `Some(message)` when `violation` is a `maxResults` outside the `@range` its member
/// declares, worded as FIS words it ([`crate::state::max_results_out_of_range`]); `None`
/// for every other violation.
///
/// FIS states both published bounds whichever one the value crossed, and echoes no value,
/// so the two range violations draw the one sentence.
fn max_results_out_of_range(violation: &smithy_server_runtime::Violation) -> Option<String> {
    (violation.path == MAX_RESULTS_PATH
        && matches!(
            violation.kind,
            smithy_server_runtime::ViolationKind::MinValue(_)
                | smithy_server_runtime::ViolationKind::MaxValue(_)
        ))
    .then(crate::state::max_results_out_of_range)
}

/// `Some(message)` when `violation` is a `GetTargetResourceType` `resourceType` outside
/// `TargetResourceTypeId`'s `@pattern`; `None` for every other violation.
///
/// FIS judges that member against its published catalog rather than reporting the pattern:
/// a value the pattern refuses matches no catalogued entry either, so the caller is told
/// the type was not found ([`crate::state::target_resource_type_not_found`]) — the same
/// refusal, in the same words, as any other id the catalog holds no entry for.
fn unmatchable_resource_type(violation: &smithy_server_runtime::Violation) -> Option<String> {
    if violation.path != RESOURCE_TYPE_PATH
        || !matches!(
            violation.kind,
            smithy_server_runtime::ViolationKind::Pattern(_)
        )
    {
        return None;
    }
    crate::state::target_resource_type_not_found(violation.value.as_deref().unwrap_or_default())
        .message
}

/// `Some(message)` when `violation` is a safety-lever state naming a status outside the
/// values `SafetyLeverStatusInput` admits, worded as FIS words it
/// ([`crate::state::invalid_safety_lever_status_message`]); `None` for every other
/// violation.
///
/// The boundary validator checks enum membership before a handler runs, so this is where
/// that refusal is reworded; the handler that persists the status words the same rejection
/// through the same function.
fn invalid_safety_lever_status(violation: &smithy_server_runtime::Violation) -> Option<String> {
    if violation.path != SAFETY_LEVER_STATUS_PATH
        || !matches!(
            violation.kind,
            smithy_server_runtime::ViolationKind::EnumValue(_)
        )
    {
        return None;
    }
    Some(crate::state::invalid_safety_lever_status_message(
        violation.value.as_deref().unwrap_or_default(),
    ))
}

/// How this service words the refusal of a value longer than its member's `@length`
/// maximum: a message built from the request, naming the member that was too long
/// ([`member_of`] `path`) and nothing else — the rejected value is NOT echoed back.
///
/// The ONE place that sentence is written, so a value past a modelled `@length` maximum is
/// refused the same way wherever the bound was applied. A refusal the boundary validator
/// raised reaches it through [`EmulatedService::override_refusal`]; the one modelled leaf
/// that validator does not reach — a map's KEY, whose bounds are applied in
/// [`crate::state::map_key_refusal`] — reaches it from the handler, with the dotted path
/// of the map it keys.
pub(crate) fn max_length_message(path: &str) -> String {
    format!("Maximum length exceeded: {}", member_of(path))
}

/// The member a violation's dotted path names.
///
/// A path is the dotted position of the offending value — `id`, or
/// `targets.web.member.resourceType` for one nested inside a collection. FIS's message
/// names the MEMBER, so the trailing segments that describe the step into a collection
/// (the literal `member`, and a list element's 1-based index) are dropped and the last
/// named member is what is left.
fn member_of(path: &str) -> &str {
    path.split('.')
        .rfind(|segment| *segment != "member" && !segment.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or(path)
}
