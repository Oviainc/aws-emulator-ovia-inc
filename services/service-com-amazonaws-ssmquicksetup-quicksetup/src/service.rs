// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ssmquicksetup-quicksetup` service: how it claims requests and how it words the
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
//!   correctly. EDITED here: the operations whose route labels a resource they do not
//!   themselves address are served behind [`TargetAuthorization`], the
//!   request-authorization stage described below.
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
//! ## The request-authorization stage in front of the cross-resource routes
//!
//! Two of this service's route families carry, as a path label, the ARN of a resource
//! that is NOT the resource the route addresses:
//!
//! * the tag plane, `/tags/{ResourceArn}`, whose label is any taggable resource's ARN —
//!   the model bounds it with no `@pattern` and no `@length` at all, so the route does not
//!   even say which of this service's resource kinds it names;
//! * `/configurationDefinition/{ManagerArn}/{Id}`, which addresses one configuration
//!   definition and labels it with the ARN of the configuration manager that OWNS it.
//!
//! Those routes are authorized against that ARN before the request reaches the service:
//! an ARN that names no resource the endpoint serves is not an ARN the caller may act on,
//! so the request is refused there with HTTP 401, the wire error code
//! [`UNAUTHORIZED_CODE`] and the one fixed sentence [`UNAUTHORIZED_MESSAGE`] — carried by
//! a shape this service's model does NOT declare, which is how a client can tell the two
//! layers apart: `ResourceNotFoundException` and `ValidationException` are what the
//! service itself answers, and neither is what a request stopped ahead of it gets.
//!
//! The stage sits in front of EVERYTHING those requests say, which is what makes it
//! observable: it precedes the modelled constraint walk (so a path label outside its
//! `@pattern` never reaches validation) and the handler's own lookup (so the modelled 404
//! is not what an unauthorized target earns).
//!
//! It fronts NOTHING else, and the two exclusions are the rule's edges rather than
//! omissions:
//!
//! * `/configurationManager/{ManagerArn}` labels the very resource it addresses, and
//!   `GetConfigurationManager`, `UpdateConfigurationManager` and
//!   `DeleteConfigurationManager` resolve it themselves and answer the modelled
//!   `ResourceNotFoundException` for an ARN naming no manager — which is what a caller
//!   polling a manager it created, through a deletion, is answered;
//! * an ARN a request carries in its BODY is no route label at all, so no route is
//!   authorized against it and the handler resolves it like any other input — which is why
//!   `ListConfigurations` answers its modelled `ResourceNotFoundException`, with this
//!   service's own not-found sentence, for a `ManagerArn` naming no manager.
//!
//! Only the ARN is judged here. Everything a request says about the resource it named —
//! the definition id under a manager that does exist, the tags, the parameters, the
//! teardown already in flight — belongs to the handler and is answered in this service's
//! own modelled shapes.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

/// The wire error code (`__type` and `x-amzn-errortype`) of the authorization refusal
/// that fronts this service's cross-resource routes.
///
/// Not a shape this service's model declares: the refusal is stated ahead of the code
/// that owns the modelled errors, so it travels with its own code and status rather than
/// through an operation's error enum.
pub const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The HTTP status that refusal is served on.
pub const UNAUTHORIZED_STATUS: u16 = 401;

/// The one sentence that refusal states, for every request it refuses.
///
/// Wholly fixed text: an authorization refusal reports no member, no value and no
/// resource, so there is nothing in it to derive from the request.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The `message` member of the synthetic shape the authorization refusal's body is
/// rendered from.
///
/// Lowercase `message`, which is what the layer in front of the service names it —
/// unlike this service's own modelled `ResourceNotFoundException` / `ValidationException`,
/// whose single member is `Message`. `with_xml_name` keeps the member correct on the XML
/// envelopes too, as the synthetic shapes in [`crate::scaffolded`] do.
static UNAUTHORIZED_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#TargetAuthorizationRefusal$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Schema for that synthetic shape.
///
/// In the `smithy.framework` namespace for the same reason as the shapes in
/// [`crate::scaffolded`]: it is in no service's model, so it must not collide with a
/// modelled one. The schema shapes only the BODY; the wire error code and the status
/// travel beside it.
static UNAUTHORIZED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#TargetAuthorizationRefusal",
        "smithy.framework",
        "TargetAuthorizationRefusal",
    ),
    ShapeType::Structure,
    &[&UNAUTHORIZED_MESSAGE_MEMBER],
);

/// The body of the authorization refusal: the fixed sentence and nothing else.
#[derive(Debug, Clone, Copy)]
struct Unauthorized;

