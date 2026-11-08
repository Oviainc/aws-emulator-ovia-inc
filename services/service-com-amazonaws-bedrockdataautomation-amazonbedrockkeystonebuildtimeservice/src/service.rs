// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice` service: how it claims requests and how it words the
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
//! A sixth seam, `override_protocol`, is the one this service DOES take: it serves the
//! model's own `restJson1` codec with every timestamp in the ISO-8601 `date-time` form
//! this model's single timestamp shape declares (see [`date_time_timestamps`]).
//!
//! # The authorization layer every handler runs under
//!
//! AWS decides authorization at the IAM layer, in front of the service: a caller whose
//! identity-based policies do not grant the action is refused `AccessDeniedException`
//! (HTTP 403 — a SERVICE-COMMON error of this model, so every one of its operations
//! declares it) and the call performs nothing. That decision is a property of the
//! request's identity, the action it names and the resource it addresses rather than of
//! what a handler goes on to do, so it is taken ONCE here in [`Gated`], for every
//! operation of this service ([`authorization`] holds the lookups and the sentence), and
//! nowhere else. The resource it is taken against is [`authorization::judged_resource`]:
//! the ARN the request names of the family its URI space addresses, else that family's
//! collection in this account and Region — and for the three tagging operations, which bind
//! no URI label, the ARN their body names ([`tagging_target`]).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

/// Whether IAM lets a request to this service perform the action it asks for: the gate
/// [`Gated`] takes in front of every handler.
pub mod authorization;

/// The resource a tagging call addresses, read off the request's body — the one member
/// those three operations name their target in, read by the gate below and by the Bedrock
/// control plane's front door for the paths both models bind.
pub mod tagging_target;

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock};

/// This service's Smithy model: the one source for which of its members are
/// `@sensitive`, and so for which values a boundary refusal states none.
const MODEL_JSON: &str = include_str!(
    "../smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json"
);

/// The members of this model whose value a refusal WITHHOLDS, read off [`MODEL_JSON`]
/// rather than listed here, so the decision cannot drift from the model as the model
/// grows a `@sensitive` member.
///
/// This model marks the two resource NAMES (`BlueprintName`,
/// `DataAutomationProjectName`) and the library name beside them, so a request refused
/// for the shape of a name it submitted is told which member and which requirement, and
/// never handed the name back.
static WITHHELD_MEMBERS: LazyLock<service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON));

pub mod date_time_timestamps;

