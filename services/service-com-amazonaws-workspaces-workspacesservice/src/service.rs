// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-workspaces-workspacesservice` service: how it claims requests and how it words the
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
//! # The account-link operations' refusal wording
//!
//! WorkSpaces' account-link API — the six operations whose model declares
//! `ValidationException` — states a refused request by its ERROR CLASS and not by
//! the request: recorded responses show three of those operations refusing two
//! different members, each with a value of its own, all answering the single
//! sentence [`crate::state::ACCOUNT_LINK_INVALID_REQUEST_MESSAGE`], which names
//! neither the member, nor the value, nor the constraint.
//!
//! A modelled constraint is checked BEFORE a handler runs, so that refusal is not
//! something a handler can word: the framework words it through
//! [`Service::override_refusal`], which is handed the violations ALONE — no
//! operation — and this service's OTHER ~85 operations keep the framework's own
//! `N validation error detected: …` envelope (`ModifyAccount`'s CIDR range,
//! `CreateWorkspaces`' bundle ids), which is the wording AWS's older WorkSpaces
//! surface answers with. The account-link family therefore cannot be told apart at
//! that seam, and [`AccountLinkRefusal`] is where the distinction lives: those six
//! operations are served wrapped in it (through the `override_operation` seam
//! below), and the wrapper refuses a request that breaks a modelled constraint in
//! the account-link API's own words before the operation runs.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::{constraints, ProtocolError};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code an account-link constraint refusal carries: the
/// `ValidationException` the model declares for every one of those operations,
/// and the code the framework's own default picks for a constraint refusal on
/// this service's awsJson1_1 envelope — supplied beside the chosen message
/// because [`Refused`] replaces the code and the message TOGETHER.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `com-amazonaws-workspaces-workspacesservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The six account-link operations the generated table holds, each wrapped in
    /// [`AccountLinkRefusal`] so a request that breaks a modelled constraint is
    /// refused in that API's own words before the operation runs. Keyed by the
    /// routing key the protocol resolves — the key the generated table uses — and
    /// built from the SAME shared engine, so a wrapped operation reads and writes
    /// the one store.
    account_link: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped
/// in [`AccountLinkRefusal`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(AccountLinkRefusal::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// One account-link operation, served behind that API's own refusal wording.
///
/// The framework refuses a modelled constraint before any handler runs and words it
/// from the violations alone. A wrapper is handed the request AND the operation, so
/// it can word that refusal the way the operation's own API does: every violation of
/// this operation's modelled input — a `@required` member absent, a value outside a
/// declared `@pattern`, `@length`, `@range` or enum — is answered as
/// `ValidationException` with the class's one sentence
/// ([`crate::state::ACCOUNT_LINK_INVALID_REQUEST_MESSAGE`]), naming neither the
/// member, nor the value, nor the constraint.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed
/// to the generated [`ErasedOperation::run`], which owns deserialize -> validate ->
/// handle -> serialize, telemetry and every modelled error — including the refusals
/// the handler itself owns, which carry the same fixed text per error class.
///
/// The violations come from the framework's own machinery — the protocol's own
/// deserializer and `constraints::validate` over the built input ([`violations_in`])
/// — so this cannot find a different set from the walk that follows it.
struct AccountLinkRefusal<O> {
    operation: O,
}

