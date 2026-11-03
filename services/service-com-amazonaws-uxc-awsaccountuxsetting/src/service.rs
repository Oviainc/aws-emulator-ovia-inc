// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-uxc-awsaccountuxsetting` service: how it claims requests and how it words the
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
//! ## This service's one edited seam: the front door
//!
//! This service refuses a request BODY that breaks a modelled shape constraint
//! at its front door, before the request reaches the application code that
//! raises the modelled `ValidationException`: the answer is HTTP 400 with the
//! error code `BadRequestException`, a code this service's model does not
//! declare (`smithy_query(query="operation", operation="UpdateAccountCustomizations")`
//! lists `AccessDeniedException`/`InternalServerException`/`ThrottlingException`/`ValidationException`
//! and nothing else). A constraint on a value bound to the request URI — a
//! `@httpQuery` parameter such as `ListServices`' `maxResults`/`nextToken`, a
//! `@httpLabel` path segment — is NOT a body rejection and keeps the modelled
//! `ValidationException`, and so does every rule a handler enforces that the
//! model states only in prose (a `visibleServices` member that is not in the
//! service catalog, a `nextToken` that is not a cursor this service issued).
//!
//! The framework refuses every modelled constraint before a handler runs and
//! words it through [`Service::override_refusal`], which is handed the
//! violations alone — no operation, no request — so it cannot tell a body
//! violation from a URI one. [`FrontDoor`] is where that distinction lives:
//! each modelled operation is served wrapped in it, and the wrapper reads the
//! HTTP BINDING of the violated member off that operation's own input schema.
//!
//! The front door refuses a body it cannot READ on the same terms as one it read
//! and rejected: a document that is not well-formed JSON, a well-formed document
//! whose JSON type is not the object an input structure is read from, and a member
//! whose JSON type is not the type the model gives it never become an input at
//! all, and the front door that validates a body against the shape will not act on
//! any of them, so they all earn the same `BadRequestException` answer. Whether
//! the document is an object is asked of the BYTES ([`non_object_document`])
//! rather than left to the codec, which reads some non-object documents as a
//! structure with every member absent. Only the request's BODY earns the refusal:
//! WHICH part of the request the codec could not read is decided by reading the
//! same request again with no body at all (see [`read_request`]), so a value bound
//! to the URI keeps the modelled wording.
//!
//! A request with no body, and one carrying the empty object, are not such
//! bodies: they supply no member, which for an update is a request that sets
//! nothing, not one the service cannot read.
//!
//! The same wrapper is why a URI-bound violation is answered with the MODELLED
//! `ValidationException` value rather than through the refusal channel: that
//! channel carries a code and a message only ([`Refused`]), while the modelled
//! shape also reports the refused members structurally in `fieldList`
//! (`smithy_query(query="shape", shape="ValidationException")`). The wrapper
//! builds that shape from the violations it already holds and serializes it
//! through this service's own protocol, so a boundary refusal and a
//! handler-raised `ValidationException` reach the wire as the same document.
//!
//! ## The second thing the wrapper reads off the request: a member sent as `null`
//!
//! A JSON body distinguishes a member it OMITS from one it carries with the value
//! `null`, and this service's update operation gives the two different meanings
//! (see [`BodyMembersSentAsNull`]). The typed input a handler is handed cannot:
//! the codec reads an absent member and a `null` one into the same `Option::None`
//! / empty collection, so the distinction is gone by the time `handle` runs. The
//! wrapper, which is handed the request bytes, is where it is recovered: it names
//! the body members THIS request carried as `null` and passes them to the handler
//! as a request extension, which is the channel request metadata already reaches a
//! handler through ([`smithy_server_runtime::OperationContext::parts`]).

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, path};
use aws_smithy_schema_server::{envelope, ProtocolError, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

/// The error code AWS answers a constraint refusal with on the JSON protocols, and the
/// one the framework's own default picks for this `restJson1` service — named here so
/// the chosen message always travels with its matching code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `regex` without the whole-value anchors a model wraps it in.
///
/// The requirement a refusal states is the expression a value has to satisfy; the `^` /
/// `$` are how Smithy turns a find into a whole-string match rather than part of the
/// character grammar, so AWS quotes the expression without them and a client that lifts
/// it out of the message and re-validates locally gets the one the service advertises.
/// The CHECK always runs the model's pattern as written: only this rendering drops the
/// pair.
///
/// The pair is dropped POSITIONALLY — the leading `^` and the trailing `$`, and only when
/// BOTH ends carry one. A `^` alone, or a `$` alone, is an expression anchored at one end
/// (a different expression, not an anchored spelling of this one), as is one whose own
/// branches carry the anchors (`(^a$)|(^b$)`); each is emitted verbatim. An ESCAPED `$`
/// is a literal dollar, not an anchor, so nothing is dropped.
///
/// THE RULE'S STAGED HOME is `services/common`'s `validation::unanchored`, which states
/// it once for every service and is table-tested there; this crate carries the inlined
/// copy its ~26 peers carry (`grep -rln 'fn unanchored' services/*/src`), to be deleted
/// by the change that owns that migration. Kept byte-identical in behaviour to that one,
/// and pinned end-to-end by `tests/all/pattern_refusal_rendering.rs`.
fn unanchored(regex: &str) -> String {
    let Some(body) = regex.strip_prefix('^') else {
        return regex.to_string();
    };
    match body.strip_suffix('$') {
        // An odd number of trailing backslashes escaped the `$`, so it is a literal.
        Some(inner) if inner.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0 => {
            inner.to_string()
        }
        _ => regex.to_string(),
    }
}

/// The error code this service's front door names for a request body that
/// breaks a modelled constraint. NOT a shape in this service's model: the
/// rejection happens before the application code that owns the modelled errors,
/// so it is worded through the framework's refusal channel
/// ([`smithy_server_runtime::Refused`]) rather than through an error enum.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The HTTP status the model declares for this service's `ValidationException`
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// What the front door says about a body it refused.
///
/// The front door validates the body against the shape and reports the class of
/// failure, not the member: a caller fixes the request from the constraint the
/// model publishes. (The member-by-member clause the framework writes is what
/// this service's modelled `ValidationException` carries, for the refusals the
/// application layer owns.)
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The `com-amazonaws-uxc-awsaccountuxsetting` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`FrontDoor`] so a body that breaks a modelled constraint is refused with
    /// `BadRequestException` before the operation runs. Keyed by the routing key
    /// the protocol resolves — the key the generated table uses — and built from
    /// the SAME shared engine, so a wrapped operation reads and writes the one
    /// store.
    front_door: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each
