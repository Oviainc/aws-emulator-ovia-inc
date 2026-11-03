// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-managedblockchain-taigawebservice` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the routing
//!   key and the handler stay the generated table's, but the operation is served
//!   through [`InModelledEnvelope`], so a request refused before the handler runs is
//!   refused with a shape this service's model declares.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: a request outside a modelled constraint is
//!   `InvalidRequestException`, worded from the violations the framework collected.
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
//! # How this service refuses a bad request
//!
//! Every operation of this service lists `InvalidRequestException` ("The action or
//! operation requested is invalid. Verify that the action is typed correctly", HTTP
//! 400) among its errors — the model binds it, together with
//! `InternalServiceErrorException`, to the SERVICE rather than to one operation, so it
//! is the client-input error of all 27 of them. It is therefore the shape a request
//! this service cannot accept is refused with, whether the handler finds the fault or
//! the framework's boundary walk does: a shape the model does not declare has no
//! variant in a client's generated error union, so a typed catch never fires on it and
//! the failure surfaces as an unknown error.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The error code this service names for a request it refused as invalid: the shape
/// `com.amazonaws.managedblockchain#InvalidRequestException`, which the model binds to
/// the service (so every operation lists it) and whose only prose member it calls
/// `Message`
/// (`smithy_query(query="shape", shape="com.amazonaws.managedblockchain#InvalidRequestException")`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The HTTP status that shape's `@httpError` declares.
const INVALID_REQUEST_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-managedblockchain-taigawebservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before its handler runs is refused
    /// with this service's own `InvalidRequestException`. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
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
        wrap_operations!(
            in_modelled_envelope,
            engine,
            create_accessor::CreateAccessor,
            create_member::CreateMember,
            create_network::CreateNetwork,
            create_node::CreateNode,
            create_proposal::CreateProposal,
            delete_accessor::DeleteAccessor,
            delete_member::DeleteMember,
            delete_node::DeleteNode,
            get_accessor::GetAccessor,
            get_member::GetMember,
            get_network::GetNetwork,
            get_node::GetNode,
            get_proposal::GetProposal,
            list_accessors::ListAccessors,
            list_invitations::ListInvitations,
            list_members::ListMembers,
            list_networks::ListNetworks,
            list_nodes::ListNodes,
            list_proposal_votes::ListProposalVotes,
            list_proposals::ListProposals,
            list_tags_for_resource::ListTagsForResource,
            reject_invitation::RejectInvitation,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_member::UpdateMember,
            update_node::UpdateNode,
            vote_on_proposal::VoteOnProposal,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated in this service's
/// own `InvalidRequestException`.
///
/// The framework's pre-dispatch renderer gets the status right (400) but names the
/// synthetic `smithy.framework#ValidationException` — a shape this service's model does
/// not declare, so an SDK finds no variant for it — and writes the prose into a
/// framework-chosen lowercase `message` key, while every error shape of this service
/// calls its prose member `Message`. So that envelope tells a client neither which
/// modelled failure it is nor what was wrong with the request.
///
/// This wrapper collects the refusal itself — the very two classes the framework's own
/// walk collects, from the framework's own deserializer and constraint walk — and
/// renders it through the model's `InvalidRequestException` schema. The envelope is
/// built from the structured [`Violation`]s, never from a rendered response. Nothing
/// else is re-implemented: a request this service does not refuse is handed to the
/// generated `ErasedOperation::run`, which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error.
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
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = invalid_request_exception(protocol, &violations) {
                return refusal;
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

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a declared constraint. Empty when the request is valid,
/// and empty for a failure that is NOT a refusal — bytes the codec cannot read at all
/// are a serialization failure, a different class with a different remedy, which the
/// framework's own tail still renders.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), so this cannot find a different
/// set of violations from the walk that follows it.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking
        // what was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations`: HTTP 400 `InvalidRequestException` with the
/// prose in the `Message` member the model declares, serialized through that shape's
/// own schema. `None` when there is nothing to refuse — and also when the error cannot
/// be serialized, which leaves the framework's walk to refuse the request in its own
/// envelope rather than answering a 400 with no reason at all.
fn invalid_request_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::ops::types::shared_types::InvalidRequestException {
        message: Some(stated(violations)?),
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

/// How this service words a refusal: one sentence per refused member, naming the member
/// and what the request held for it — `Status NOT_A_REAL_STATUS is not valid.` — which
/// is how Managed Blockchain is recorded to answer a query filter outside its value
/// set, and the only thing a caller can act on. A member the request omitted has no
/// value to echo, so its sentence states the requirement instead.
///
/// `None` when `violations` is empty: there is nothing to refuse.
fn stated(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    Some(
        violations
            .iter()
            .map(sentence)
            .collect::<Vec<String>>()
            .join(" "),
    )
}

/// One violation as this service states it.
///
/// A value outside the SPACE its member's ids are drawn from is stated as MALFORMED —
/// `AccessorId parameter <value> is malformed` — whatever modelled constraint it also
/// happens to break, because that is the fault: a value the service could never have
/// issued for that member. Wording it from the constraint it tripped over instead would
/// tell a caller to shorten a 36-character UUID, when no 32-character UUID is an id of
/// that space either. The decision and the wording are both
/// [`crate::state::malformed_id_sentence`]'s — the same function the handler that looks
/// such an id up refuses through — so the boundary and the handler cannot disagree, and
/// this stays a formatter: it asks whether there is a malformed-id sentence to say and
/// otherwise words the violation as the class it is.
///
/// A value outside an ENUMERATED set gets the sentence alone: the permitted set is in
/// the model, and so in every generated client, and spelling the whole set into prose
/// would say nothing the caller cannot already read. Every other class failed a BOUND or
/// a PATTERN the caller cannot read off a value set, so the sentence is followed by the
/// framework's own clause for that class, which states the member, the value and the
/// rule it broke. Taking the clause from the framework rather than writing one here is
/// deliberate: constraint prose stays single-homed, so a constraint class this service
/// has never seen cannot arrive with a second spelling of its requirement.
fn sentence(violation: &Violation) -> String {
    let member = as_modelled(&violation.path);
    // A member the request omitted has no value to echo, so its sentence states the
    // requirement instead.
    let Some(value) = violation.value.as_deref() else {
        return format!("{member} is required.");
    };
    if let Some(malformed) = crate::state::malformed_id_sentence(&member, value) {
        return malformed;
    }
    match &violation.kind {
        ViolationKind::EnumValue(_) => format!("{member} {value} is not valid."),
        _ => format!("{member} {value} is not valid. {}", violation.message()),
    }
}

/// A member path as the MODEL names it. Every member of this service's shapes is named
/// in UpperCamel (`NetworkId`, `MaxResults`, `Status`), and the framework spells a path
/// in lowerCamel, so the leading letter is restored here.
///
/// A dotted path is left exactly as the framework spelled it: a segment of one can be a
/// map key the CALLER chose (a tag key), which is echoed as sent and never re-cased the
/// way a model name is.
fn as_modelled(path: &str) -> String {
    if path.contains('.') {
        return path.to_string();
    }
    let mut chars = path.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => path.to_string(),
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request it refuses before the handler runs is
        // refused with this service's `InvalidRequestException` rather than in the
        // framework's pre-dispatch envelope, which names a shape this model does not
        // declare and carries no `Message`. A key the model does not name resolves to
        // `None` here exactly as it does in the generated table, so the router's
        // routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam is what words any refusal the FRAMEWORK still renders
        // — a request that reached no wrapped operation, or an error this service could
        // not serialize. Wording it here from the same violations keeps the two paths
        // from disagreeing: a request outside a declared constraint is this service's
        // `InvalidRequestException` either way.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_REQUEST_EXCEPTION.to_string(),
                message: stated(violations)?,
            }),
            // Authentication is the framework's to word: this service models
            // `AccessDeniedException` for a caller it refuses, and the request never
            // reached an operation, so there is nothing of this service's own to say.
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