/// The `com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: date_time_timestamps::DateTimeRestJson,
    /// One [`Gated`] per modelled operation, keyed by the routing key the model names that
    /// operation by ([`Operation::NAME`], the same key the generated routing table uses).
    /// Built once here, never per request; each entry carries the operation's TYPE and a
    /// handle on the one shared engine, so this is a table of names rather than a second
    /// set of operations.
    gated: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gated: gated_table::<Eng>(&engine),
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            protocol: date_time_timestamps::DateTimeRestJson::new(),
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
        // This service signs as `bedrock`, the signing name every Bedrock-family service
        // shares while serving its own endpoint (`bedrock-data-automation` here).
        // Claiming on that name alone makes all six claimants for every Bedrock request,
        // which the router answers `500 AmbiguousService` — a code no AWS model carries —
        // rather than serving one of them.
        //
        // So the claim is this model's own path space (`blueprints`,
        // `data-automation-projects`, `data-automation-libraries`,
        // `invokeBlueprintOptimizationAsync`, `getBlueprintOptimizationStatus`), through
        // the one rule `service_common::bedrock` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split). Its
        // three tagging paths (`/tagResource`, `/untagResource`, `/listTagsForResource`)
        // are bound IDENTICALLY by the Bedrock control plane, so nothing in such a request
        // tells the two apart — AWS tells them apart by endpoint host, which is not on
        // the wire here — and the family rule gives that segment to the control plane.
        // `None` for the RPC argument: every operation here is bound to a URI.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve every modelled operation through its [`Gated`] wrapper, so the decision
        // AWS takes in front of a handler is taken there: the caller's authorization for
        // the action is decided before the operation reads or writes anything. The wrapper
        // delegates every request it does not refuse to the generated operation in the
        // routing table, which stays the only thing that serves it.
        //
        // Keyed on the routing key the model names each operation by, so a request that
        // names no modelled operation resolves exactly as it did before.
        // The routing key is read through the GENERATED definition's protocol, whose route
        // table is the model's; this service's `override_protocol` codec carries an empty
        // one by design (it drives only the modelled sequence), so it resolves no key.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gated.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) and leaves the words
        // to the service. This service states them the shared AWS way: the framework's
        // envelope, clauses, member paths and requirements, with two renderings the
        // framework has no way to apply itself —
        //
        // * a `@pattern` requirement is quoted WITHOUT the `^`/`$` a whole-value
        //   pattern is written with, so a caller that lifts the expression out of the
        //   message compiles the expression this service advertises;
        // * a clause about a member the model marks `@sensitive` ([`WITHHELD_MEMBERS`])
        //   states NO value: a refusal does not hand back a value the model says not to
        //   disclose.
        //
        // The code stays `ValidationException`, which is both the modelled client-input
        // error of this service and what the framework already answered, so nothing a
        // client branches on changes — only the words.
        service_common::validation_message::aws_validation_refusal_withholding(refusal, |path| {
            WITHHELD_MEMBERS.withholds_value(path)
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restJson1 codec, with the ONE difference the generic
        // renderer cannot express: a timestamp is written in the ISO-8601 date-time
        // form this model's only timestamp shape declares, rather than in the
        // protocol's default epoch-seconds form (see `date_time_timestamps`).
        // Borrowed from the field built in `new`, never constructed per request.
        Some(&self.protocol)
    }
}

/// One modelled operation, served behind the decision AWS takes in front of a handler:
/// IAM's authorization of the action.
///
/// It carries the operation TYPE — the input shape to read the request into and the input
/// `Schema` to read each member's declared form off — and a handle on the ONE shared
/// engine, which is what the authorization decision reads the caller's identity and its
/// policies through ([`authorization`]). Every request it does not refuse is delegated to
/// the one operation instance the generated routing table built from that same engine, so
/// there is no second instance of any operation and no second store.
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
            // IAM's decision, taken before the operation reads or writes anything: the
            // action is this operation's own name under this service's prefix, and the
            // resource is the one the REQUEST addresses. A refused request never reaches
            // the delegate, so it mints nothing, writes no tag and reports nothing.
            //
            // The protocol is read, used and dropped inside this one statement, and the
            // operation to delegate to is resolved into a value further down:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across the await
            // below would make this future non-`Send`.
            // The ARN the request NAMES: the leaf walk over the operation's own input,
            // falling back to the `resourceARN` the body of a tagging call carries
            // ([`tagging_target`]). The fallback is what keeps the decision exact for those
            // three operations, whose target is only ever in the body: the leaf walk stops
            // at the budget its module states, so a request whose `tags` list precedes
            // `resourceARN` could otherwise leave the target unread and be judged against
            // IAM's any-resource wildcard, which a policy scoped to one ARN does not match.
            let named = named_resource::<Op>(svc.definition().protocol(), req)
                .or_else(|| tagging_target::resource_arn_in_body(req));
            let resource = authorization::judged_resource(ctx, named, req.uri());
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
        // MODEL says for the operation it names, so authentication is decided by the
        // modelled operation this wrapper stands in front of.
        None
    }
}

