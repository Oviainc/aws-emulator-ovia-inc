// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockruntime-amazonbedrockfrontendservice` service: how it claims requests and how it words the
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
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header this
//! protocol reports that id in (`x-amzn-RequestId`) — on a success exactly as on a
//! failure, and on a streamed answer exactly as on a unary one. It is a property of the
//! RESPONSE and not of any modelled shape: no output and no error shape of this model
//! declares it, so no handler can set it, and the framework's `restJson1` renderers do
//! not write it. A client reads it as its SDK's response metadata, which is how a call is
//! correlated with the invocation-logging record written for it.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for every
//! emulated service, and this file only chooses the report this protocol takes (the header
//! alone — `restJson1` has no envelope element for the id) and wires it at the layers every
//! response of this service passes through:
//!
//! * [`RequestIdHeaderOperation`] for every operation the generated routing table serves,
//!   and [`RequestIdReportingOperation::header`] around each of the two event-stream
//!   operations this file routes itself — between them, everything an operation answers: a
//!   success, a modelled error, and a constraint refusal raised while it was being served;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, covers a response NO operation produced: a routing
//!   failure, a transport failure detected up front, and a pre-dispatch authentication
//!   refusal.
//!
//! Nothing else about a response changes: the status, the other headers and every byte of
//! the body stay as the renderer wrote them.

use crate::ops::converse_stream::ConverseStream;
use crate::ops::invoke_model_with_response_stream::InvokeModelWithResponseStream;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{Violation, ViolationKind};
use service_common::event_stream::EventStreamRoute;
// `request_path`: the one reader of a request URI's path, shared rather than inlined
// again here.
use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdHeaderOperation, RequestIdReportingOperation,
};
use service_common::request_path;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};

/// Each shape a constraint-violation path of this service can name a member of, with the
/// path prefix the validating walk reports that shape's members under.
///
/// This is the LIST of shapes to look in, not a list of members: which of their members are
/// floating point comes from the model
/// ([`service_common::validation_message::floating_point_member_paths`]). The operation
/// inputs this model declares are all listed, with no prefix; `InferenceConfiguration` is
/// listed under `inferenceConfig`, the member both Converse operations bind it to, because
/// codegen emits a member schema WITHOUT its target's own members (`Schema::new_member`
/// takes a shape type and a name) — so a nested shape has to be named here to be looked in
/// at all. An operation this model grows, or a second nested structure that carries a
/// numeric member, therefore needs a line here.
const VALIDATED_SHAPES: &[(&str, &Schema)] = &[
    ("", crate::ops::types::APPLY_GUARDRAIL_INPUT_SCHEMA),
    ("", crate::ops::types::CONVERSE_INPUT_SCHEMA),
    ("", crate::ops::types::CONVERSE_STREAM_INPUT_SCHEMA),
    ("", crate::ops::types::COUNT_TOKENS_INPUT_SCHEMA),
    ("", crate::ops::types::GET_ASYNC_INVOKE_INPUT_SCHEMA),
    ("", crate::ops::types::INVOKE_GUARDRAIL_CHECKS_INPUT_SCHEMA),
    ("", crate::ops::types::INVOKE_MODEL_INPUT_SCHEMA),
    (
        "",
        crate::ops::types::INVOKE_MODEL_WITH_RESPONSE_STREAM_INPUT_SCHEMA,
    ),
    ("", crate::ops::types::LIST_ASYNC_INVOKES_INPUT_SCHEMA),
    ("", crate::ops::types::START_ASYNC_INVOKE_INPUT_SCHEMA),
    (
        "inferenceConfig",
        crate::ops::types::INFERENCE_CONFIGURATION_SCHEMA,
    ),
];

/// The validation paths of the members this service's MODEL types as `float` or `double`.
///
/// Built once off the schemas themselves, through the one shared reader of that set
/// ([`service_common::validation_message::floating_point_member_paths`]), so nothing here
/// names a member: a path is in this set exactly when the model says that member is
/// floating point.
fn floating_point_member_paths() -> &'static BTreeSet<String> {
    static PATHS: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
        service_common::validation_message::floating_point_member_paths(VALIDATED_SHAPES)
    });
    &PATHS
}

