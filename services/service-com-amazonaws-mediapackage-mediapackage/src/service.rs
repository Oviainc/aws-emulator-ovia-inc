// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediapackage-mediapackage` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it: it puts the input-validation gate described
//!   below in front of every modelled operation and lets the generated routing table
//!   serve the request from there.
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
//! ## Why this service refuses a bad request itself
//!
//! MediaPackage models no `ValidationException` at all: its client-input error is
//! `UnprocessableEntityException`, HTTP **422**, "The parameters sent in the request are
//! not valid" (`smithy_query(query="operation",operation="ListChannels",…)` lists
//! `ForbiddenException` 403, `NotFoundException` 404, `TooManyRequestsException` 429 and
//! `UnprocessableEntityException` 422 and nothing else). The framework's own request
//! boundary refuses a value outside a declared constraint BEFORE any handler runs, and it
//! answers that refusal as a generic **400 `ValidationException`** — a status and a code
//! this service never produces, so an SDK cannot even deserialize it into a modelled
//! type. The seam that rewords such a refusal (`Service::override_refusal` ->
//! `Refused`) carries only a code and a message, never a status or a body member, so it
//! cannot state this one.
//!
//! So this service refuses a bad request itself, through [`ModeledInputValidation`]: a
//! gate [`Service::override_operation`] puts in front of every modelled operation. For
//! each request it asks, in order:
//!
//! 1. the bound this SERVICE enforces on the page size, which is narrower than the
//!    modelled `MaxResults` range (see [`crate::state::page_size_violation`]) and applies
//!    to exactly the operations whose input SCHEMA binds that query parameter;
//! 2. the framework's OWN constraint walk
//!    (`aws_smithy_schema_server::constraints::validate`, the same call the centralized
//!    sequence makes), so the declared rules enforced are exactly the model's and nothing
//!    about a constraint is restated here.
//!
//! Either way the answer is this service's own envelope — 422
//! `UnprocessableEntityException` with `message` and the per-member `errors` map, built by
//! [`crate::state`] — and every request the service accepts is handed straight to the
//! generated operation, which the routing table still owns.
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing key
//! names (so a gate cannot weaken an operation's auth schemes), and the per-operation
//! telemetry the centralized sequence records — a request the gate refuses never enters
//! that sequence, so it is counted nowhere, exactly as a request refused before dispatch
//! is.
//!
//! ## The two data planes
//!
//! `CreateChannel` hands out ingest URLs and `CreateOriginEndpoint` a playback URL, and a
//! URL is a promise that it can be FOLLOWED. Neither wire protocol is modelled, so both
//! paths reach this file twice:
//!
//! - [`Service::can_handle`] also claims a request whose PATH is one of those URLs' and
//!   names a channel or an endpoint THIS store holds, so following the URL routes back
//!   here even when nothing else in the request names a service (an encoder presents its
//!   ingest credentials rather than a signature, a viewer presents nothing, and the
//!   published host names no service the front door resolves).
//! - [`Service::override_routing_error`] serves it: that seam is reached BEFORE
//!   authentication precisely for requests that name no modelled operation, which is what
//!   makes an unsigned push and an unsigned playback request work.
//!
//! What each plane ANSWERS — which credentials authenticate an ingest push, what a channel
//! retains of what was pushed, and what an origin endpoint's `Origination` and `Whitelist`
//! let a viewer see — is [`crate::state::data_plane`]'s, decided under one hold of the
//! store's guard. This file only turns the outcome it reports into an HTTP response.
//!
//! ## How a response with no body is framed
//!
//! Two operations here answer with no body at all: `TagResource` (POST
//! `/tags/{ResourceArn}`) and `UntagResource` (DELETE `/tags/{ResourceArn}`) are both
//! modelled with output `Unit` at HTTP **204**
//! (`smithy_query(query="operation",operation="TagResource,UntagResource",…)`). A response
//! with no body still has a LENGTH, and AWS states it: these answers carry
//! `content-length: 0`. The protocol's serializer leaves the header off, which leaves the
//! length to the transport to frame — so a client, proxy or SDK transport that keys on a
//! framing header being present on a bodyless response reads the emulator's reply
//! differently from the service's.
//!
//! [`Service::override_protocol`] therefore serves a protocol that is the model's own in
//! every respect except one: a response this service serializes with an EMPTY body says so
//! with `content-length: 0`. A response that carries a body is left exactly as the model's
//! protocol renders it, framed from the body it carries.
//!
//! That framing is what this service answers, and it is what the in-process front door
//! delivers (the inter-service callback path and the SDK connector both serve the router's
//! response unchanged). An answer served over HTTP/1.1 reaches the client without it: the
//! HTTP writer the vendored transport uses removes `content-length` from a `204`, which
//! RFC 7230 §3.3.2 forbids there ("A server MUST NOT send a Content-Length header field in
//! any response with a status code of 1xx (Informational) or 204 (No Content)"), and that
//! writer is below anything this crate decides. `tests/all/empty_response_framing.rs` pins
//! both halves.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{validate, Violation};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