impl SerializableStruct for Unauthorized {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&UNAUTHORIZED_MESSAGE_MEMBER, UNAUTHORIZED_MESSAGE)?;
        Ok(())
    }
}

/// The modelled input member that carries the ARN of the configuration manager an
/// operation addresses — `GetConfigurationManager`, `UpdateConfigurationManager`,
/// `DeleteConfigurationManager` and `UpdateConfigurationDefinition` all bind it as an
/// `@httpLabel` of their route.
/// The modelled input member that carries the ARN of the configuration manager OWNING
/// the configuration definition `UpdateConfigurationDefinition` addresses: the parent's
/// ARN, labelling the child route `/configurationDefinition/{ManagerArn}/{Id}`.
const MANAGER_ARN_MEMBER: &str = "ManagerArn";

/// The modelled input member that carries the ARN of the resource a tag operation
/// addresses — `ListTagsForResource`, `TagResource` and `UntagResource` all bind it as
/// the `@httpLabel` of `/tags/{ResourceArn}`. The configuration manager is this
/// service's only ARN-addressable resource, so this label names one too.
const RESOURCE_ARN_MEMBER: &str = "ResourceArn";

/// One modelled operation, served behind the request-authorization stage described in
/// the module docs.
///
/// Holds no operation of its own: it carries the operation TYPE, which is all the stage
/// needs (the input schema the target ARN is read through, and the operation's own name),
/// and delegates a request whose target it authorizes to the one operation instance the
/// generated routing table built from the shared engine — so there is no second instance
/// of any operation and no second store. The `engine` field is the SAME `Arc` the
/// generated entry holds, and is what lets the stage ask this service's state whether the
/// ARN names a resource at all.
struct TargetAuthorization<Op, Eng> {
    engine: Arc<Eng>,
    /// The input member this operation's route carries the target ARN in, as the model
    /// names it.
    target: &'static str,
    operation: PhantomData<fn() -> Op>,
}