/// wrapped in [`FrontDoor`] and keyed by its operation shape name — the routing
/// key `ServerProtocol::routing_key` resolves and the generated table is keyed
/// by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(FrontDoor::new(
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
        let mut front_door = OperationRoutingTable::new();
        // EVERY modelled operation, so the front door's rule is applied by one
        // code path rather than per operation: an operation that binds nothing
        // to the body (both reads here) simply never has a body violation to
        // find, which the wrapper reads off its input schema.
        wrap_operations!(
            front_door,
            engine,
            get_account_customizations::GetAccountCustomizations,
            list_services::ListServices,
            update_account_customizations::UpdateAccountCustomizations,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            front_door,
        }
    }
}

/// One modelled operation, served behind this service's front door.
///
/// WHY this is a seam and not a line in each handler: the framework validates
/// every modelled constraint BEFORE a handler runs, so a request that breaks one
/// never reaches `handle` — and the seam that words that refusal
/// ([`Service::override_refusal`]) is handed the violations alone, with no way to
/// tell which HTTP location the violated value came from, nor any channel for the
/// `fieldList` the modelled exception carries. A wrapper is handed the request
/// and the operation, so it has both: it places each violation by the binding the
/// operation's input schema gives that member, and answers a BODY one with
/// [`BAD_REQUEST_EXCEPTION`] and a URI-bound one (`@httpLabel`, `@httpQuery`,
/// `@httpHeader`) with the modelled `ValidationException` naming every refused
/// member ([`request_rejection`]).
///
/// It owns the request the codec could not read for the same reason: a body that
/// never became an input is one the service will not act on, and placing it needs
/// the same operation-level reading ([`read_request`]).
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is
/// handed to the generated `ErasedOperation::run`, which owns deserialize ->
/// validate -> handle -> serialize, telemetry and every modelled error — including
/// the ones a handler raises for the rules the model states only in prose.
///
/// The violations come from the framework's own machinery (the protocol's
/// deserializer and `constraints::validate` over the built input), so this
/// cannot find a different set from the walk that follows it.
struct FrontDoor<O> {
    operation: O,
}