impl<O> AccountLinkRefusal<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for AccountLinkRefusal<O>
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
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across
            // the await below would make this future non-`Send`.
            let refused = violations_in::<O>(svc.definition().protocol(), req);
            if !refused.is_empty() {
                return account_link_refusal(svc, req, refused);
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must
    /// not change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every modelled constraint `req` breaks, for the operation `O`.
///
/// Both classes the framework refuses before a handler runs are collected: a value
/// outside a declared `@pattern`, `@length`, `@range` or enum, found by walking the
/// input that was read, and a `@required` member ABSENT, found while READING it. To a
/// caller they are one refusal — the framework's own `refusal_of` folds the absent
/// member into exactly the violation built below — so they are one here. Any OTHER
/// codec failure (a body the codec cannot read at all, a member of the wrong type) is
/// not a constraint violation and is left to the framework's own walk, which refuses
/// it in its own envelope.
///
/// The walk itself is the framework's (`constraints::validate`), so this cannot find a
/// different set from the one the operation's own sequence finds. Written here rather
/// than taken from `service_common::validation` for the same reason `networkmonitor`
/// and `uxc` write it in their own `service.rs`: these are a handful of framework
/// calls, and the refusal this words quotes no value, so none of that module's
/// value-echo restoration would ever be read.
///
/// Empty when the request satisfies the model.
fn violations_in<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // `member_name` is already a violation path in the spelling the constraint
        // walk uses, so it places identically to a value violation.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

/// The account-link API's answer to a request that breaks a modelled constraint:
/// HTTP 400 `ValidationException` carrying that class's one fixed sentence,
/// rendered in THIS service's protocol envelope.
///
/// The envelope and the status stay with the framework's refusal channel
/// (`refusal_response`, which the generated entry renders from the service's
/// model-derived protocol); only the code and the message are chosen here, and they
/// are chosen together — a client branches on the code, so one without the other is
/// worse than neither.
fn account_link_refusal(svc: &dyn Service, req: &RtRequest, refused: Vec<Violation>) -> RtResponse {
    svc.definition().refusal_response(
        ProtocolError::Invalid(refused),
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: crate::state::ACCOUNT_LINK_INVALID_REQUEST_MESSAGE.to_string(),
        }),
        req,
    )
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut account_link = OperationRoutingTable::new();
        // The account-link family, which is exactly the set of operations whose
        // model declares `ValidationException` — the unit test below reads that
        // set off the model, so an operation added to the family and not to this
        // list fails there rather than answering a constraint refusal in the
        // framework's wording.
        wrap_operations!(
            account_link,
            engine,
            accept_account_link_invitation::AcceptAccountLinkInvitation,
            create_account_link_invitation::CreateAccountLinkInvitation,
            delete_account_link_invitation::DeleteAccountLinkInvitation,
            get_account_link::GetAccountLink,
            list_account_links::ListAccountLinks,
            reject_account_link_invitation::RejectAccountLinkInvitation,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            account_link,
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `AccountLinkRefusal` for the six account-link operations, so a request
        // of theirs that breaks a modelled constraint is refused in that API's
        // own words before the operation runs. Every other key, and a key the
        // model does not name, resolves to `None` here, so those operations are
        // served by the generated table exactly as before.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.account_link.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording — 400 `ValidationException` with the `N validation error
        // detected: Value '<v>' at '<member>' failed to satisfy constraint: …`
        // envelope — which is what this service's older surface answers with, and
        // which this change deliberately leaves alone: no recording shows those
        // operations answering anything else. The refusals worded differently are
        // the account-link family's, and `AccountLinkRefusal` answers those before
        // this walk is reached, because the operation cannot be told from the
        // violations this seam is handed.
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
    //! The one thing about the wiring above a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is EXACTLY the family the rule is
    //! about, read off the model rather than off a second hand-written list.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test
    /// time rather than embedded: it is JSON no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-workspaces-workspacesservice.json"
    );

    /// The error shape that marks the account-link family: it is the client-input
    /// error those operations model, and no other operation of this service
    /// declares it.
    const VALIDATION_EXCEPTION_SHAPE: &str = "com.amazonaws.workspaces#ValidationException";

    /// Every operation whose model declares `ValidationException`, by shape name.
    fn operations_modelling_validation_exception() -> Vec<String> {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes")
            .clone();
        let mut names: Vec<String> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter(|(_, shape)| {
                shape["errors"]
                    .as_array()
                    .is_some_and(|errors| errors.iter().any(|e| e["target"] == VALIDATION_EXCEPTION_SHAPE))
            })
            .filter_map(|(id, _)| id.rsplit('#').next().map(str::to_string))
            .collect();
        names.sort();
        names
    }

    /// RULE, over the whole modelled surface: an operation is served behind
    /// [`AccountLinkRefusal`] exactly when its model declares
    /// `ValidationException` — the account-link family the fixed refusal wording
    /// belongs to. An operation that joins the family and is left out of
    /// `wrap_operations!` fails here instead of silently answering a constraint
    /// refusal in the framework's `N validation error detected: …` wording, and one
    /// wrapped that does not model the error fails here instead of silently losing
    /// that wording.
    #[test]
    fn exactly_the_account_link_family_is_served_behind_the_fixed_refusal() {
        let family = operations_modelling_validation_exception();
        assert_eq!(
            family.len(),
            6,
            "case the model's surface: expected the six account-link operations to \
             model ValidationException, got {family:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in &family {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.account_link.get(operation).is_some(),
                "case {operation}: an operation modelling ValidationException is served \
                 behind AccountLinkRefusal; this one is missing from wrap_operations!"
            );
        }

        // And nothing outside the family is wrapped: the other operations keep the
        // framework's own constraint wording.
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let outside: Vec<String> = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes")
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next().map(str::to_string))
            .filter(|name| !family.contains(name))
            .collect();
        assert!(
            !outside.is_empty(),
            "case the model's surface: this service models operations outside the \
             account-link family, got {outside:?}"
        );
        for operation in outside {
            assert!(
                service.account_link.get(&operation).is_none(),
                "case {operation}: an operation that does not model ValidationException \
                 must keep the framework's own constraint wording, but it is wrapped"
            );
        }
    }
}
