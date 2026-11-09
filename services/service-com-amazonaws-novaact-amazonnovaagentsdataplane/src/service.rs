// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-novaact-amazonnovaagentsdataplane` service: how it claims requests and how it words the
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
//! # The authorization layer every handler runs under
//!
//! AWS decides authorization at the IAM layer, in front of the service: a caller whose
//! identity-based policies do not grant the action — or that an explicit `Deny` covers — is
//! refused `AccessDeniedException` (HTTP 403, a SERVICE-COMMON error of this model, so every
//! one of its operations declares it) and the call performs nothing, so a refused create
//! leaves no resource behind. That decision is a property of the request's identity, the
//! action it names and the resource it addresses rather than of what a handler goes on to do,
//! so it is taken ONCE here in [`Gated`], for every operation of this service
//! ([`authorization`] holds the lookups and the sentence), and nowhere else. The resource it
//! is taken against is [`authorization::judged_resource`]: the resource this model's chain
//! names in the request's URI, and for the create of a definition the definition it would
//! create.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// Whether IAM lets a request to this service perform the action it asks for: the decision
/// [`Gated`] takes in front of every handler.
pub mod authorization;

/// The ONE choice this service makes about how a modelled input-constraint refusal is
/// worded — the `@pattern` class, stated over the expression alone — delegating every
/// other part of the sentence to the shared home for it. Private: it is reached through
/// `override_refusal` below, not by a caller.
mod validation;

/// The `com-amazonaws-novaact-amazonnovaagentsdataplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`Gated`] per modelled operation, keyed by the routing key the model names that
    /// operation by ([`Operation::NAME`], the same key the generated routing table uses).
    /// Built once here, never per request; each entry carries the operation's TYPE and a
    /// handle on the one shared engine, so this is a table of names rather than a second set
    /// of operations.
    gated: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gated: gated_table::<Eng>(&engine),
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
        // Serve every modelled operation through its [`Gated`] wrapper, so the decision AWS
        // takes in front of a handler is taken there: the caller's authorization for the
        // action is decided before the operation reads or writes anything. The wrapper
        // delegates every request it does not refuse to the generated operation in the
        // routing table, which stays the only thing that serves it.
        //
        // Keyed on the routing key the model names each operation by, so a request that names
        // no modelled operation resolves exactly as it did before.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gated.get(key).map(|wrapper| &**wrapper)
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
        //
        // This service states a `@pattern` requirement over the expression alone —
        // see [`validation`], which replaces code and message together.
        validation::refused(refusal)
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

/// One modelled operation, served behind the decision AWS takes in front of a handler: IAM's
/// authorization of the action.
///
/// It carries the operation TYPE — which is what names the action the request asks for
/// ([`Operation::NAME`]) — and a handle on the ONE shared engine, which is what the decision
/// reads the caller's identity and its policies through ([`authorization`]). Every request it
/// does not refuse is delegated to the one operation instance the generated routing table
/// built from that same engine, so there is no second instance of any operation and no second
/// store.
struct Gated<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> Gated<Op, Eng> {
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for Gated<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // IAM's decision, taken before the operation reads or writes anything: the action
            // is this operation's own name under this service's prefix, and the resource is
            // the one the REQUEST addresses. A refused request never reaches the delegate, so
            // it mints nothing and reports nothing.
            let resource = authorization::judged_resource(ctx, Op::NAME, req);
            let denial = authorization::authorize(&*self.engine, ctx, Op::NAME, &resource).err();
            if let Some(denial) = denial {
                return authorization::refusal_response(svc.definition().protocol(), &denial);
            }
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key the
                // generated table registered. Answer the framework's own no-such-operation
                // response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed (or not) as the
        // MODEL says for the operation it names, so authentication is decided by the modelled
        // operation this wrapper stands in front of.
        None
    }
}

/// One [`Gated`] per modelled operation of this service, keyed by the routing key the model
/// names that operation by.
///
/// Every operation is listed, because the authorization decision is taken for every one of
/// them: `AccessDeniedException` is a service-common error of this model, so no operation is
/// left unjudged.
fn gated_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut wrappers: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($($operation:ty),* $(,)?) => {
            $(
                wrappers.insert(
                    <$operation as Operation>::NAME,
                    Box::new(Gated::<$operation, Eng>::new(Arc::clone(engine)))
                        as Box<dyn ErasedOperation>,
                );
            )*
        };
    }
    gate!(
        crate::ops::create_act::CreateAct<Eng>,
        crate::ops::create_session::CreateSession<Eng>,
        crate::ops::create_workflow_definition::CreateWorkflowDefinition<Eng>,
        crate::ops::create_workflow_run::CreateWorkflowRun<Eng>,
        crate::ops::delete_workflow_definition::DeleteWorkflowDefinition<Eng>,
        crate::ops::delete_workflow_run::DeleteWorkflowRun<Eng>,
        crate::ops::get_workflow_definition::GetWorkflowDefinition<Eng>,
        crate::ops::get_workflow_run::GetWorkflowRun<Eng>,
        crate::ops::invoke_act_step::InvokeActStep<Eng>,
        crate::ops::list_acts::ListActs<Eng>,
        crate::ops::list_models::ListModels<Eng>,
        crate::ops::list_sessions::ListSessions<Eng>,
        crate::ops::list_workflow_definitions::ListWorkflowDefinitions<Eng>,
        crate::ops::list_workflow_runs::ListWorkflowRuns<Eng>,
        crate::ops::update_act::UpdateAct<Eng>,
        crate::ops::update_workflow_run::UpdateWorkflowRun<Eng>,
    );
    wrappers
}