impl<O> FrontDoor<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for FrontDoor<O>
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
            if let Some(refusal) = request_rejection::<O>(svc, req) {
                return refusal;
            }
            // Which body members this request carried as an explicit `null` —
            // read off the bytes here, because the typed input the handler is
            // handed no longer tells a `null` member from an omitted one.
            let sent_as_null = members_sent_as_null::<O>(req);
            if sent_as_null.is_empty() {
                // Nothing to carry: the request runs under the context the
                // router built, untouched.
                return self.operation.run(svc, req, ctx).await;
            }
            // The context is rebuilt rather than mutated: a handler reads request
            // metadata through a SHARED borrow of the router's parts, so this is
            // the one way to add to them. Everything else — the headers, the URI,
            // the extensions the router and the middleware set, the service
            // identity and the callback into this emulator — is carried over
            // unchanged, so the handler sees the same request it otherwise would.
            let parts = parts_with_extension(ctx.parts(), BodyMembersSentAsNull(sent_as_null));
            let carried = OperationContext::new(&parts, ctx.identity(), ctx.callback().clone());
            self.operation.run(svc, req, &carried).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must
    /// not change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The request-body members of one request that carried an explicit JSON `null`,
/// each by the name the model gives it, as a handler reads them
/// ([`sent_as_null`]).
///
/// # The distinction this carries, and why it needs carrying
///
/// A JSON object can say three different things about a member: it can omit it,
/// it can carry it with a value, or it can carry it with `null`. The update
/// operation of this service gives all three a meaning — "Only the settings that
/// you include in the request body are modified. Omitted settings remain
/// unchanged. To reset a setting to its default behavior, set the value to null"
/// (`smithy_query(query="doc", shape="UpdateAccountCustomizations")`) — so a
/// handler that cannot tell the first form from the third cannot serve the
/// documented reset at all.
///
/// The typed boundary cannot tell them apart: the codec reads a `null` member and
/// an omitted one into the same value (`None` for a scalar, an empty collection
/// for a list), by design — that is what makes a Smithy client's `null` mean
/// "unset" on every protocol. So the set of `null`-valued members is read off the
/// request bytes at this service's front door, which is handed them, and travels
/// to the handler as a request extension.
///
/// Only TOP-LEVEL body members are named: it is the operation's own input members
/// a request resets, and a `null` nested inside a member's value is that value's
/// own business.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BodyMembersSentAsNull(Vec<String>);

impl BodyMembersSentAsNull {
    /// Whether `member` is one of the members this request sent as `null`.
    fn names(&self, member: &str) -> bool {
        self.0.iter().any(|named| named == member)
    }
}

/// Whether THIS request carried the body member `member` with an explicit JSON
/// `null` — the request form a handler serves differently from an omitted member.
///
/// `false` for a request that omitted it, for one that carried a value, and for
/// every operation whose request never reached the front door that records this
/// (nothing recorded means nothing was sent as `null`).
pub fn sent_as_null(ctx: &OperationContext<'_>, member: &str) -> bool {
    ctx.parts()
        .extensions
        .get::<BodyMembersSentAsNull>()
        .is_some_and(|sent| sent.names(member))
}