/// `refusal` with every clause about a FLOATING-POINT member quoting the submitted value
/// the way this service writes one, and everything else untouched.
///
/// A refusal states the value it was given, and a service writes a floating-point value
/// with a fractional part or an exponent — `-1.0E-6`, not the decimal expansion
/// `-0.000001` — whatever its magnitude. The rendering is the one shared implementation in
/// `service_common`, so this service cannot drift from the others; which members it
/// applies to comes from the model ([`floating_point_member_paths`]).
fn in_float_value_rendering(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(
        violations
            .iter()
            .map(|violation| {
                service_common::validation_message::in_float_value_rendering(violation, |path| {
                    floating_point_member_paths().contains(path)
                })
            })
            .collect(),
    )
}

/// Whether `path` is `/model/{modelId}/{tail}` — the URI space every inference
/// operation of this service binds — and, if so, its `{tail}`.
///
/// `{modelId}` is a single `@httpLabel` segment, so the tail is the third segment
/// and there is nothing after it.
fn inference_route(path: &str) -> Option<&str> {
    let mut segments = path.trim_start_matches('/').split('/');
    let ("model", Some(model_id), Some(tail), None) = (
        segments.next()?,
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    (!model_id.is_empty() && !tail.is_empty()).then_some(tail)
}

/// `refusal` with every ENUM requirement's value set quoted in the order this
/// service quotes that member's values in (`crate::state::service_value_order`),
/// and everything else untouched.
///
/// A refusal names the values a member admits, and the ORDER it names them in is
/// the service's, not the model's: the model's declaration order is an artefact of
/// how the model was written. The lookup is by the MEMBER being refused, so a
/// member this service states no order for is left exactly as it arrived.
fn in_service_value_order(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(
        violations
            .iter()
            .map(|violation| match &violation.kind {
                ViolationKind::EnumValue(values) => Violation {
                    kind: ViolationKind::EnumValue(
                        crate::state::service_value_order(&violation.path, values)
                            .unwrap_or_else(|| values.clone()),
                    ),
                    path: violation.path.clone(),
                    value: violation.value.clone(),
                    sensitive: violation.sensitive,
                },
                _ => violation.clone(),
            })
            .collect(),
    )
}

/// The service's own wording for a boundary refusal that lands on a member
/// naming a GUARDRAIL the operation is to apply, or `None` when no violation
/// names one.
///
/// The references are walked in the order the service reads them
/// (`crate::state::RESOURCE_REFERENCES`), not in the order the violations
/// arrived, so a request wrong in more than one reference is refused on the
/// first one READ — which is what decides the wording. Which members are
/// references, and why `modelId` and `invocationArn` are not among them, is
/// stated on that table.
///
/// The code travels with the message: `ValidationException` is the code AWS
/// answers a readable-but-rejected request with on this restJson1 service, and
/// the code the framework's own default picks, so the choice changes only the
/// words.
fn resource_reference_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let message = crate::state::RESOURCE_REFERENCES
        .iter()
        .find(|(member, _)| violations.iter().any(|violation| violation.path == *member))
        .map(|(_, message)| *message)?;
    Some(Refused {
        code: "ValidationException".to_owned(),
        message: message.to_owned(),
    })
}

