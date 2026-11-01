// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ssmsap-ssmsap` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it: every modelled operation is served through it
//!   wrapped in `StatedPerOperation`, which is what puts the request's own lists in
//!   hand when a constraint broken by a list's ELEMENT has to be stated at the list
//!   member (see the inline note on that wrapper). The operation served is the one the
//!   generated table's own protocol resolves, so nothing is re-routed.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS service uses it: it keeps the framework's
//!   `ValidationException` envelope, clause and echoed value, reports a
//!   `@pattern` requirement as the expression without the model's outer `^`/`$`
//!   anchors, and attributes a constraint broken by an element of a list to the list
//!   member with that element's requirement nested (see the inline notes on those
//!   methods).
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
use aws_smithy_schema_server::ProtocolError;
use service_common::validation::list_of_element;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException` (`com.amazonaws.ssmsap#ValidationException`,
/// `@httpError 400`), which every operation of this service declares and which is
/// also the code the framework's own refusal renderer picks. Restated here because
/// [`Service::override_refusal`] replaces the code and the message together, and a
/// message without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors the model spells it with.
///
/// A model writes `^…$` to turn Smithy's unanchored find into a whole-string match;
/// that anchoring is how the rule is SPELLED, not part of the expression the service
/// reports, so a client that re-validates locally with the expression it read out of
/// the message applies the one AWS states. Only the OUTER anchors go: a `$` or `^`
/// inside an alternation belongs to the expression, and a `$` escaped by a backslash
/// is a literal dollar sign.
///
/// The same rule is inlined in ~24 other service crates
/// (`grep -rln 'fn unanchored' services/*/src`) rather than shared, because
/// `services/common/src/validation.rs` states ("Still to migrate here (staged, not
/// yet wired)") that moving it — together with the requirement and subject halves —
/// is one change of its own and not a side effect of a single service's issue. This
/// is another such copy, pinned by this crate's
/// `tests/all/validation_message_rendering.rs`; the migration that moves the rule
/// owns this copy too.
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

/// The requirement one violation states, as this service phrases it: the framework's
/// own sentence for every class, with a `@pattern` reported [`unanchored`].
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence for it ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
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
        ViolationKind::Pattern(pattern) => pattern_requirement(pattern),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// The requirement a value outside a `@pattern` states, as this service phrases it: the
/// framework's own sentence over the expression reported [`unanchored`].
///
/// Named on its own because a handler states it too: the ONE constraint class no walk
/// ahead of a handler can apply is a `@pattern` on a map's KEY (the walk treats a key as a
/// path segment), and this service's key expression is one the walk's regex engine cannot
/// compile either — so the sentence a handler-stated refusal carries
/// ([`map_key_pattern_refusal`], [`element_pattern_refusal`]) is this same one rather than
/// a second spelling of it.
fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// What a clause states AROUND the requirement an ELEMENT of a list failed: the element's
/// own requirement nested inside this, so the member reported is the list and the rule
/// reported is the one its elements have to satisfy.
const ELEMENT_CONSTRAINT_OPEN: &str = "Member must satisfy constraint: [";

/// The deepest collection nesting one clause walks out of.
///
/// A step out of a collection is spelled `<position>.member`, and the step a MAP entry
/// carries is the key the CALLER chose, so a key spelled `<digits>.member` is
/// indistinguishable from a list position by the path alone: without a ceiling, the nesting
/// a request could ask for would grow with the length of such a key. Every list this
/// service's inputs declare holds SCALARS (`ConfigurationCheckTypeList`, `TagKeyList`,
/// `InstanceList`), so one step is all they can produce; the ceiling is one past that,
/// enough to state a list of lists were the model to grow one.
const MAX_ELEMENT_DEPTH: usize = 2;

/// The member path a clause names, and how many collection steps it walked out of to get
/// there: `instances.1.member` is stated at `instances`, one step out.
///
/// Bounded by [`MAX_ELEMENT_DEPTH`], so a path that keeps looking like an element step
/// cannot nest a requirement without end. The element spelling itself is
/// `service_common::validation::list_of_element`'s, the one place in the tree that mirrors
/// the validating walk's path bookkeeping.
fn collapsed(path: &str) -> (&str, usize) {
    let mut at = path;
    let mut depth = 0;
    while depth < MAX_ELEMENT_DEPTH {
        match list_of_element(at) {
            Some(collection) => {
                at = collection;
                depth += 1;
            }
            None => break,
        }
    }
    (at, depth)
}