/// Every top-level body member of `O`'s input that `req` carried as an explicit
/// JSON `null`, by the member name the model declares.
///
/// The model decides what to look for and where: the members come from `O`'s own
/// input schema, each is looked up under the name it travels on the wire (its
/// `@jsonName` if it declares one, else its member name), and a member bound to
/// the URI rather than the body is not looked for at all.
///
/// Empty for a request with no body, a body that is not a JSON object, and a body
/// this operation's input declares no `null` member of — including a body the
/// codec cannot read and one that is well-formed JSON but not an object, both of
/// which are refused before this is reached ([`read_request`]) for every operation
/// that carries a member in the body, rather than re-judged here.
fn members_sent_as_null<O: Operation>(req: &RtRequest) -> Vec<String> {
    let Some(bytes) = req.body().bytes() else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Object(body)) = serde_json::from_slice::<serde_json::Value>(bytes)
    else {
        return Vec::new();
    };
    O::INPUT_SCHEMA
        .members()
        .iter()
        .filter(|member| carried_in_the_body(member))
        .filter_map(|member| {
            let name = member.member_name()?;
            let wire_name = member.json_name().map_or(name, |json| json.value());
            body.get(wire_name)
                .is_some_and(serde_json::Value::is_null)
                .then(|| name.to_string())
        })
        .collect()
}

/// `parts` with `extension` added: the request metadata the router built, plus the
/// one fact this service's front door read off the body.
///
/// Built from a fresh request rather than copied field by field into a literal —
/// `http::request::Parts` has a private member, so a literal cannot name it — and
/// every public field is carried over, so nothing the router or the middleware put
/// on the request is dropped on the way to the handler.
fn parts_with_extension<T>(parts: &http::request::Parts, extension: T) -> http::request::Parts
where
    T: Clone + Send + Sync + 'static,
{
    let (mut carried, ()) = http::Request::new(()).into_parts();
    carried.method = parts.method.clone();
    carried.uri = parts.uri.clone();
    carried.version = parts.version;
    carried.headers = parts.headers.clone();
    carried.extensions = parts.extensions.clone();
    carried.extensions.insert(extension);
    carried
}

/// What the front door found when it read `req` against `O`'s input schema.
enum RequestReading {
    /// The request WAS read into `O`'s input shape, and these are the modelled
    /// member constraints it breaks — empty when it breaks none.
    Read(Vec<Violation>),
    /// The request's BODY is what the codec could not read into that shape,
    /// carrying the failure the codec reported.
    UnreadableBody(ProtocolError),
    /// The codec could not read the request, and taking the body away does not
    /// change that: what it could not read did not travel in the body, so it is
    /// left to the framework's own walk, which refuses it through this service's
    /// refusal wording.
    UnreadableRequest,
}

/// Read `req` into `O`'s input shape with the framework's own machinery — the
/// protocol's deserializer and the generated `DeserializableStruct` the dispatch
/// sequence uses — so nothing about the codec or the model is restated here.
fn read_input<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Result<O::Input, ProtocolError> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA)?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer)
        .map_err(ProtocolError::from)?;
    drop(deserializer);
    Ok(input)
}