/// The `com-amazonaws-bedrockruntime-amazonbedrockfrontendservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
///
/// Besides the generated entry it holds this service's two event-stream
/// operations. They are NOT in the entry's routing table in a servable form: the
/// generated `ErasedOperation` shim of an operation whose output is a `@streaming`
/// union declines the request (it cannot drive `serialize_as_event_stream<P>`
/// through an object-safe `run`), so each one is held here behind an
/// [`EventStreamRoute`] and routed by [`EmulatedService::override_operation`] —
/// which the router consults FIRST, before the generated table.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    converse_stream: RequestIdReportingOperation<EventStreamRoute<ConverseStream<Eng>>>,
    invoke_model_with_response_stream:
        RequestIdReportingOperation<EventStreamRoute<InvokeModelWithResponseStream<Eng>>>,
    /// Serves every operation the generated routing table names, so each response an
    /// operation produced names the request it answers. Held, not built per request:
    /// `override_operation` hands back a borrow of it.
    request_id: RequestIdHeaderOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(crate::ServiceDefinition::new(
                Arc::clone(&engine),
            )),
            converse_stream: RequestIdReportingOperation::header(EventStreamRoute(
                ConverseStream::new(Arc::clone(&engine)),
            )),
            invoke_model_with_response_stream: RequestIdReportingOperation::header(
                EventStreamRoute(InvokeModelWithResponseStream::new(engine)),
            ),
            request_id: RequestIdHeaderOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what is
        // handed back is the shared `RequestIdEnvelopeDefinition`, which reads identity,
        // protocol and the routing table straight off that entry and adds the request-id
        // header to the three error renderers (see the module docs).
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `bedrock` — the signing name the WHOLE Bedrock family
        // shares (the control plane, this data plane, `bedrock-agent`,
        // `bedrock-agent-runtime`, `bedrock-data-automation` and
        // `bedrock-data-automation-runtime`), while AWS separates them by endpoint host
        // (`bedrock-runtime` here). The router keys on the SigV4 credential scope first,
        // so all six resolve to `bedrock`: claiming on that name alone makes every one of
        // them claim every Bedrock request and the router answers `500 AmbiguousService`
        // — an error code no AWS model declares — instead of serving any of them.
        //
        // The claim is therefore this service's own modelled path space, through the one
        // rule `service_common::bedrock` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split):
        // `async-invoke`, `guardrail`, `guardrail-checks` and `model` — the segments this
        // `restJson1` model binds, none of which another member publishes, matched by
        // EXACT segment equality, never by prefix, which is what keeps `model` out of the
        // control plane's `model-copy-jobs`, `model-import-jobs` and the rest. `None` for
        // the RPC argument: every operation here is bound to a URI.
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
        // The two event-stream operations the generated routing table cannot
        // serve. Routed by the method and the URI the model gives each one, so
        // every other request — including `InvokeModelWithBidirectionalStream`,
        // which this slice does not implement — still resolves through the
        // generated table and keeps answering the framework's "modeled but not
        // served" response rather than being half-served here.
        //
        // Each of the two is served through the shared request-id report, and every other
        // request the generated table answers goes through `RequestIdHeaderOperation` —
        // the one place every operation's response passes, so no operation can answer
        // without naming the request it answered. A request the table does not answer
        // declines here exactly as it would there, leaving the router's routing-error path
        // (which names the request itself, through the definition wrapper) unchanged.
        if req.method() == http::Method::POST.as_str() {
            match inference_route(request_path(req.uri())) {
                Some("converse-stream") => return Some(&self.converse_stream),
                Some("invoke-with-response-stream") => {
                    return Some(&self.invoke_model_with_response_stream)
                }
                _ => {}
            }
        }
        self.entry
            .routes(req)
            .then_some(&self.request_id as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // FIRST: a refusal that lands on a member naming the GUARDRAIL the
        // operation is to apply. Bedrock refuses a guardrail reference it cannot
        // read in its own words for that reference rather than with the per-member
        // constraint envelope below, which is what a BODY member outside its
        // constraints gets. `crate::state::RESOURCE_REFERENCES` is the one table of
        // those references, in the order the service reads them.
        if let Some(stated) = resource_reference_refusal(refusal) {
            return Some(stated);
        }
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) before any handler
        // runs. This service is `restJson1`, where AWS answers such a refusal as
        // `400 ValidationException` with the `N validation error(s) detected: …`
        // envelope the framework already renders — except for ONE difference, which
        // is why this delegates rather than returning `None`: AWS quotes a
        // `@pattern` requirement WITHOUT the anchors a whole-value pattern wraps it
        // in, so `outputDataConfig.s3OutputDataConfig.s3Uri` reads
        // `… pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?` rather than
        // leaking the `^`/`$` into the client-facing text. The CHECK still uses the
        // model's pattern as written; only the rendering drops them, and it is the
        // one shared implementation in `service_common`, so this service cannot
        // drift from the others.
        //
        // The value set of an ENUM requirement is listed in the order this service
        // lists those values in, which is not the order the model happens to declare
        // them in (`in_service_value_order`), and a FLOATING-POINT member's submitted
        // value is quoted back the way this service writes one
        // (`in_float_value_rendering`).
        service_common::validation_message::aws_validation_refusal(&in_float_value_rendering(
            &in_service_value_order(refusal),
        ))
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
