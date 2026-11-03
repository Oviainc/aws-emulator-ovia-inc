// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-amp-amazonprometheusservice` service: how it claims requests, how it words the
//! failures the framework hands back, and the media type it announces.
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
//!   correctly. THIS service uses it to put the response envelope described below in
//!   front of every modelled operation, and lets the generated routing table serve the
//!   request from there.
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
//! ## The response envelope this service puts in front of every operation
//!
//! [`ApsResponse`] is a gate [`Service::override_operation`] places in front of each
//! modelled operation. It changes nothing about what an operation DOES; it owns the two
//! facts that are true of every aps response and that the generated walk cannot state:
//!
//! 1. **The media type.** The model declares `restJson1`, whose serializer announces
//!    `application/json` on every JSON body, but every recorded aps response — a 200, a
//!    400 and a 404 alike — carries [`crate::state::RESPONSE_CONTENT_TYPE`]. The body is
//!    the same JSON either way, so [`announced`] replaces that one header on the way out
//!    and leaves a response that is NOT the JSON envelope (a `@httpPayload` blob's
//!    `application/octet-stream`, a string payload's `text/plain`) announcing its own
//!    type. `Service::override_protocol` is not the seam for it: the codec is right, and
//!    swapping in another protocol would swap the URI-based routing this REST service
//!    needs along with it.
//! 2. **The shape of a refused request.** `ValidationException` models `reason`
//!    (REQUIRED) and `fieldList` beside `message`, and the framework validates every
//!    modelled constraint — a `@pattern`, a `@length`, a `@range`, an enum — BEFORE a
//!    handler runs, rendering a violation as the protocol's generic refusal body, which
//!    carries `message` alone. A client deserializing that error is handed null for a
//!    member the model marks required, so branching on `reason` and reporting per-field
//!    errors from `fieldList` are both impossible. The gate therefore asks the
//!    framework's OWN constraint walk
//!    (`aws_smithy_schema_server::constraints::validate`, the same call the centralized
//!    sequence makes) what the request violates and states it through
//!    [`crate::state::constraint_validation_failed`] — the one composition this
//!    service's handlers also refuse through, so a rule the model declares and a rule a
//!    handler enforces read alike. Nothing about a constraint is restated here: the
//!    rules enforced are exactly the model's, and only the BODY of the refusal changes.
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the refusals
//! that happen in FRONT of the service — a request naming no modelled operation, a
//! malformed `aws-chunked` body, an unreadable body the codec cannot turn into a value
//! at all. Those are not responses this service produced, so they keep the framework's
//! protocol-correct rendering.
//!
//! ## The one operation served off its own input binding
//!
//! `ListScrapers` is fronted by [`ListScrapersFromQuery`] instead, for the reason
//! `crate::ops::list_scrapers::input_from_query` states: its `filters` member is
//! `@httpQueryParams` over a map of value LISTS, which the framework's HTTP binding
//! cannot read, so every request carrying a query parameter would be refused 400 before
//! reaching the handler. That gate binds the input the model's way and drives the same
//! deserialize-equivalent -> validate -> handle -> serialize sequence by hand; the
//! operation's behaviour still lives in its own file.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{validate, Violation};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationError, Refusal, Refused, ServerHttpProtocol, Service,
};

/// `ValidationException`'s wire error code — the shape name, which restJson1 puts in
/// `x-amzn-errortype` and in the body's `__type`.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// `ValidationException`'s modelled status (`httpError` 400 on
/// `com.amazonaws.amp#ValidationException`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-amp-amazonprometheusservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The [`ApsResponse`] gate for each modelled operation, keyed by the routing key the
    /// protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    response_envelope: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            response_envelope: response_envelopes::<Eng>(engine),
        }
    }

    /// The routing keys this service's response envelope fronts, sorted.
    ///
    /// Public for ONE reason: `OperationRoutingTable` exposes no iterator over its keys,
    /// so nothing else can check that the envelope covers every operation the model
    /// binds — and an operation it missed would answer in the `restJson1` default media
    /// type instead of this service's, quietly. `tests/all/wire_fidelity.rs` compares
    /// this list against the operations `OPERATIONS.md` renders from the model.
    pub fn announced_operations(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.response_envelope.keys().copied().collect();
        names.sort_unstable();
        names
    }
}

