// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ec2-amazonec2` service: how it claims requests and how it words the
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
//! Six opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `override_protocol` — the wire protocol the dispatch sequence runs. This
//!   service overrides it with [`Ec2QueryEnvelope`], which is the framework's
//!   ec2Query protocol plus the three parts of EC2's response envelope the Smithy
//!   protocol definition does not describe: the `<return>` success flag, the
//!   `<RequestID>` element of a refusal, and the media type's `charset`
//!   parameter. It reaches every response this service renders and none of the
//!   ones the framework renders in front of it; see `RESPONSE_CONTENT_TYPE` for
//!   exactly where the line falls.
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — which operation serves a request. This service serves
//!   every modelled operation through [`FinishedEnvelope`], which runs that same
//!   generated operation and then finishes whatever it answered with the envelope
//!   above — the one response of the dispatch sequence `override_protocol` cannot
//!   reach being its framework-level refusal tail (a value outside a declared
//!   constraint), which the sequence renders itself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. This service takes the entry's
//!   protocol-correct rendering (query -> 400 InvalidAction) and stamps EC2's
//!   error envelope on it, so a routing failure closes with the same
//!   `<RequestID>` and media type every other error does.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). Answered the same way as the routing failure above.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::{ProtocolError, QueryServerProtocol, ServerProtocol};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-ec2-amazonec2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's ec2Query protocol, built once here and served through
    /// [`Service::override_protocol`]. See [`Ec2QueryEnvelope`] for what it adds
    /// to the generated one.
    protocol: Ec2QueryEnvelope,
    /// The decorator every modelled operation is served through
    /// ([`Service::override_operation`]), so the one response the protocol above
    /// cannot reach carries the same envelope as every other. Carries no state of
    /// its own: it resolves the operation it runs from the service the framework
    /// hands it.
    envelope: FinishedEnvelope,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            // Built once, per `override_protocol`'s contract: never per request.
            protocol: Ec2QueryEnvelope::new(),
            envelope: FinishedEnvelope,
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
        // Every modelled operation is served through [`FinishedEnvelope`], which runs
        // the generated operation the routing table names and then finishes its
        // response with this service's envelope. Claimed only for a routing key that
        // table actually serves, so a request naming no operation of this service
        // still reaches `override_routing_error` rather than an operation that would
        // have nothing to run.
        modeled_operation(self, req)?;
        Some(&self.envelope)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        match refusal {
            // The framework refused a value against a rule the model declares — an absent
            // `@required` member, or a value outside a declared `@range`, `@length`,
            // `@pattern` or enum catalog. EC2 declares no error SHAPES at all
            // (`smithy_query(query="operation", …)` reports `errors: []` for every
            // operation), so what it answers is a CODE, and this service states that code
            // only where EC2's own words for the refusal are known: a value outside a
            // modelled catalog this service has recorded wording for is EC2's
            // `InvalidParameterValue` with the sentence EC2 words for that parameter TYPE,
            // and a page size below the smallest its `@range` publishes is EC2's
            // `InvalidMaxResults`, and a page size that is no page size at all — zero or
            // negative — is EC2's `InvalidParameter` ([`ec2_value_refusal`]).
            //
            // Every other framework refusal answers `None`, which keeps the framework's own
            // code and its request-built clause in AWS's validation envelope
            // ([`aws_smithy_schema_server::envelope`]) rather than inventing a second
            // dialect for a parameter whose EC2 wording this service does not know. Which
            // code EC2 answers for each of those, and how it words them, is one rule for the
            // whole service and is recorded as its own issue
            // (`issues/.../20261023T102725Z-ec2-answers-parameter-rejections-with-generic-validationerror-and.md`),
            // to be stated here once when that issue is fixed. The refusals this service's
            // HANDLERS give for the rules the model cannot express are a different path and
            // already carry EC2's own codes (`crate::state`).
            Refusal::Invalid(violations) => {
                // EC2 reports ONE parameter at a time, so a request that also failed other
                // constraints is still refused with this parameter's own sentence.
                violations.iter().find_map(ec2_value_refusal)
            }
            // Authentication is the framework's answer to give, not this service's, and a
            // refusal carrying no violation has nothing for this service to name.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. A query service's default is the
        // protocol-correct 400 `InvalidAction`, which is an EC2 ERROR RESPONSE and
        // so closes the same envelope every other one does: this takes that
        // default rendering, code and message and all, and stamps the envelope on
        // it ([`stamp_error_envelope`]) rather than wording the failure again.
        let mut response = self.entry.routing_error_response(req);
        stamp_envelope(&mut response);
        Some(response)
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. That answer is an EC2 error response too, so it is
        // the default rendering with this service's envelope stamped on it, as the
        // routing failure above. The error is BORROWED here (the default renderer
        // consumes one), so it is restated as an owned error of the same kind
        // first; an error [`restated`] cannot restate answers `None`, which keeps
        // the framework's own rendering rather than reporting the failure as a
        // different kind.
        //
        // The default rendering is taken from the framework's own renderer for this
        // service's protocol envelope rather than from the entry: this seam is handed
        // the error alone, and the entry's renderer resolves the envelope from the
        // REQUEST it is given. EC2 declares one protocol — ec2Query — so the envelope
        // is the same one the entry would select for every request it serves.
        let mut response = smithy_server_runtime::protocol_error_response(
            smithy_server_runtime::PreDispatchProtocol::Query { ec2: true },
            restated(error)?,
        );
        stamp_envelope(&mut response);
        Some(response)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // EC2's ec2Query envelope carries three things the generated protocol does
        // not write: the success flag `<return>` on a mutation whose modelled
        // output shape does not declare it, the `<RequestID>` a refusal closes its
        // envelope with, and the `charset` parameter on the response media type.
        // Each belongs to the envelope rather than to any one operation, so they
        // are served here, once, for every operation. What this seam does NOT
        // reach is noted on `RESPONSE_CONTENT_TYPE`.
        Some(&self.protocol)
    }
}