/// What the list at `path` HELD, as a clause states it.
///
/// The request's own list when it is in hand (`service_common::validation::submitted_lists`
/// renders every element the caller sent, in position order); otherwise the elements of
/// that list the walk REFUSED, which is as much of the member's value as the violations
/// themselves know. The fallback understates a list of which only some elements were
/// refused, and is only reached on the path that is handed the violations alone
/// ([`Service::override_refusal`]).
fn list_value(
    violations: &[Violation],
    path: &str,
    lists: &HashMap<String, String>,
) -> Option<String> {
    if let Some(rendered) = lists.get(path) {
        return Some(rendered.clone());
    }
    let mut elements: Vec<&str> = Vec::new();
    for violation in violations {
        let (list, depth) = collapsed(&violation.path);
        if depth == 0 || list != path {
            continue;
        }
        if let Some(value) = violation.value.as_deref() {
            if !elements.contains(&value) {
                elements.push(value);
            }
        }
    }
    (!elements.is_empty()).then(|| format!("[{}]", elements.join(", ")))
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per failing (member, requirement),
/// joined with `; `.
///
/// A clause states the submitted value, the input position that carries it and the
/// requirement it failed. The value is echoed as it was SUBMITTED — this message is the
/// only place the server reports the offending input back, and a caller comparing the echo
/// against what it sent must see its own bytes — and an ABSENT member, which has no
/// submitted value, is stated as the `null` it was.
///
/// A constraint broken by an ELEMENT of a list is stated at the LIST member, with no
/// position and no `member` step: the value echoed is the whole list the member held
/// ([`list_value`]) and the element's requirement is nested inside
/// [`ELEMENT_CONSTRAINT_OPEN`]. Two elements of one list that broke the same rule are
/// therefore ONE clause, while a member that broke two rules still states both. A
/// constraint broken by the list ITSELF (its own `@length`) is not nested, and neither is
/// one broken by a member INSIDE a structure element, which keeps its own full path.
///
/// `None` declines the whole refusal, which keeps the framework's own wording: a violation
/// class this service has no requirement sentence for would otherwise be stated as a clause
/// with no requirement at all.
fn constraint_message(
    violations: &[Violation],
    lists: &HashMap<String, String>,
) -> Option<String> {
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        let mut requirement = requirement(&violation.kind)?;
        for _ in 0..depth {
            requirement = format!("{ELEMENT_CONSTRAINT_OPEN}{requirement}]");
        }
        if !stated.insert((path.to_string(), requirement.clone())) {
            continue;
        }
        let held = (depth > 0)
            .then(|| list_value(violations, path, lists))
            .flatten();
        clauses.push(match (&violation.value, held) {
            (None, _) => {
                format!("Value null at '{path}' failed to satisfy constraint: {requirement}")
            }
            (Some(submitted), held) => {
                let shown = held.as_deref().unwrap_or(submitted.as_str());
                format!("Value '{shown}' at '{path}' failed to satisfy constraint: {requirement}")
            }
        });
    }
    envelope(&clauses)
}

/// AWS's envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `<N> validation errors detected: <clause>; <clause>` for several.
///
/// The wording is the framework's own (`aws_smithy_schema_server::constraints::envelope`),
/// which cannot be CALLED here because it renders each clause from a [`Violation`] and a
/// nested element requirement is not a [`ViolationKind`] any violation carries. The two are
/// pinned against each other in [`tests`].
fn envelope(clauses: &[String]) -> Option<String> {
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// [`constraint_message`] for the path that is handed the violations ALONE, with no
/// request's lists to read ([`Service::override_refusal`]).
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    constraint_message(violations, &HashMap::new())
}

/// The whole `ValidationException` message for a MAP whose KEY is outside the `@pattern`
/// the map's KEY SHAPE declares.
///
/// A HANDLER states this, because no walk ahead of one can: the validating walk treats a
/// map's key as a path segment rather than as a constrained leaf, so it applies no
/// constraint a key shape declares. The wording is still the framework's envelope and
/// clause over the map MEMBER, with the requirement a KEY has to satisfy listed as such
/// and no value quoted — a map has no scalar rendering to echo — and the requirement
/// sentence is [`pattern_requirement`]'s, the same one a reported member gets, so a clause
/// about a key cannot drift from a clause about a member.
pub(crate) fn map_key_pattern_refusal(member_path: &str, pattern: &str) -> String {
    let clause = format!(
        "Value at '{member_path}' failed to satisfy constraint: Map keys must satisfy \
         constraint: {}",
        service_common::validation_message::bracketed_findings(&[pattern_requirement(pattern)])
    );
    match envelope(std::slice::from_ref(&clause)) {
        Some(stated) => stated,
        None => clause,
    }
}