use crate::state::data_plane::{
    self, IngestOutcome, IngestTarget, PlaneRequest, PlaybackOutcome, PlaybackTarget,
};

/// The `com-amazonaws-mediapackage-mediapackage` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the two data planes are served from [`Service::can_handle`] and
    /// [`Service::override_routing_error`], neither of which is an operation with a
    /// `self.engine` of its own; both must read the one store `CreateChannel` and
    /// `CreateOriginEndpoint` wrote to, not a second copy.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            input_validation: input_validation_gates::<Eng>(),
            engine,
        }
    }

    /// Whether this request addresses the ingest collection of a channel THIS store holds,
    /// through one of that channel's own ingest endpoints.
    ///
    /// The path labels are derived from the resources' own identities, so a path naming a
    /// channel or an endpoint this store does not hold is claimed by nobody and gets the
    /// router's unknown-endpoint refusal — which is what an address this emulator never
    /// published should answer.
    fn holds_ingest_target(&self, target: &IngestTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.ingest_channel_key(target).is_some()
    }

    /// Whether this request addresses the playback path of an origin endpoint THIS store
    /// holds. See [`Self::holds_ingest_target`].
    fn holds_playback_target(&self, target: &PlaybackTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.playback_endpoint_key(&target.endpoint_label).is_some()
    }

    /// Serve one request to the ingest collection of a channel this store holds.
    ///
    /// Every decision — which credentials authenticate, what a verified push stores, what
    /// a read answers — is `crate::state::data_plane`'s, under one hold of the store's
    /// guard; this renders the outcome it reports as the HTTP answer that outcome names,
    /// after the guard is released.
    fn serve_ingest(&self, req: &RtRequest, target: &IngestTarget) -> RtResponse {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return plane_status(STORE_UNAVAILABLE_STATUS);
        };
        let request = plane_request(req);
        let outcome = {
            let mut store = state.lock();
            data_plane::serve_ingest(&mut store, target, &request)
        };
        match outcome {
            IngestOutcome::Challenge(challenge) => {
                let mut response = plane_status(401);
                // RFC 9110 requires a 401 to carry the challenge that asks for
                // credentials, and a Digest client is driven entirely by it: without one
                // it cannot compute a response and can never push.
                response
                    .headers_mut()
                    .insert("www-authenticate", challenge);
                response
            }
            IngestOutcome::SchemeRefused => plane_status(403),
            IngestOutcome::UnknownEndpoint | IngestOutcome::NoSuchObject => plane_status(404),
            IngestOutcome::Stored => plane_status(200),
            IngestOutcome::Served {
                content,
                media_type,
            } => served(req, content, media_type),
            IngestOutcome::MethodRefused => method_refused(INGEST_METHODS),
            IngestOutcome::NameRefused | IngestOutcome::Unreadable => plane_status(400),
            IngestOutcome::ObjectTooLarge => plane_status(413),
            IngestOutcome::StoreFull => plane_status(507),
        }
    }

    /// Serve one request to the playback path of an origin endpoint this store holds. See
    /// [`Self::serve_ingest`] for the division of labour.
    fn serve_playback(&self, req: &RtRequest, target: &PlaybackTarget) -> RtResponse {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return plane_status(STORE_UNAVAILABLE_STATUS);
        };
        let request = plane_request(req);
        let outcome = {
            let store = state.lock();
            data_plane::serve_playback(&store, target, &request)
        };
        match outcome {
            PlaybackOutcome::Forbidden => plane_status(403),
            PlaybackOutcome::UnknownEndpoint | PlaybackOutcome::NoContent => plane_status(404),
            PlaybackOutcome::Served {
                content,
                media_type,
            } => served(req, content, media_type),
            PlaybackOutcome::MethodRefused => method_refused(PLAYBACK_METHODS),
        }
    }
}