// =============================================================================
// EC2's ec2Query response envelope
// =============================================================================

/// The XML namespace EC2's model stamps on an ec2Query envelope — the service
/// shape's `@xmlNamespace` URI:
///
/// ```text
/// smithy_query(query="service_info",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// The generated `ServiceDefinition` builds its own protocol with this same
/// value. [`Ec2QueryEnvelope`] cannot borrow that instance — it lives beside it
/// on [`EmulatedService`] — so it names the namespace from the model here.
const XML_NAMESPACE: &str = "http://ec2.amazonaws.com/doc/2016-11-15";

/// The media type EC2 declares on an ec2Query response.
///
/// The model declares the protocol but not the media type's `charset`
/// parameter, so this is recorded behaviour: an EC2 ec2Query response answers
/// `content-type: text/xml;charset=UTF-8`, where the generated protocol writes
/// the bare `text/xml` the Smithy protocol definition prescribes.
///
/// [`Ec2QueryEnvelope`] is the one place a serializer's media type is chosen, so
/// it reaches every response this SERVICE renders: each success and each modelled
/// error, including the scaffold's 501 — the same reach the envelope's
/// `<RequestID>` has. What a protocol does NOT render is the dispatch sequence's
/// framework-level refusal tail — a value outside a constraint the model declares,
/// an absent `@required` member — which the sequence renders itself from the
/// model-derived pre-dispatch declaration; those responses are finished with this
/// same envelope by [`FinishedEnvelope`], the decorator every modelled operation is
/// served through. The two the router renders in front of the service — a routing
/// failure and a transport failure — are taken back through
/// `override_routing_error` / `override_framework_error` and finished the same way
/// ([`stamp_envelope`]).
///
/// One framework response remains outside every seam, because the router renders it
/// without consulting the service for anything but its code and message: an
/// authentication refusal, which answers the model-derived envelope.
const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// The element an ec2Query ERROR envelope names its request id with.
///
/// The EC2 API Reference's error response is
/// `<Response><Errors><Error><Code/><Message/></Error></Errors><RequestID/></Response>` —
/// the id sits beside the `<Errors>` block and is spelled `RequestID`, where a SUCCESS
/// envelope writes the lowercase `requestId` the framework already emits. The framework's
/// ec2Query error envelope writes neither, so this service adds it here, once, for every
/// error it renders.
const ERROR_REQUEST_ID_ELEMENT: &str = "RequestID";