/// One [`ApsResponse`] gate per modelled operation.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
fn response_envelopes<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(ApsResponse::<crate::ops::$module::$operation<Eng>>::new())
                        as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation the model binds. The media type is a property of the SERVICE and a
    // constraint is a property of the INPUT SHAPE, so the gate goes in front of all of
    // them, implemented or still scaffolded: a 501 from a scaffolded operation announces
    // the same media type as a 200 from an implemented one, and a request that violates
    // the model is refused the same way whoever serves it.
    gate!(
        gates;
        create_alert_manager_definition::CreateAlertManagerDefinition,
        create_anomaly_detector::CreateAnomalyDetector,
        create_logging_configuration::CreateLoggingConfiguration,
        create_query_logging_configuration::CreateQueryLoggingConfiguration,
        create_rule_groups_namespace::CreateRuleGroupsNamespace,
        create_scraper::CreateScraper,
        create_workspace::CreateWorkspace,
        delete_alert_manager_definition::DeleteAlertManagerDefinition,
        delete_anomaly_detector::DeleteAnomalyDetector,
        delete_logging_configuration::DeleteLoggingConfiguration,
        delete_query_logging_configuration::DeleteQueryLoggingConfiguration,
        delete_resource_policy::DeleteResourcePolicy,
        delete_rule_groups_namespace::DeleteRuleGroupsNamespace,
        delete_scraper::DeleteScraper,
        delete_scraper_logging_configuration::DeleteScraperLoggingConfiguration,
        delete_workspace::DeleteWorkspace,
        describe_alert_manager_definition::DescribeAlertManagerDefinition,
        describe_anomaly_detector::DescribeAnomalyDetector,
        describe_logging_configuration::DescribeLoggingConfiguration,
        describe_query_logging_configuration::DescribeQueryLoggingConfiguration,
        describe_resource_policy::DescribeResourcePolicy,
        describe_rule_groups_namespace::DescribeRuleGroupsNamespace,
        describe_scraper::DescribeScraper,
        describe_scraper_logging_configuration::DescribeScraperLoggingConfiguration,
        describe_workspace::DescribeWorkspace,
        describe_workspace_configuration::DescribeWorkspaceConfiguration,
        get_default_scraper_configuration::GetDefaultScraperConfiguration,
        list_anomaly_detectors::ListAnomalyDetectors,
        list_rule_groups_namespaces::ListRuleGroupsNamespaces,
        list_tags_for_resource::ListTagsForResource,
        list_workspaces::ListWorkspaces,
        put_alert_manager_definition::PutAlertManagerDefinition,
        put_anomaly_detector::PutAnomalyDetector,
        put_resource_policy::PutResourcePolicy,
        put_rule_groups_namespace::PutRuleGroupsNamespace,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_logging_configuration::UpdateLoggingConfiguration,
        update_query_logging_configuration::UpdateQueryLoggingConfiguration,
        update_scraper::UpdateScraper,
        update_scraper_logging_configuration::UpdateScraperLoggingConfiguration,
        update_workspace_alias::UpdateWorkspaceAlias,
        update_workspace_configuration::UpdateWorkspaceConfiguration,
    );
    // `ListScrapers` is the one operation the generated walk cannot serve: the framework
    // cannot read its `@httpQueryParams` map of value lists. Its gate binds the input
    // itself and calls the handler directly — see [`ListScrapersFromQuery`].
    gates.insert(
        <crate::ops::list_scrapers::ListScrapers<Eng> as Operation>::NAME,
        Box::new(ListScrapersFromQuery::new(engine)) as Box<dyn ErasedOperation>,
    );
    gates
}