/// The ARN of one of this service's own resources the request CARRIES, when it carries
/// one — the resource it addresses, which IAM evaluates the action against.
///
/// Read from the operation's own input: the request is deserialized against `Op`'s input
/// schema (so a member is placed by the model, not by where it happens to sit in the wire
/// form) and every string the built input carries is collected by
/// `service_common::validation::submitted_leaves`, the one walk in the tree that spells a
/// member path the way the framework's constraint walk does. Nothing is read out of the
/// STORE, which is what keeps the decision independent of whether that resource exists.
///
/// `None` for a request that carries no such ARN, and for a body no codec can read — a
/// failure the framework's own walk words. Whether the ARN it carries IS the resource the
/// action is judged against is [`authorization::judged_resource`]'s question: an operation
/// may carry an ARN of another family beside its target.
///
/// KNOWN BOUND: the leaf walk stops at the budget its module states, so a request whose
/// earlier strings exhaust it can leave a later ARN uninspected. The miss is FAIL-SAFE: such
/// a request is authorized against the collection of the family it addressed rather than
/// against an ARN this layer did not see — never against a resource the caller did not name.
fn named_resource<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<String> {
    let mut deserializer = protocol.deserialize_input(req, Op::INPUT_SCHEMA).ok()?;
    let read = <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    let input = read.ok()?;
    service_common::validation::submitted_leaves(&input, Op::INPUT_SCHEMA)
        .into_iter()
        .find(|(_path, value)| authorization::names_a_resource_of_this_service(value))
        .map(|(_path, value)| value)
}

/// One [`Gated`] per modelled operation of this service, keyed by the routing key the model
/// names that operation by.
///
/// Every operation is listed, because the authorization decision is taken for every one of
/// them: `AccessDeniedException` is a service-common error of this model, so no operation
/// is left unjudged. Each entry names the operation TYPE, which is what carries the input
/// shape and the input schema the ARN above is read through.
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
        crate::ops::copy_blueprint_stage::CopyBlueprintStage<Eng>,
        crate::ops::create_blueprint::CreateBlueprint<Eng>,
        crate::ops::create_blueprint_version::CreateBlueprintVersion<Eng>,
        crate::ops::create_data_automation_library::CreateDataAutomationLibrary<Eng>,
        crate::ops::create_data_automation_project::CreateDataAutomationProject<Eng>,
        crate::ops::delete_blueprint::DeleteBlueprint<Eng>,
        crate::ops::delete_data_automation_library::DeleteDataAutomationLibrary<Eng>,
        crate::ops::delete_data_automation_project::DeleteDataAutomationProject<Eng>,
        crate::ops::get_blueprint::GetBlueprint<Eng>,
        crate::ops::get_blueprint_optimization_status::GetBlueprintOptimizationStatus<Eng>,
        crate::ops::get_data_automation_library::GetDataAutomationLibrary<Eng>,
        crate::ops::get_data_automation_library_entity::GetDataAutomationLibraryEntity<Eng>,
        crate::ops::get_data_automation_library_ingestion_job::GetDataAutomationLibraryIngestionJob<Eng>,
        crate::ops::get_data_automation_project::GetDataAutomationProject<Eng>,
        crate::ops::invoke_blueprint_optimization_async::InvokeBlueprintOptimizationAsync<Eng>,
        crate::ops::invoke_data_automation_library_ingestion_job::InvokeDataAutomationLibraryIngestionJob<Eng>,
        crate::ops::list_blueprints::ListBlueprints<Eng>,
        crate::ops::list_data_automation_libraries::ListDataAutomationLibraries<Eng>,
        crate::ops::list_data_automation_library_entities::ListDataAutomationLibraryEntities<Eng>,
        crate::ops::list_data_automation_library_ingestion_jobs::ListDataAutomationLibraryIngestionJobs<Eng>,
        crate::ops::list_data_automation_projects::ListDataAutomationProjects<Eng>,
        crate::ops::list_tags_for_resource::ListTagsForResource<Eng>,
        crate::ops::tag_resource::TagResource<Eng>,
        crate::ops::untag_resource::UntagResource<Eng>,
        crate::ops::update_blueprint::UpdateBlueprint<Eng>,
        crate::ops::update_data_automation_library::UpdateDataAutomationLibrary<Eng>,
        crate::ops::update_data_automation_project::UpdateDataAutomationProject<Eng>,
    );
    wrappers
}