/// The closing element of the ec2Query error envelope, which the request id is written in
/// front of.
const ERROR_ENVELOPE_CLOSE: &str = "</Response>";

/// The source of each error envelope's request id: a process-wide counter, so no two
/// responses of this process can report the same id.
static NEXT_ERROR_REQUEST_ID: ::std::sync::atomic::AtomicU64 =
    ::std::sync::atomic::AtomicU64::new(1);

/// A fresh request id, in the 8-4-4-4-12 lowercase-hex form an AWS request id takes.
///
/// What a client does with a request id is CORRELATE one response with another, so
/// uniqueness is the whole invariant — the framework's own renderer puts it as "an
/// identifier, not a security token: a random v4 UUID is sufficient and need not be
/// unpredictable" for the success envelope's `<requestId>`
/// (`vendor/smithy-schema-server/src/query.rs`). So it is minted from a counter, the way
/// this service mints every resource id it hands out
/// (`state::ComAmazonawsEc2Amazonec2State::next_id`): a counter cannot repeat, needs no
/// state of the store's and no dependency of its own, and nothing in a request can
/// influence it.
fn new_error_request_id() -> String {
    let counted = NEXT_ERROR_REQUEST_ID.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    // 32 hex digits, which the 8-4-4-4-12 grouping below slices exactly.
    let hex = format!("{counted:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `body` with a freshly generated request id written into the error envelope, or `None`
/// when the body is not an envelope this can add one to.
///
/// One id per response ([`new_error_request_id`]): the id is what a client correlates a
/// failure on, so each response carries its own — an identifier, not a secret. It is
/// generated per RESPONSE rather than read from state, so nothing about it is stored and two
/// refusals are distinguishable.
///
/// A body that does not end with the envelope's closing element is left alone rather than
/// guessed at, so this can only ever add the element the envelope documents — and so is
/// one that already names the request it answers, the envelope carrying exactly one id
/// however many times a response passes through here.
fn with_error_request_id(body: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(body).ok()?;
    let head = text.strip_suffix(ERROR_ENVELOPE_CLOSE)?;
    if head.contains(&format!("<{ERROR_REQUEST_ID_ELEMENT}>")) {
        return None;
    }
    let request_id = new_error_request_id();
    Some(
        format!(
            "{head}<{ERROR_REQUEST_ID_ELEMENT}>{request_id}</{ERROR_REQUEST_ID_ELEMENT}>\
             {ERROR_ENVELOPE_CLOSE}"
        )
        .into_bytes(),
    )
}

/// Finish `response` with EC2's envelope: the media type it declares
/// ([`RESPONSE_CONTENT_TYPE`]) and, when the body is an error envelope that does not
/// name the request yet, the `<RequestID>` it closes with
/// ([`with_error_request_id`]).
///
/// The one place a response of this service is finished, so every response it
/// answers — a success, a modelled error from a handler, the scaffold's 501, a
/// refusal the request walk gave before a handler ran, a routing failure, a
/// transport failure — carries the same envelope. The body the renderer built is
/// in memory, so it can be read back and rewritten here; a body that is not an
/// envelope this can close is left exactly as it was, and so is one already closed,
/// which is what lets this be applied wherever a response is finished.
fn stamp_envelope(response: &mut RtResponse) {
    response
        .headers_mut()
        .insert("content-type", RESPONSE_CONTENT_TYPE);
    let rewritten = response
        .body()
        .bytes()
        .and_then(with_error_request_id)
        .map(::aws_smithy_types::body::SdkBody::from);
    if let Some(body) = rewritten {
        *response.body_mut() = body;
    }
}

/// `error` as an owned error of the same kind, or `None` for a kind that cannot be
/// restated.
///
/// The router hands `override_framework_error` a BORROWED error, because the
/// default renderer it falls back to consumes one and a [`ProtocolError`] is not
/// `Clone`. Rendering the default and then stamping the envelope on it needs an
/// owned error, so each kind is restated from the data it carries. The transport
/// failure the router detects up front is a `Malformed`; every other kind is
/// restated too, so the stamp does not depend on which one arrives. A codec error
/// carries an error that cannot be restated, and answers `None` — the framework's
/// own rendering, unstamped, rather than a failure reported as a different kind.
fn restated(error: &ProtocolError) -> Option<ProtocolError> {
    match error {
        ProtocolError::Malformed(detail) => Some(ProtocolError::Malformed(detail.clone())),
        ProtocolError::BodyRead(detail) => Some(ProtocolError::BodyRead(detail.clone())),
        ProtocolError::MissingHeader(name) => Some(ProtocolError::MissingHeader(name)),
        ProtocolError::InvalidHeader { name, message } => Some(ProtocolError::InvalidHeader {
            name,
            message: message.clone(),
        }),
        ProtocolError::UnknownOperation(action) => {
            Some(ProtocolError::UnknownOperation(action.clone()))
        }
        ProtocolError::Invalid(violations) => Some(ProtocolError::Invalid(violations.clone())),
        ProtocolError::Unauthenticated { code, message } => {
            Some(ProtocolError::Unauthenticated { code, message })
        }
        _ => None,
    }
}

/// The envelope's success flag, as the `<return>` element an ec2Query response
/// carries.
///
/// A member schema rather than a modelled one: the Smithy model declares no
/// `return` member on the output shapes this is written for, so the element has
/// no shape of its own to borrow. The XML codec names an element from the
/// member name, which is what makes this write `<return>true</return>`.
static RETURN_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#Ec2QueryEnvelope$return",
        "com.amazonaws.ec2",
        "return",
    ),
    ShapeType::Boolean,
    "return",
    0,
);