/// This service's response envelope in front of one operation `Op`: the media type every
/// aps response announces, and the modelled shape of a refused request. See the module
/// docs for both.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape and the schema the
/// constraint walk runs against. It holds no engine and no state, so the operation it
/// fronts stays the single owner of the request's behaviour.
struct ApsResponse<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ApsResponse<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ApsResponse<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            if let Some(refused) = refuse_invalid_input::<Op>(protocol, req) {
                return announced(refused);
            }
            // Valid: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse). Reached through the routing table rather than
            // through `override_operation`, which would route straight back here.
            let response = match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            };
            announced(response)
        })
    }
}

/// Serves `ListScrapers` off the input binding in
/// `crate::ops::list_scrapers::input_from_query`, because the framework cannot read that
/// operation's `@httpQueryParams` map of value LISTS (see the module docs and that
/// function).
///
/// It drives the SAME sequence the centralized walk does — bind the input, validate the
/// model's constraints, call the handler, serialize the output or the error through this
/// service's protocol — and nothing else: the behaviour is still the operation's own
/// `handle`. Unlike the generated walk it records no per-operation telemetry, exactly as
/// any other author-written operation served from this seam does not.
struct ListScrapersFromQuery<Eng> {
    operation: crate::ops::list_scrapers::ListScrapers<Eng>,
}

impl<Eng> ListScrapersFromQuery<Eng> {
    /// Built from the SAME shared engine the generated entry threads into every
    /// operation, so this gate and the rest of the service read one store.
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: crate::ops::list_scrapers::ListScrapers::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ListScrapersFromQuery<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        type Op<Eng> = crate::ops::list_scrapers::ListScrapers<Eng>;
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let query = req.uri().split_once('?').map_or("", |(_, query)| query);
            let input = match crate::ops::list_scrapers::input_from_query(query) {
                Ok(input) => input,
                // A member the request carries that cannot become its modelled type:
                // stated in the same modelled `ValidationException` as a constraint
                // violation, since both are the caller's to fix.
                Err(refused) => {
                    return announced(modelled_validation_response(
                        svc,
                        req,
                        protocol,
                        &refused,
                        <Op<Eng> as Operation>::NAME,
                    ))
                }
            };
            // The model's own constraints, checked by the framework's own walk — the
            // same call the centralized sequence makes for every other operation.
            let violations = validate(&input, <Op<Eng> as Operation>::INPUT_SCHEMA);
            if !violations.is_empty() {
                let refused = crate::state::constraint_validation_failed(&violations);
                return announced(modelled_validation_response(
                    svc,
                    req,
                    protocol,
                    &refused,
                    <Op<Eng> as Operation>::NAME,
                ));
            }
            let response = match self.operation.handle(input, ctx).await {
                Ok(output) => protocol.serialize_output(
                    &output,
                    <Op<Eng> as Operation>::OUTPUT_SCHEMA,
                    <Op<Eng> as Operation>::NAME,
                    <Op<Eng> as Operation>::SUCCESS_STATUS,
                ),
                Err(error) => protocol.serialize_error(
                    error.as_serializable(),
                    error.variant_schema(),
                    error.shape_name(),
                    error.http_status(),
                ),
            };
            announced(response.unwrap_or_else(|error| {
                svc.definition().framework_error_response(error, req)
            }))
        })
    }
}