/// The methods an ingest collection serves, as the refusal of any other one states them.
const INGEST_METHODS: &str = "GET, HEAD, PUT";

/// The methods a playback path serves: a viewer reads, it does not write.
const PLAYBACK_METHODS: &str = "GET, HEAD";

/// The status a data-plane request is answered with when this service's store is
/// unreachable — the planes' equivalent of the modelled server failure a handler answers
/// with, in a plane that has no modelled envelope to state it in.
const STORE_UNAVAILABLE_STATUS: u16 = 500;

/// The request header the address a request came from is recorded in where it reached the
/// emulator through a forwarder.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The address a request that no forwarder recorded an address for came from: this emulator
/// is reached in process or over the loopback interface, so that is where such a request
/// arrived from.
const LOOPBACK_SOURCE_ADDRESS: &str = "127.0.0.1";

/// One data-plane request, as both planes read it off the wire.
///
/// The source address is the FIRST hop a forwarder recorded, and the loopback address for a
/// request no forwarder recorded one for — the same reading every emulated data plane that
/// reports a caller's source address uses.
fn plane_request<'r>(req: &'r RtRequest) -> PlaneRequest<'r> {
    let source_address = req
        .headers()
        .get(FORWARDED_FOR_HEADER)
        .and_then(|forwarded| forwarded.split(',').next())
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .unwrap_or(LOOPBACK_SOURCE_ADDRESS);
    PlaneRequest {
        method: req.method(),
        // The PATH the request addressed, read by the workspace's one reader of it, so a
        // request that carries its endpoint in an absolute-form URI is read exactly like
        // one that carries it in the host.
        path: ::service_common::request_path(req.uri()),
        authorization: req.headers().get("authorization"),
        source_address,
        body: req.body().bytes(),
        now: std::time::SystemTime::now(),
    }
}

/// One answer of a plane this service serves outside the model: the status, and no body.
///
/// The recorded listeners answer a refusal bare — a status and nothing else — so a client
/// that presents the wrong credentials and one that presents the right ones under the wrong
/// scheme cannot be told apart by what comes back, and a refused viewer learns nothing
/// about the endpoint it asked for. Every refusal of both planes is framed this way, and
/// the framing header states the length the answer has.
fn plane_status(status: u16) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::empty(),
    );
    response.headers_mut().insert("content-length", "0");
    response
}

/// A 405 naming the methods the plane that refused it serves, as RFC 9110 requires of one.
fn method_refused(methods: &str) -> RtResponse {
    let mut response = plane_status(405);
    response.headers_mut().insert("allow", methods.to_string());
    response
}

/// Serve `content` as `media_type`: the bytes, their length and their type — and for a
/// `HEAD`, the same answer without the body, which is what that method asks for.
fn served(req: &RtRequest, content: Vec<u8>, media_type: &'static str) -> RtResponse {
    let length = content.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        if req.method() == "HEAD" {
            SdkBody::empty()
        } else {
            SdkBody::from(content)
        },
    );
    response.headers_mut().insert("content-type", media_type);
    response
        .headers_mut()
        .insert("content-length", length.to_string());
    response
}