/// The operations whose modelled output declares members of its own and whose
/// AWS-documented response elements nevertheless include `return`.
///
/// Source: the EC2 API Reference's "Response Elements" for each operation.
/// `CreateSecurityGroup` documents `requestId`, `return` and `groupId`, while
/// `CreateSecurityGroupResult` in the Smithy model declares only `groupId`,
/// `tags` and `securityGroupArn` — so the flag has to come from the envelope.
/// An operation that reports a created resource instead of a flag (`CreateVpc`
/// answers `<vpc>` and no `<return>`) is deliberately absent.
const RETURN_BESIDE_MODELLED_OUTPUT: &[&str] = &["CreateSecurityGroup"];

/// An output serialized as itself, then followed by the envelope's `<return>`
/// success flag.
///
/// `true` is not a value read off state: reaching the serializer at all means
/// the handler returned `Ok`, which is exactly what the flag reports. A failure
/// renders through `serialize_error` and never through here.
struct WithReturnFlag<'a> {
    inner: &'a dyn SerializableStruct,
}

impl SerializableStruct for WithReturnFlag<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.inner.serialize_members(s)?;
        s.write_boolean(&RETURN_MEMBER, true)
    }
}

/// EC2's ec2Query protocol: the framework's [`QueryServerProtocol`] plus the
/// three parts of EC2's envelope the Smithy protocol definition does not
/// describe.
///
/// Request reading and routing are the framework's unchanged, and so is the shape of
/// the error envelope down to `</Errors>` — this only adds what EC2 writes on top.
#[derive(Debug)]
pub struct Ec2QueryEnvelope {
    inner: QueryServerProtocol,
}