/// What the front door has to say about `req`, for the operation `O`: the
/// modelled constraints it breaks, or the fact that its body could not be read
/// into the input shape at all.
///
/// A body the codec cannot read is one this service will not act on just as much
/// as a body whose value breaks a declared constraint, so the front door owns it
/// too. WHICH part of the request the codec could not read is what decides it,
/// and that is read with the codec rather than guessed: the same request is read
/// again with NO BODY, and only a request the rest of which reads cleanly on its
/// own is reported as an unreadable body. An operation that carries no member in
/// the body validates no body, so a codec failure there is never one of its
/// ([`reads_a_body`]).
///
/// One class of body is placed WITHOUT the codec, before it is asked: a body that
/// is a well-formed JSON document of a type other than an object travelled in the
/// body by definition and declares no member of the input structure, so it is a
/// body refusal whether or not the codec reports it ([`non_object_document`]).
fn read_request<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> RequestReading {
    // Asked first, and asked of the request bytes: a well-formed document that is
    // not a JSON object supplies no member of the input structure however the
    // codec happens to treat it, so it is a body refusal before anything is read
    // into the shape.
    if let Some(failure) = non_object_document(O::INPUT_SCHEMA, req) {
        return RequestReading::UnreadableBody(failure);
    }
    let failure = match read_input::<O>(protocol, req) {
        Ok(input) => return RequestReading::Read(constraints::validate(&input, O::INPUT_SCHEMA)),
        Err(failure) => failure,
    };
    if !reads_a_body(O::INPUT_SCHEMA) {
        return RequestReading::UnreadableRequest;
    }
    match without_body(req) {
        Some(stripped) if read_input::<O>(protocol, &stripped).is_ok() => {
            RequestReading::UnreadableBody(failure)
        }
        _ => RequestReading::UnreadableRequest,
    }
}

/// The failure to refuse `req` with when its body is a well-formed JSON document
/// that is NOT a JSON object, or `None` when its body is not one of those.
///
/// An operation that carries members in the request body is defined over a
/// structure, and the only JSON document a structure is read from is an object: a
/// document of any other JSON type declares no member, so it is a body the service
/// will not act on just as much as one that is not well-formed JSON. The codec
/// reports most of them by failing, but not all — a well-formed document can be
/// read as a structure with every member absent — so the question is asked of the
/// bytes here, which is also where the neighbouring reading of the body lives
/// ([`members_sent_as_null`]).
///
/// `None` for a body that IS a JSON object, for a request with no body, for a body
/// that is not a well-formed JSON document at all — which the codec reports
/// itself, with its own failure, so this does not restate what it will say — and
/// for an operation that carries no member in the body, which validates no body
/// ([`reads_a_body`]).
///
/// The reading holds while the body IS the input structure, which is every
/// operation of this model: no member of it binds `@httpPayload`, which would make
/// the body one member's own value and a bare JSON scalar a legal document for it.
fn non_object_document(input_schema: &Schema, req: &RtRequest) -> Option<ProtocolError> {
    if !reads_a_body(input_schema) {
        return None;
    }
    let bytes = req.body().bytes()?;
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(serde_json::Value::Object(_)) | Err(_) => None,
        Ok(document) => Some(ProtocolError::Malformed(format!(
            "the request body is a JSON {}, and a request body is read from an object",
            json_type_name(&document)
        ))),
    }
}