/// The response as this service announces it: one media type for every JSON body it
/// produces, successes and failures alike.
///
/// Only the `restJson1` JSON envelope is re-announced
/// ([`crate::state::JSON_CONTENT_TYPE`] -> [`crate::state::RESPONSE_CONTENT_TYPE`]); a
/// body that is not that envelope carries the type its own target shape maps to and is
/// left exactly as the serializer produced it.
fn announced(mut response: RtResponse) -> RtResponse {
    if response.headers().get("content-type") == Some(crate::state::JSON_CONTENT_TYPE) {
        response
            .headers_mut()
            .insert("content-type", crate::state::RESPONSE_CONTENT_TYPE);
    }
    response
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// `Some(response)` when `Op`'s input violates a constraint the model declares: this
/// service's modelled `ValidationException`, 400, with `reason`, `fieldList` and a
/// `message` naming the first refused member and the requirement it failed.
///
/// `None` when the request satisfies the model, or when it failed to READ for a reason
/// that is not a constraint violation — an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse> {
    let violations = violations_of::<Op>(protocol, req);
    if violations.is_empty() {
        return None;
    }
    let error = crate::state::constraint_validation_failed(&violations);
    serialize_validation(protocol, &error)
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

/// This service's modelled `ValidationException` as a 400 response, for a gate that has
/// no generated walk to fall back to: a modelled error that will not serialize is
/// rendered in the entry's own framework envelope instead, and logged.
fn modelled_validation_response(
    svc: &dyn Service,
    req: &RtRequest,
    protocol: &dyn ServerHttpProtocol,
    error: &crate::ops::types::ValidationException,
    operation: &str,
) -> RtResponse {
    serialize_validation(protocol, error).unwrap_or_else(|error| {
        tracing::warn!(
            operation,
            %error,
            "the modelled ValidationException would not serialize; this refusal falls \
             back to the framework's generic envelope"
        );
        svc.definition().framework_error_response(error, req)
    })
}

/// `ValidationException` serialized in this service's protocol: the modelled status, the
/// modelled shape and the error code restJson1 resolves an error shape from.
fn serialize_validation(
    protocol: &dyn ServerHttpProtocol,
    error: &crate::ops::types::ValidationException,
) -> Result<RtResponse, ProtocolError> {
    protocol.serialize_error(
        error,
        &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
        VALIDATION_EXCEPTION_CODE,
        VALIDATION_EXCEPTION_STATUS,
    )
}

/// Every constraint `Op`'s input violates, as the framework's own walk reports them.
///
/// Both validation classes are classified by the framework's own
/// [`smithy_server_runtime::refusal_of`], so this carries no second copy of "what counts
/// as a constraint refusal": a value outside a declared bound is found by walking what
/// was read, and a required member absent from the request is found while READING it, and
/// `refusal_of` already states that they are one refusal. Anything it does not recognise
/// (a body the codec cannot read at all, a missing header) is not a constraint refusal
/// and is left to the framework, which words it per protocol.
fn violations_of<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => return validate(&input, Op::INPUT_SCHEMA),
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error) {
        Some(Refusal::Invalid(violations)) => violations,
        // Authentication is settled by the router before any operation runs, and a body
        // that is not readable at all names no member — neither is a constraint
        // violation. `Refusal` is `#[non_exhaustive]`, so a class added later lands here
        // too and keeps the framework's own wording.
        _ => Vec::new(),
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
        // Put this service's response envelope in front of the modelled operation the
        // request names — the media type every aps response announces, and the modelled
        // `ValidationException` a request outside the model's constraints is refused
        // with (see the module docs). The gate then hands every request the model accepts
        // to the generated operation, which the routing table still owns; a request that
        // names no modelled operation resolves as it always did, which is what leaves it
        // to `override_routing_error`.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.response_envelope.get(key)?;
        Some(gate.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Nothing to reword here. A refusal the framework finds in a request to a
        // MODELLED operation never reaches this seam: `ApsResponse` answers a constraint
        // violation before the framework renders it, because the words are not the
        // problem — the generic refusal body cannot carry the modelled `reason` and
        // `fieldList` at all, and `Refused` is only a code and a message. What is left
        // (a body the codec cannot read, an unauthenticated request) is refused in front
        // of this service rather than produced by it, so it keeps the framework's
        // wording.
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