impl Ec2QueryEnvelope {
    fn new() -> Self {
        Self {
            inner: QueryServerProtocol::ec2_query().with_namespace(XML_NAMESPACE),
        }
    }

    /// Whether this operation's success envelope carries the `<return>` flag.
    ///
    /// Three cases, in order:
    ///
    /// * the modelled output declares a `return` member of its own — the
    ///   handler owns the value (`DeleteSecurityGroup` sets it), so the envelope
    ///   must not write a second one;
    /// * the modelled output declares no members at all (`smithy.api#Unit`) —
    ///   the operation has nothing of its own to report, and every EC2 operation
    ///   modelled that way is a mutation, so the flag is the whole body. No
    ///   reporting operation is modelled with a Unit output: every `Describe*`
    ///   and `Get*` answers a result shape;
    /// * otherwise the flag is written only for the operations
    ///   [`RETURN_BESIDE_MODELLED_OUTPUT`] names.
    fn writes_return_flag(operation_name: &str, output_schema: &Schema) -> bool {
        let members = output_schema.members();
        if members
            .iter()
            .any(|member| member.member_name() == Some("return"))
        {
            return false;
        }
        members.is_empty() || RETURN_BESIDE_MODELLED_OUTPUT.contains(&operation_name)
    }
}

impl ServerProtocol for Ec2QueryEnvelope {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = if Self::writes_return_flag(operation_name, output_schema) {
            self.inner.serialize_output(
                &WithReturnFlag { inner: output },
                output_schema,
                operation_name,
                success_status,
            )?
        } else {
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?
        };
        response
            .headers_mut()
            .insert("content-type", RESPONSE_CONTENT_TYPE);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        // The media type and the envelope's own request id, neither of which the
        // framework's ec2Query error body carries; see [`stamp_envelope`].
        stamp_envelope(&mut response);
        Ok(response)
    }
}

/// The generated operation the model's routing key names for `req`, or `None` when the
/// key names no operation of this service.
///
/// The SAME lookup the router's own table does, through the model-derived protocol, so
/// the operation claimed here and the operation the router would have resolved are one.
///
/// Called twice per request on purpose — once by `override_operation`, to decide whether
/// this service claims the request at all, and once by [`FinishedEnvelope::run`], which
/// borrows the service for exactly as long as the future it returns and so cannot be
/// handed the first result. The lookup is the protocol's own routing-key resolution over a
/// body already in memory, which is what the router itself does a third time to read the
/// operation's auth schemes.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// One modelled operation, run as itself and then answered in EC2's response envelope.
///
/// [`Ec2QueryEnvelope`] reaches the responses the dispatch sequence's CODEC renders — a
/// success and a modelled error. It does not reach the sequence's refusal tail: a value
/// outside a `@range` or `@length` the model declares, or an absent `@required` member, is
/// refused before any handler runs and rendered by the sequence itself from the
/// model-derived pre-dispatch declaration — the bare media type the Smithy protocol
/// definition prescribes, and no request id. Those refusals are EC2 responses like any
/// other (one of them carries this service's own `InvalidMaxResults`), so this decorator
/// runs the operation and finishes WHATEVER it answered with the same envelope
/// ([`stamp_envelope`]), which is idempotent for a response already finished.
///
/// Stateless: the operation to run is resolved from the service the framework hands
/// `run`, so one value serves every operation and nothing is built per request.
#[derive(Debug)]
struct FinishedEnvelope;