/// The whole `ValidationException` message for a LIST one of whose ELEMENTS is outside the
/// `@pattern` the element shape declares, when the walk could not apply that expression
/// itself — this service's tag-key expression is one its regex engine cannot compile, so
/// every value satisfies it there and the handler applies it instead.
///
/// Worded by the SAME renderer every refused request of this service is worded by
/// ([`constraint_message`]), handed the violation the walk would have reported — at the
/// element's own path, spelled by the walk's own path builder — and the list the caller
/// sent: so the clause lands at the LIST member quoting the whole list, and two elements
/// breaking one rule are one clause, exactly as when the walk reports them. `position` is
/// the 1-based position of the offending element.
pub(crate) fn element_pattern_refusal(
    member_path: &str,
    elements: &[String],
    position: usize,
    pattern: &str,
) -> String {
    use aws_smithy_schema_server::constraints::path;

    let violation = Violation {
        path: path::join(&[member_path.to_string(), path::element(position)]),
        kind: ViolationKind::Pattern(pattern.to_string()),
        value: Some(
            elements
                .get(position.saturating_sub(1))
                .cloned()
                .unwrap_or_default(),
        ),
    };
    // The value a clause states at a list member is the whole list, rendered the way the
    // walk's own collector renders one (`service_common::validation::submitted_lists`):
    // the elements in position order, comma-separated, in brackets.
    let lists: HashMap<String, String> = [(
        member_path.to_string(),
        format!("[{}]", elements.join(", ")),
    )]
    .into_iter()
    .collect();
    constraint_message(std::slice::from_ref(&violation), &lists)
        .unwrap_or_else(|| violation.message())
}

/// The `com-amazonaws-ssmsap-ssmsap` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words it,
    /// with the request's own lists in hand. Keyed by the routing key the protocol resolves
    /// — the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! stated_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
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
        let mut stated = OperationRoutingTable::new();
        // EVERY modelled operation (27 of them, one per file in `src/ops/`), because the
        // rule is the SERVICE's and not one operation's: an operation whose input declares
        // no constrained list simply never has such a violation to state, which the wrapper
        // reads off that operation's own input schema. An operation added later is added
        // here as well, or it answers a refusal without the request's lists in hand; the
        // test below fails when one is missing.
        stated_operations!(
            stated,
            engine,
            delete_resource_permission::DeleteResourcePermission,
            deregister_application::DeregisterApplication,
            get_application::GetApplication,
            get_component::GetComponent,
            get_configuration_check_operation::GetConfigurationCheckOperation,
            get_database::GetDatabase,
            get_operation::GetOperation,
            get_resource_permission::GetResourcePermission,
            list_applications::ListApplications,
            list_components::ListComponents,
            list_configuration_check_definitions::ListConfigurationCheckDefinitions,
            list_configuration_check_operations::ListConfigurationCheckOperations,
            list_databases::ListDatabases,
            list_operation_events::ListOperationEvents,
            list_operations::ListOperations,
            list_sub_check_results::ListSubCheckResults,
            list_sub_check_rule_results::ListSubCheckRuleResults,
            list_tags_for_resource::ListTagsForResource,
            put_resource_permission::PutResourcePermission,
            register_application::RegisterApplication,
            start_application::StartApplication,
            start_application_refresh::StartApplicationRefresh,
            start_configuration_checks::StartConfigurationChecks,
            stop_application::StopApplication,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_application_settings::UpdateApplicationSettings,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it — in particular with the value a clause quotes at a LIST member, which is the
/// whole list the caller sent and which no [`Violation`] carries.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this
/// wrapper does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation. The
/// structurally right place for the collecting half is the frozen crate — handing a handler,
/// or `override_refusal`, the input it already built — which
/// `services/common/src/validation.rs` records as the core seam this re-read exists only
/// until.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// This service's answer to every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk finds and the wording this file gives them.
/// `None` when the request breaks none — and for a failure that is NOT a refusal (a body
/// the codec cannot read at all, a member of the wrong type), which the framework's own
/// walk answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_lists`: one read of the request
/// yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input) with each value echo
/// restored to the bytes the caller sent, plus the request's own lists — the value a clause
/// states at a LIST member. Nothing is re-checked here, and no second copy of that walk
/// lives in this crate: both halves are bounded where those bounds are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    if violations.is_empty() {
        return None;
    }
    let stated = Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: constraint_message(&violations, &lists)?,
    };
    Some((violations, stated))
}