/// The JSON type of `document`, by the name JSON gives that type.
fn json_type_name(document: &serde_json::Value) -> &'static str {
    match document {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Whether `input_schema` carries any member in the request body — the only
/// operations that have a body to validate at all.
fn reads_a_body(input_schema: &Schema) -> bool {
    input_schema
        .members()
        .iter()
        .any(|member| carried_in_the_body(member))
}

/// `req` with its BODY taken away: the same method, the same request target and
/// the same headers, carrying no body bytes.
///
/// What is left is the half of the request that does NOT travel in the body, so
/// reading it is how the front door tells a body the codec could not read from a
/// URI-bound value it could not read. `None` when the request cannot be restated
/// — its target or method does not parse — which is reported as a failure the
/// body was not shown to own.
fn without_body(req: &RtRequest) -> Option<RtRequest> {
    let mut stripped = RtRequest::new(aws_smithy_types::body::SdkBody::empty());
    stripped.set_method(req.method()).ok()?;
    stripped.set_uri(req.uri()).ok()?;
    for (name, value) in req.headers().iter() {
        // `append`, not `insert`: a repeated or multi-valued header keeps every
        // value, so a member bound to one reads exactly what was sent.
        stripped
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Some(stripped)
}

/// How this service answers a request for `O` its front door refuses, or `None`
/// when it refuses nothing and the operation should run.
///
/// The refusals this service words differ only in WHERE what it refused
/// travelled, which is read off `O`'s own input schema:
///
/// * the request BODY — a member whose value breaks a modelled constraint, or a
///   body the codec could not read into the input shape — keeps the front door's
///   `BadRequestException`;
/// * a member bound to the request URI (`@httpQuery`, `@httpLabel`,
///   `@httpHeader`) is refused with the modelled `ValidationException`, whose
///   `fieldList` names every refused member.
///
/// A request breaking both is a body rejection: the front door refuses a body it
/// will not read before anything is said about the rest of the request.
fn request_rejection<O: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violations = match read_request::<O>(protocol, req) {
        RequestReading::Read(violations) => violations,
        RequestReading::UnreadableBody(failure) => {
            return Some(front_door_rejection(svc, req, failure));
        }
        RequestReading::UnreadableRequest => return None,
    };
    if violations.is_empty() {
        return None;
    }
    let (body, uri): (Vec<Violation>, Vec<Violation>) = violations
        .into_iter()
        .partition(|violation| binds_to_body(O::INPUT_SCHEMA, &violation.path));
    if !body.is_empty() {
        return Some(front_door_rejection(svc, req, ProtocolError::Invalid(body)));
    }
    Some(member_constraint_rejection(svc, req, protocol, uri))
}

/// Whether `violation_path` names a member `input_schema` carries in the request
/// BODY.
///
/// Read off the member's own HTTP binding traits, so the model decides: a member
/// with no `@httpLabel` / `@httpQuery` / `@httpQueryParams` / `@httpHeader` /
/// `@httpPrefixHeaders` binding travels in the body. A nested path
/// (`visibleServices.1.member`) is placed by its FIRST segment, the top-level
/// member the whole subtree hangs off.
fn binds_to_body(input_schema: &Schema, violation_path: &str) -> bool {
    let member_name = violation_path.split('.').next().unwrap_or(violation_path);
    input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_name)
            && carried_in_the_body(member)
    })
}

/// Whether `member` travels in the request BODY, read off its own HTTP binding
/// traits: a member with no `@httpLabel` / `@httpQuery` / `@httpQueryParams` /
/// `@httpHeader` / `@httpPrefixHeaders` binding is a body member.
///
/// The one place that reading is written down, so the two things this service's
/// front door does with it — placing a constraint violation ([`binds_to_body`])
/// and looking for a member sent as `null` ([`members_sent_as_null`]) — cannot
/// disagree about where a member travels.
fn carried_in_the_body(member: &Schema) -> bool {
    member.http_label().is_none()
        && member.http_query().is_none()
        && member.http_query_params().is_none()
        && member.http_header().is_none()
        && member.http_prefix_headers().is_none()
}

/// The front door's answer to a body it refused: HTTP 400 with the error code
/// [`BAD_REQUEST_EXCEPTION`], rendered in THIS service's protocol envelope.
///
/// `refused` is the failure as the framework stated it — the broken constraints
/// for a body that was read, the codec's own error for one that could not be —
/// and it is carried in rather than rebuilt so the status and the envelope stay
/// with the framework's refusal channel (`refusal_response`, which the generated
/// entry renders from the service's model-derived protocol). Only the code and
/// the message are this service's, and they travel together: a client branches on
/// the code, so one without the other is worse than neither.
fn front_door_rejection(svc: &dyn Service, req: &RtRequest, refused: ProtocolError) -> RtResponse {
    svc.definition().refusal_response(
        refused,
        Some(Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: INVALID_REQUEST_BODY.to_string(),
        }),
        req,
    )
}