impl<Op, Eng> TargetAuthorization<Op, Eng> {
    const fn new(engine: Arc<Eng>, target: &'static str) -> Self {
        Self {
            engine,
            target,
            operation: PhantomData,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for TargetAuthorization<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The stage's own answer is computed in one call that reads, uses and drops
            // the protocol: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it
            // across the await below would make this future non-`Send`.
            if let Some(refused) = self.refusal(svc, req, ctx) {
                return refused;
            }
            // A request whose target is authorized is served by the generated operation,
            // which runs the whole framework sequence (deserialize, validate, handle,
            // serialize) over the same bytes.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // Wrapping an operation must not change which schemes sign it: authentication is
        // decided by the modelled operation this stage stands in front of.
        None
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> TargetAuthorization<Op, Eng> {
    /// This stage's answer to `req`, or `None` when the target it names is one the
    /// endpoint serves — in which case the operation behind it is served.
    fn refusal(
        &self,
        svc: &dyn Service,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Option<RtResponse> {
        let protocol = svc.definition().protocol();
        // A request carrying no value for the label names nothing to authorize (no route
        // of this service binds one optionally, so this is the unreadable request rather
        // than a real case): it is left to the layers behind the stage.
        let arn = target_arn(protocol, req, Op::INPUT_SCHEMA, self.target)?;
        if self.serves(&arn, ctx) {
            return None;
        }
        Some(unauthorized(svc, req))
    }

    /// Whether the endpoint this request reached serves a resource with this ARN.
    ///
    /// The lookup is the store's own ([`crate::state::ComAmazonawsSsmquicksetupQuicksetupState::manager_in`]):
    /// the configuration manager is this service's only ARN-addressable resource, and a
    /// manager belongs to the one Region its ARN was minted in, so an ARN of another
    /// Region's endpoint names no resource HERE exactly as one that was never minted
    /// does.
    ///
    /// A store the engine does not hold is not a decision this stage can make, so such a
    /// request is passed through to the handler, which reports the unavailable store in
    /// this service's own modelled server error.
    fn serves(&self, arn: &str, ctx: &OperationContext<'_>) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return true;
        };
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();
        // A manager whose teardown has come due is gone before this request is judged,
        // rather than being authorized on its way out of the store: the same materialising
        // of what the clock already owes that every handler behind this stage does.
        store.settle(SystemTime::now());
        store.manager_in(&region, arn).is_some()
    }
}

/// The value one named member of an operation's input carries in this request, read
/// through the PROTOCOL's own binding — `None` when the request carries no such value,
/// when the shape declares no such member, or when the value is not a string.
///
/// Reads that ONE member and nothing else, which is what lets it answer for a request
/// the operation's own input reader cannot complete: a body member the shape requires and
/// the request omits yields no bound input to read a field off, while the label the
/// request DID carry is bound exactly as dispatch would bind it, with the same
/// percent-decoding. Deserializing does not enforce constraints, which is what lets the
/// stage judge a target the modelled `@pattern` would have refused.
fn target_arn(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    schema: &'static Schema,
    member: &str,
) -> Option<String> {
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    let mut found = None;
    // The result is deliberately dropped: a walk that reads the wanted member and then
    // stops short of the end of the request still read it, and a walk that cannot read
    // the request at all answers `None` like one that found no such member.
    let _ = deserializer.read_struct(schema, &mut |candidate, d| {
        if found.is_some() {
            return Ok(());
        }
        if candidate.member_name() == Some(member) {
            found = Some(d.read_string(candidate)?);
            return Ok(());
        }
        // A member the walk passes over still has its value in the body's document, and
        // only the codec that reads values can step over one, so a body member's value is
        // read and discarded to keep that codec in step with its own input. A member bound
        // to the URI carries its value outside the body, where there is nothing to step
        // over.
        if !carried_in_uri(candidate) {
            d.read_document(candidate)?;
        }
        Ok(())
    });
    found
}

/// Whether this member schema binds its value somewhere in the request URI rather than in
/// the body: the five HTTP locations a member can occupy outside the payload.
fn carried_in_uri(member: &Schema) -> bool {
    member.http_label().is_some()
        || member.http_query().is_some()
        || member.http_query_params().is_some()
        || member.http_header().is_some()
        || member.http_prefix_headers().is_some()
}

/// The authorization refusal, rendered through this service's own protocol so it carries
/// the same envelope (and the same `x-amzn-errortype` header) as every other error this
/// service answers. The status and the error code come from the refusal itself: the shape
/// is not in the model, so there is no modelled error to read them off.
fn unauthorized(svc: &dyn Service, req: &RtRequest) -> RtResponse {
    svc.definition()
        .protocol()
        .serialize_error(
            &Unauthorized,
            &UNAUTHORIZED_SCHEMA,
            UNAUTHORIZED_CODE,
            UNAUTHORIZED_STATUS,
        )
        .unwrap_or_else(|error| svc.definition().framework_error_response(error, req))
}

/// Every operation whose route carries, as a path label, the ARN of a resource the route
/// does not itself address, each served behind the request-authorization stage, keyed by
/// the routing key the protocol resolves.
///
/// The list is the model's (`smithy_query(query="digest")`): the three operations bound to
/// `/tags/{ResourceArn}`, whose label is any taggable resource's ARN, and the one bound to
/// `/configurationDefinition/{ManagerArn}/{Id}`, whose label is the addressed definition's
/// PARENT. The three operations bound to `/configurationManager/{ManagerArn}` are
/// deliberately absent — that label IS the resource they address, and they answer the
/// modelled `ResourceNotFoundException` for an ARN naming no manager (see the module
/// docs) — and so is `GetConfiguration`, whose label is a configuration id rather than an
/// ARN.
fn target_authorization_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> OperationRoutingTable {
    let mut table = OperationRoutingTable::new();
    macro_rules! authorize {
        ($($module:ident :: $operation:ident => $member:expr),+ $(,)?) => {
            $({
                table.add_operation(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(TargetAuthorization::<crate::ops::$module::$operation<Eng>, Eng>::new(
                        Arc::clone(engine),
                        $member,
                    )) as Box<dyn ErasedOperation>,
                );
            })+
        };
    }
    authorize!(
        update_configuration_definition::UpdateConfigurationDefinition => MANAGER_ARN_MEMBER,
        list_tags_for_resource::ListTagsForResource => RESOURCE_ARN_MEMBER,
        tag_resource::TagResource => RESOURCE_ARN_MEMBER,
        untag_resource::UntagResource => RESOURCE_ARN_MEMBER,
    );
    table
}

/// The `com-amazonaws-ssmquicksetup-quicksetup` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The cross-resource routes, each behind the request-authorization stage
    /// ([`target_authorization_table`]).
    authorized: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            authorized: target_authorization_table(&engine),
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

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // Serve every operation whose route labels a resource it does not itself address
        // behind the request-authorization stage (see the module docs), so the ARN is
        // authorized where those routes share one rule rather than in four handlers — and
        // ahead of the modelled constraint walk, which is where the stage has to be:
        // behind it, a path label outside its `@pattern` would be refused as malformed
        // input before the target was ever judged. The stage delegates a request whose
        // target it authorizes to the generated operation in the routing table, which
        // stays the only thing that serves it.
        //
        // A request that names no modelled operation has no route and so no target: the
        // routing failure is the answer it gets.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.authorized.get(key)
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
        let _ = refusal;
        None
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
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
    ) -> Option<RtResponse> {
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