/// One [`ModeledInputValidation`] gate per modelled operation.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
fn input_validation_gates<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModeledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation the model binds. What a request must satisfy is a property of its
    // INPUT SHAPE, so the gate goes in front of all of them, implemented or still
    // scaffolded: a request this service will not accept is refused the same way
    // whoever serves it.
    gate!(
        gates;
        configure_logs::ConfigureLogs,
        create_channel::CreateChannel,
        create_harvest_job::CreateHarvestJob,
        create_origin_endpoint::CreateOriginEndpoint,
        delete_channel::DeleteChannel,
        delete_origin_endpoint::DeleteOriginEndpoint,
        describe_channel::DescribeChannel,
        describe_harvest_job::DescribeHarvestJob,
        describe_origin_endpoint::DescribeOriginEndpoint,
        list_channels::ListChannels,
        list_harvest_jobs::ListHarvestJobs,
        list_origin_endpoints::ListOriginEndpoints,
        list_tags_for_resource::ListTagsForResource,
        rotate_channel_credentials::RotateChannelCredentials,
        rotate_ingest_endpoint_credentials::RotateIngestEndpointCredentials,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_channel::UpdateChannel,
        update_origin_endpoint::UpdateOriginEndpoint,
    );
    gates
}

/// This service's input-validation gate in front of one operation `Op`.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape and the schema the
/// constraint walk runs against. It holds no engine and no state, so the operation it
/// guards stays the single owner of the request's behaviour.
struct ModeledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModeledInputValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModeledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req) {
                return refused;
            }
            // Accepted: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// The protocol `svc` answers with: its own [`Service::override_protocol`] when it serves
/// one, else the model-derived protocol of the generated entry.
///
/// The same choice the framework's centralized sequence makes, so a refusal the gate below
/// renders is framed exactly like a response the sequence renders — one protocol for every
/// answer this service gives.
fn resolved_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| svc.definition().protocol())
}

/// `Some(response)` when this service will not accept `Op`'s input: its modelled
/// `UnprocessableEntityException`, HTTP 422, with both body members — the sentence and the
/// per-member `errors` map.
///
/// `None` when the request is one the service accepts, or when it failed to READ for a
/// reason that is not an input failure — an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = resolved_protocol(svc);
    // The service's own page-size bound first: it is narrower than the modelled range, so
    // the wider modelled one would otherwise answer for the values between them with a
    // bound this service does not apply.
    let error = page_size_violation::<Op>(req).or_else(|| {
        first_violation::<Op>(protocol, req).map(|violation| crate::state::member_violation(&violation))
    })?;
    protocol
        .serialize_error(
            &error,
            &crate::scaffolded::SERVICE_ERROR_SCHEMA,
            error.code(),
            error.status(),
        )
        // A modelled error that will not serialize leaves the request to the framework,
        // which refuses it in the generic envelope — the very divergence this gate
        // removes, so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                operation = Op::NAME,
                %error,
                "this service's UnprocessableEntityException would not serialize; this \
                 refusal falls back to the framework's generic envelope"
            );
        })
        .ok()
}

/// `Some(error)` when `Op` is a paginated operation and the request's page size is outside
/// the bound this service enforces.
///
/// Whether the operation HAS a page size is read off its input schema — the member bound
/// to [`crate::state::PAGE_SIZE_QUERY`] — so it is the model that decides which operations
/// this applies to, and a request to one that binds no such member is never refused for
/// carrying that parameter.
fn page_size_violation<Op: Operation>(req: &RtRequest) -> Option<crate::scaffolded::ServiceError> {
    if !binds_page_size::<Op>() {
        return None;
    }
    crate::state::page_size_violation(crate::state::request_query(req.uri()))
}

/// `true` when `Op`'s input binds the page-size query parameter.
fn binds_page_size<Op: Operation>() -> bool {
    for member in Op::INPUT_SCHEMA.members() {
        if member
            .http_query()
            .is_some_and(|query| query.value() == crate::state::PAGE_SIZE_QUERY)
        {
            return true;
        }
    }
    false
}