/// `violations` with every requirement stated the way THIS service states it: a
/// `@pattern` quotes the expression without the model's whole-value anchors
/// ([`unanchored`]), and every other kind stays the framework's own, so a member
/// path or a requirement cannot drift from the walk that produced it.
fn restated(violations: &[Violation]) -> Vec<Violation> {
    violations
        .iter()
        .map(|violation| match &violation.kind {
            ViolationKind::Pattern(pattern) => Violation {
                path: violation.path.clone(),
                kind: ViolationKind::Pattern(unanchored(pattern)),
                value: violation.value.clone(),
                sensitive: violation.sensitive,
            },
            _ => violation.clone(),
        })
        .collect()
}

/// The modelled `ValidationException` for member-constraint `violations`: the
/// AWS envelope in `message`, and the SAME clauses reported member by member in
/// `fieldList` — one entry per refused member, `path` its dotted member path and
/// `message` that member's clause.
///
/// `None` when there is nothing to state (no violations), since the exception's
/// `message` is a required member.
fn member_constraint_exception(
    violations: &[Violation],
) -> Option<crate::ops::types::ValidationException> {
    let restated = restated(violations);
    Some(crate::ops::types::ValidationException {
        message: envelope(&restated)?,
        field_list: Some(
            restated
                .iter()
                .map(|violation| crate::ops::types::ValidationExceptionField {
                    path: violation.path.clone(),
                    message: violation.message(),
                })
                .collect(),
        ),
    })
}

/// The answer to a URI-bound member this service refused: the modelled
/// `ValidationException`, serialized through this service's own protocol so the
/// `fieldList` the model declares reaches the wire.
///
/// Falls back to the framework's refusal channel — the same code, status and
/// envelope message, without `fieldList` — if the modelled shape cannot be
/// serialized, so a refusal is never dropped over a rendering failure.
fn member_constraint_rejection(
    svc: &dyn Service,
    req: &RtRequest,
    protocol: &dyn ServerHttpProtocol,
    violations: Vec<Violation>,
) -> RtResponse {
    let rendered = member_constraint_exception(&violations).and_then(|exception| {
        protocol
            .serialize_error(
                &exception,
                &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                VALIDATION_EXCEPTION,
                VALIDATION_EXCEPTION_STATUS,
            )
            .ok()
    });
    rendered.unwrap_or_else(|| {
        let chosen = svc.override_refusal(&Refusal::Invalid(violations.clone()));
        svc.definition()
            .refusal_response(ProtocolError::Invalid(violations), chosen, req)
    })
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
        // `FrontDoor` so a request body that breaks a modelled constraint is
        // refused with `BadRequestException` before the operation runs, the way
        // this service's front door refuses it. A key the model does not name
        // resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service state it. What reaches it here is a refusal `FrontDoor` could
        // not place in the request BODY — an absent required member, or a value
        // the codec could not read that did not travel in the body — since a
        // violation of a modelled member constraint, and a body the codec could
        // not read, are both answered by `FrontDoor` before the framework's walk
        // is reached: a body one with `BadRequestException`, a URI-bound one with
        // the modelled `ValidationException` and its `fieldList`.
        //
        // AWS states it as the `400 ValidationException` envelope the framework
        // already renders — `N validation error(s) detected: Value '<value>' at
        // '<member path>' failed to satisfy constraint: <requirement>` — with ONE
        // difference: a `@pattern` requirement quotes the expression a value has to
        // satisfy WITHOUT the model's anchors ([`unanchored`]). So only that one
        // requirement kind is restated: the code, the envelope, every member path
        // and every other requirement stay the framework's own, and cannot drift
        // from the walk that produced these violations.
        //
        // `None` for anything that is not a constraint refusal — an unauthenticated
        // caller keeps the framework's 403 code and wording — and `None` when there
        // is nothing to state.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(&restated(violations))?,
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