impl<O> ErasedOperation for StatedPerOperation<O>
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
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated));
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`StatedPerOperation`]
        // so a refused request is worded with the request's own lists in hand. A key the
        // model does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. It is handed the violations ALONE — no request — so a
        // clause stated at a LIST member has only the REFUSED elements of that list to
        // quote; `StatedPerOperation` answers a modelled operation's refusals before this
        // walk is reached, with the list the caller sent in hand. A refusal that gets here
        // anyway is still stated by the SAME wording, so the two paths cannot differ on
        // anything but how much of a list's value is known.
        //
        // The differences from the framework's own wording are the requirement a
        // `@pattern` violation states — the expression without the model's outer
        // anchors (see [`unanchored`]) — and the member a constraint broken by a
        // list's ELEMENT is attributed to (see [`constraint_message`]). The count, the
        // clause form, the echoed value and the envelope stay the framework's, and the
        // code stays the modelled `ValidationException` (HTTP 400) the framework would
        // have answered.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `@pattern` this service's model declares, paired with the expression a
    /// requirement must name: the modelled spelling with its OUTER anchors removed.
    /// Source: `smithy_query(query="shape", shape="AppRegistryArn,ApplicationId,\
    /// ComponentId,DatabaseId,NextToken,OperationId,SAPInstanceNumber,SID,SsmSapArn,\
    /// TagKey", model_path="services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/\
    /// com-amazonaws-ssmsap-ssmsap.json")`.
    const MODELLED_PATTERNS: &[(&str, &str, &str)] = &[
        ("ApplicationId", r"^[\w\d\.-]+$", r"[\w\d\.-]+"),
        ("ComponentId", r"^[\w\d-]+$", r"[\w\d-]+"),
        (
            "AppRegistryArn",
            r"^arn:aws:servicecatalog:[a-z0-9:\/-]+$",
            r"arn:aws:servicecatalog:[a-z0-9:\/-]+",
        ),
        ("NextToken", r"^.{16,2048}$", r".{16,2048}"),
        (
            "OperationId",
            r"^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$",
            r"[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?",
        ),
        ("SID", r"^[A-Z][A-Z0-9]{2}$", r"[A-Z][A-Z0-9]{2}"),
        ("SAPInstanceNumber", r"^[0-9]{2}$", r"[0-9]{2}"),
        (
            "TagKey",
            r"^(?!aws:)[a-zA-Z+-=._:/]+$",
            r"(?!aws:)[a-zA-Z+-=._:/]+",
        ),
        // `DatabaseId` spells only the end anchor, which is still an outer one.
        ("DatabaseId", r"[\w\d]+$", r"[\w\d]+"),
        // Only the OUTER anchors go: the `$` and `^` either side of `SsmSapArn`'s
        // alternation belong to its branches, so they are part of the expression.
        (
            "SsmSapArn",
            r"^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$",
            r"arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+",
        ),
    ];

    #[test]
    fn every_modelled_pattern_is_reported_without_its_outer_anchors() {
        for (shape, modelled, expected) in MODELLED_PATTERNS {
            let reported = unanchored(modelled);
            assert_eq!(
                reported, *expected,
                "case {shape:?}: expected the requirement to name {expected:?}, got \
                 {reported:?}"
            );
        }
    }

    #[test]
    fn an_escaped_trailing_dollar_is_a_literal_and_stays() {
        // A `$` preceded by an odd number of backslashes is a dollar sign inside the
        // expression rather than the end anchor; one past an even number is the anchor.
        let cases: &[(&str, &str)] = &[
            (r"^cost\$", r"cost\$"),
            (r"^cost\$$", r"cost\$"),
            (r"^cost\\$", r"cost\\"),
            ("^$", ""),
        ];
        for (modelled, expected) in cases {
            let reported = unanchored(modelled);
            assert_eq!(
                reported, *expected,
                "case {modelled:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }

    #[test]
    fn only_the_pattern_requirement_differs_from_the_frameworks_own_wording() {
        let pattern = Violation {
            path: "applicationId".to_string(),
            kind: ViolationKind::Pattern(r"^[\w\d\.-]+$".to_string()),
            value: Some("bad app id!".to_string()),
        };
        let stated = constraint_envelope(std::slice::from_ref(&pattern))
            .expect("one clause is stated");
        assert_eq!(
            stated,
            "1 validation error detected: Value 'bad app id!' at 'applicationId' failed \
             to satisfy constraint: Member must satisfy regular expression pattern: \
             [\\w\\d\\.-]+",
            "case \"a pattern clause\": got {stated:?}"
        );

        // Every other class keeps the sentence the framework (and AWS) states, which
        // `Violation::message` renders.
        for kind in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(60),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("50".to_string()),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "applicationId".to_string(),
                kind: kind.clone(),
                value: match kind {
                    ViolationKind::MissingRequiredMember => None,
                    _ => Some("x".to_string()),
                },
            };
            let stated = constraint_envelope(std::slice::from_ref(&violation))
                .expect("one clause is stated");
            let framework =
                aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(
                    &violation,
                ))
                .expect("the framework states it");
            assert_eq!(
                stated, framework,
                "case {kind:?}: expected the framework's own wording {framework:?}, got \
                 {stated:?}"
            );
        }

        assert!(
            constraint_envelope(&[]).is_none(),
            "case \"no violations\": there is nothing to state"
        );
    }

    /// A violation of `kind` at `path` holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
        }
    }

    /// The count and the join are the framework's, for every clause count a refusal can
    /// carry — pinned against the framework's own renderer, which [`envelope`] cannot call
    /// because a nested element requirement is no [`ViolationKind`].
    #[test]
    fn the_envelope_counts_and_joins_clauses_the_way_the_framework_does() {
        for count in 0_usize..4 {
            let violations: Vec<Violation> = (0..count)
                .map(|index| {
                    violation(
                        &format!("member{index}"),
                        ViolationKind::MaxLength(8),
                        Some("too long a value"),
                    )
                })
                .collect();
            let framework = aws_smithy_schema_server::constraints::envelope(&violations);
            let clauses: Vec<String> = violations.iter().map(Violation::message).collect();
            let mine = envelope(&clauses);
            assert_eq!(
                mine, framework,
                "case {count} clause(s): expected the framework's envelope {framework:?}, \
                 got {mine:?}"
            );
        }
    }

    /// A constraint broken by an ELEMENT of a list is stated at the LIST member, whatever
    /// the constraint class — and a failure that is NOT on an element keeps its own path.
    #[test]
    fn an_elements_failure_is_stated_at_its_list_with_the_requirement_nested() {
        // (case, violation, the whole expected message)
        let rows = [
            (
                "an element outside its enum value set",
                violation(
                    "configurationCheckIds.1.member",
                    ViolationKind::EnumValue(vec![
                        "SAP_CHECK_01".to_string(),
                        "SAP_CHECK_02".to_string(),
                        "SAP_CHECK_03".to_string(),
                    ]),
                    Some("SAP_CHECK_NOT_A_REAL_ID"),
                ),
                "1 validation error detected: Value '[SAP_CHECK_NOT_A_REAL_ID]' at \
                 'configurationCheckIds' failed to satisfy constraint: Member must satisfy \
                 constraint: [Member must satisfy enum value set: [SAP_CHECK_01, \
                 SAP_CHECK_02, SAP_CHECK_03]]",
            ),
            (
                "an element outside its pattern, at a position past the first",
                violation(
                    "instances.3.member",
                    ViolationKind::Pattern(r"^i-[\w\d]{8}$|^i-[\w\d]{17}$".to_string()),
                    Some("not-an-instance"),
                ),
                "1 validation error detected: Value '[not-an-instance]' at 'instances' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy regular expression pattern: i-[\\w\\d]{8}$|^i-[\\w\\d]{17}]",
            ),
            (
                "an element shorter than its minimum",
                violation("tagKeys.1.member", ViolationKind::MinLength(1), Some("")),
                "1 validation error detected: Value '[]' at 'tagKeys' failed to satisfy \
                 constraint: Member must satisfy constraint: [Member must have length \
                 greater than or equal to 1]",
            ),
            (
                // The LIST's own constraint, not an element's: stated at the same path and
                // NOT nested.
                "the list's own length",
                violation("instances", ViolationKind::MaxLength(1), Some("2 items")),
                "1 validation error detected: Value '2 items' at 'instances' failed to \
                 satisfy constraint: Member must have length less than or equal to 1",
            ),
            (
                // A member INSIDE a structure element keeps its own full path.
                "a member of a structure element",
                violation("filters.1.member.name", ViolationKind::MinLength(1), Some("")),
                "1 validation error detected: Value '' at 'filters.1.member.name' failed to \
                 satisfy constraint: Member must have length greater than or equal to 1",
            ),
            (
                // A map entry's step carries the caller's key, not a position.
                "a map entry value",
                violation(
                    "tags.someKey.member",
                    ViolationKind::MaxLength(256),
                    Some("over long"),
                ),
                "1 validation error detected: Value 'over long' at 'tags.someKey.member' \
                 failed to satisfy constraint: Member must have length less than or equal \
                 to 256",
            ),
            (
                "an absent required member",
                violation("applicationId", ViolationKind::MissingRequiredMember, None),
                "1 validation error detected: Value null at 'applicationId' failed to \
                 satisfy constraint: Member must not be null",
            ),
        ];

        for (case, violation, expected) in rows {
            let stated = constraint_envelope(std::slice::from_ref(&violation))
                .unwrap_or_else(|| panic!("case {case:?}: a constraint refusal is stated"));
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// The value a clause states at a list member is the WHOLE list when the request's
    /// lists are in hand, and the refused elements when only the violations are — and two
    /// elements breaking ONE rule are one clause, not two.
    #[test]
    fn a_list_clause_states_the_whole_list_when_it_is_in_hand() {
        let allowed = ViolationKind::EnumValue(vec!["SAP_CHECK_01".to_string()]);
        let violations = vec![
            violation(
                "configurationCheckIds.2.member",
                allowed.clone(),
                Some("NOPE_ONE"),
            ),
            violation(
                "configurationCheckIds.3.member",
                allowed,
                Some("NOPE_TWO"),
            ),
        ];
        let lists: HashMap<String, String> = [(
            "configurationCheckIds".to_string(),
            "[SAP_CHECK_01, NOPE_ONE, NOPE_TWO]".to_string(),
        )]
        .into_iter()
        .collect();

        let with_lists =
            constraint_message(&violations, &lists).expect("a constraint refusal is stated");
        assert_eq!(
            with_lists,
            "1 validation error detected: Value '[SAP_CHECK_01, NOPE_ONE, NOPE_TWO]' at \
             'configurationCheckIds' failed to satisfy constraint: Member must satisfy \
             constraint: [Member must satisfy enum value set: [SAP_CHECK_01]]",
            "case \"the request's lists in hand\": got {with_lists:?}"
        );

        let without = constraint_envelope(&violations).expect("a constraint refusal is stated");
        assert_eq!(
            without,
            "1 validation error detected: Value '[NOPE_ONE, NOPE_TWO]' at \
             'configurationCheckIds' failed to satisfy constraint: Member must satisfy \
             constraint: [Member must satisfy enum value set: [SAP_CHECK_01]]",
            "case \"the violations alone\": got {without:?}"
        );
    }

    /// A path that keeps looking like an element step nests at most [`MAX_ELEMENT_DEPTH`]
    /// times, so the nesting a request can ask for is bounded.
    #[test]
    fn the_element_nesting_is_bounded() {
        let deep = format!("outer{}", ".1.member".repeat(8));
        let (path, depth) = collapsed(&deep);
        assert_eq!(
            depth, MAX_ELEMENT_DEPTH,
            "case {deep:?}: expected at most {MAX_ELEMENT_DEPTH} steps out, got {depth}"
        );
        assert!(
            path.starts_with("outer"),
            "case {deep:?}: expected the remaining path to start at the outermost member, \
             got {path:?}"
        );
        let stated = constraint_envelope(&[violation(
            &deep,
            ViolationKind::MinLength(1),
            Some(""),
        )])
        .expect("a constraint refusal is stated");
        assert_eq!(
            stated.matches(ELEMENT_CONSTRAINT_OPEN).count(),
            MAX_ELEMENT_DEPTH,
            "case {deep:?}: expected {MAX_ELEMENT_DEPTH} nested requirements, got {stated:?}"
        );
    }

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is JSON no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-ssmsap-ssmsap.json"
    );

    /// The whole surface: every operation the model declares is served behind
    /// [`StatedPerOperation`], under the same key the generated routing table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `stated_operations!` fails here instead of silently
    /// answering a list-element refusal without the request's lists in hand.
    #[test]
    fn every_modelled_operation_is_served_with_this_services_refusal_wording() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert_eq!(
            operations.len(),
            27,
            "case \"the model's surface\": expected this service's 27 operations, got \
             {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from stated_operations!, so a \
                 list-element refusal of it quotes only the refused elements"
            );
        }
    }
}