impl ErasedOperation for FinishedEnvelope {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> smithy_server_runtime::BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let mut response = match modeled_operation(svc, req) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // `override_operation` claims a request only for a routing key the table
                // serves, so this is unreachable; a request that somehow arrives here
                // named no operation of this service and is answered the way one that
                // names none is, rather than with an invented envelope.
                None => svc.definition().routing_error_response(req),
            };
            stamp_envelope(&mut response);
            response
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` defers to the modelled operation the request's routing key names — the
        // operation this decorator runs — so serving a request through here cannot change
        // which auth schemes the model requires of it. The router resolves those schemes
        // from the modelled operation FIRST and consults the operation it is about to run
        // only when the model names none: `authenticate` in
        // `vendor/smithy-server-runtime/src/multi_service_router.rs`, which reads them
        // through its own `modeled_operation` lookup rather than from the value
        // `override_operation` answered.
        None
    }
}

/// EC2's own code and sentence for a value this service knows how EC2 refuses, or `None`
/// when the refusal has no recorded EC2 wording.
///
/// EC2 words a refused value per parameter TYPE rather than as a validation clause —
/// "`<value>` is not a valid value for RoleType. Specify either odcr-owner or
/// unused-reservation-billing-owner." for a value outside the `CallerRole` catalog. The
/// wording lives with the type it belongs to ([`crate::state::caller_role_refusal_message`]),
/// keyed on the value set the constraint walk reports, so every operation that carries that
/// member is refused in the same words and nothing here keys on an operation or a request's
/// shape. A type whose EC2 wording this service has not recorded answers `None` and keeps
/// the framework's own clause.
///
/// A value outside the burstable `UnlimitedSupportedInstanceFamily` catalog is the second
/// catalog this service has EC2's wording for: EC2 answers its own `InvalidInstanceFamily`
/// with the sentence it words for that parameter type
/// ([`crate::state::instance_family_refusal_message`]), keyed on that catalog the same way.
///
/// A `MaxResults` below the smallest page the operation publishes is the other refusal this
/// service states: EC2 answers its own code and sentence for it, naming the value and the
/// bound ([`crate::state::max_results_refusal`]). It is keyed on the member the constraint
/// walk reports, on the bound it failed and on the value it carried — a positive page size
/// the operation will not page with is `InvalidMaxResults`, a value that is no page size at
/// all is `InvalidParameter` — so every paged operation of this service answers one code and
/// one wording for each of the two, the minimum coming from the violation rather than from a
/// number written here.
fn ec2_value_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    use aws_smithy_schema_server::ViolationKind;

    match &violation.kind {
        ViolationKind::EnumValue(allowed) if crate::state::is_caller_role_catalog(allowed) => {
            // Already elided to the framework's `MAX_ECHOED_VALUE`, so echoing it cannot
            // amplify a request into an arbitrarily long response.
            Some(Refused {
                code: crate::state::INVALID_PARAMETER_VALUE.to_string(),
                message: crate::state::caller_role_refusal_message(
                    violation.value.as_deref().unwrap_or_default(),
                ),
            })
        }
        ViolationKind::EnumValue(allowed)
            if crate::state::is_unlimited_supported_instance_family_catalog(allowed) =>
        {
            // A family outside the burstable catalog is EC2's own `InvalidInstanceFamily`
            // rather than a validation clause, in the sentence EC2 words for that parameter
            // type ([`crate::state::instance_family_refusal_message`], which the handlers
            // that resolve a family answer with too). The value it names is already elided
            // to the framework's `MAX_ECHOED_VALUE`, so echoing it cannot amplify a request
            // into an arbitrarily long response.
            Some(Refused {
                code: crate::state::INVALID_INSTANCE_FAMILY.to_string(),
                message: crate::state::instance_family_refusal_message(
                    violation.value.as_deref().unwrap_or_default(),
                ),
            })
        }
        ViolationKind::MinValue(minimum)
            if violation.path == crate::state::MAX_RESULTS_MEMBER_PATH =>
        {
            let value = violation.value.as_deref().unwrap_or_default();
            // Which code and which sentence depend on what the value IS — a page size the
            // operation will not page with, or no page size at all — never on the operation
            // that carries it ([`crate::state::max_results_refusal`], which answers both so
            // they cannot disagree).
            let (code, message) =
                crate::state::max_results_refusal(&violation.path, value, minimum);
            Some(Refused {
                code: code.to_string(),
                message,
            })
        }
        _ => None,
    }
}