/// The first constraint `Op`'s input violates, as the framework's own walk reports it.
///
/// Both validation classes are classified by the framework's own
/// [`smithy_server_runtime::refusal_of`], so this carries no second copy of "what counts
/// as a constraint refusal": a value outside a declared bound is found by walking what
/// was read, and a required member absent from the request is found while READING it, and
/// `refusal_of` already states that they are one refusal. Anything it does not recognise
/// (an unreadable body, a missing header) is not a constraint refusal and is left to the
/// framework, which words it per protocol.
///
/// The walk visits members in schema order, so "the first" is a property of the input
/// shape rather than of the request.
fn first_violation<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Violation> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => {
                    let violations = validate(&input, Op::INPUT_SCHEMA);
                    if violations.is_empty() {
                        return None;
                    }
                    ProtocolError::Invalid(violations)
                }
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error)? {
        Refusal::Invalid(violations) => violations.into_iter().next(),
        // Authentication is settled by the router before any operation runs, so an
        // `Unauthenticated` refusal cannot arrive here — and it is not an input
        // failure in any case.
        _ => None,
    }
}

/// How a response with no body is framed: with the `content-length: 0` that states its
/// length, rather than with no framing header at all. See the module docs.
///
/// Only an EMPTY body is touched, and only that one header: a response that carries a body
/// is framed from the body it carries, which is already right.
fn frame_empty_body(response: &mut RtResponse) {
    if response.body().bytes().is_some_and(<[u8]>::is_empty) {
        response.headers_mut().insert("content-length", "0");
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry.protocol().deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status, the body and every other header are the protocol's: only the framing
        // of an answer that has no body is added.
        frame_empty_body(&mut response);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries the `message`/`errors` envelope,
        // so this body is never empty; the same framing is applied for the same reason
        // rather than leaving one of the two paths to answer differently.
        frame_empty_body(&mut response);
        Ok(response)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
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
        // A request that FOLLOWS one of the URLs this service published for a channel's
        // ingest endpoints or an origin endpoint's playback path is claimed by the store
        // that minted it, whatever else the request carries: such a URL is followed
        // WITHOUT a signature (an encoder authenticates with the endpoint's own
        // credentials, a viewer with nothing at all) and its published host names no
        // service the front door resolves, so the route key cannot route it. Neither path
        // binds a modelled operation, so nothing modelled is diverted here, and a path
        // whose label names no resource THIS store holds is not claimed at all. See
        // `crate::state::data_plane`.
        if let Some(target) = crate::state::data_plane::parse_ingest_path(parts.uri.path()) {
            return self.holds_ingest_target(&target);
        }
        if let Some(target) = crate::state::data_plane::parse_playback_path(parts.uri.path()) {
            return self.holds_playback_target(&target);
        }
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
        // Put this service's input-validation gate in front of the modelled operation
        // the request names, so a request it will not accept is refused with the
        // MODELLED `UnprocessableEntityException` (422, both body members) instead of the
        // framework's generic 400 `ValidationException` envelope — see the module docs.
        // The gate then hands every request the service accepts to the generated
        // operation, which the routing table still owns; a request that names no
        // modelled operation resolves as it always did.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.input_validation.get(key)?;
        Some(gate.as_ref())
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by
        // the protocol returned here. See the module docs: it is the model's own protocol,
        // with one correction — an answer with no body is framed with `content-length: 0`
        // instead of with no framing header at all.
        Some(self)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Left to the framework DELIBERATELY. Every input failure this service states is
        // answered by the gate above, before the centralized sequence reaches this seam,
        // and `Refused` carries only a code and a message: rewording a refusal here would
        // name `UnprocessableEntityException` on a response the protocol still sends as
        // 400 with no `errors` member, which is a worse answer than the framework's own —
        // a code that does not match its status. The only refusals that reach here are
        // the ones the gate leaves alone (authentication, an unreadable body), which are
        // the emulator's and the protocol's rather than this service's.
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
        //
        // A request that follows one of the URLs this service PUBLISHED is exactly that
        // case by construction — the model binds no operation to an ingest or a playback
        // path — so this is where the two data planes are served: the seam is reached
        // BEFORE authentication precisely for a request that names no modelled operation,
        // which is what lets an encoder authenticate with its ingest credentials and a
        // viewer with none at all. Anything else keeps the framework's default.
        let path = ::service_common::request_path(req.uri());
        if let Some(target) = crate::state::data_plane::parse_ingest_path(path) {
            return Some(self.serve_ingest(req, &target));
        }
        let target = crate::state::data_plane::parse_playback_path(path)?;
        Some(self.serve_playback(req, &target))
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
    /// This crate's shipped Smithy model, parsed.
    ///
    /// The tests below read the MODEL rather than a copy of it, so a value this file or
    /// `crate::scaffolded` states about the service's errors cannot drift from what the
    /// model declares.
    fn model() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/smithy-model/com-amazonaws-mediapackage-mediapackage.json"
        );
        let model = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
        serde_json::from_str(&model).expect("the Smithy model is JSON")
    }

    /// Every error the envelope answers with carries the code and the status the model
    /// declares for that shape.
    ///
    /// `crate::scaffolded` states each pair as a constant, because the envelope is not a
    /// modelled shape and so cannot read them off one; this is what keeps those constants
    /// in step with the model, so an `httpError` change cannot leave the envelope
    /// answering a stale status under a name no shape carries.
    #[test]
    fn every_envelope_code_and_status_is_the_models() {
        let model = model();
        let shapes = model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes");
        let pairs = [
            (
                crate::scaffolded::NOT_FOUND_CODE,
                crate::scaffolded::NOT_FOUND_STATUS,
            ),
            (
                crate::scaffolded::UNPROCESSABLE_ENTITY_CODE,
                crate::scaffolded::UNPROCESSABLE_ENTITY_STATUS,
            ),
            (
                crate::scaffolded::TOO_MANY_REQUESTS_CODE,
                crate::scaffolded::TOO_MANY_REQUESTS_STATUS,
            ),
        ];
        for (code, status) in pairs {
            let id = format!("com.amazonaws.mediapackage#{code}");
            let shape = shapes
                .get(&id)
                .unwrap_or_else(|| panic!("the model declares no error shape named {code}"));
            assert_eq!(
                shape.get("type").and_then(serde_json::Value::as_str),
                Some("structure"),
                "{code} must be the error STRUCTURE the envelope names, got {shape:?}"
            );
            let traits = shape.get("traits").expect("an error shape carries traits");
            assert!(
                traits.get("smithy.api#error").is_some(),
                "{code} must be an @error shape, got {traits:?}"
            );
            let declared = traits
                .get("smithy.api#httpError")
                .and_then(serde_json::Value::as_u64);
            assert_eq!(
                declared,
                Some(u64::from(status)),
                "{code}'s modelled httpError must be the status the envelope answers \
                 ({status}), got {declared:?}"
            );
        }
    }

    /// Every operation the model declares has an input-validation gate.
    ///
    /// The gate set is written out by name in [`super::input_validation_gates`] — the
    /// operation TYPE is what names the input shape and its schema, and the erased
    /// routing table cannot hand that back — so this is what keeps the two in step: an
    /// operation added to the model (and so to `src/ops/`) without a gate would quietly
    /// go back to answering a refusal in the framework's generic 400 envelope, and this
    /// test fails naming it.
    #[test]
    fn every_modeled_operation_has_an_input_validation_gate() {
        let model = model();
        let shapes = model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape.get("type").is_some_and(|kind| kind == "operation"))
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            !operations.is_empty(),
            "the model declares no operations, so this test proves nothing"
        );
        let gates = super::input_validation_gates::<engine::InMemoryEngine>();
        for operation in &operations {
            assert!(
                gates.contains_key(*operation),
                "operation {operation} has no input-validation gate, so a request this \
                 service will not accept would be refused without the modelled status, \
                 code and members"
            );
        }
        assert_eq!(
            gates.len(),
            operations.len(),
            "the gate set and the model's operations must match exactly, got {} gates for \
             {} operations",
            gates.len(),
            operations.len()
        );
    }
}
