// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3-amazons3` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. S3 answers every one of them with one of its OWN
//!   error codes (see `s3_refusal`), never the framework's generic
//!   `ValidationException`, because a client branches on the S3 code.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. S3's SELECT data plane is the one such request
//!   this service words itself: no routing-table entry claims `?select`, and S3
//!   refuses a Select request with `MethodNotAllowed` (405) rather than the
//!   framework's "not written yet" 501. Everything else keeps the default.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//! - `override_protocol` — the codec the modeled sequence reads and answers in.
//!   The default returns `None`, serving the model-derived protocol; THIS service
//!   returns [`wire::UnwrappedXmlRestXml`], the model's own restXml with the
//!   envelope-free output document the model marks `s3UnwrappedXmlOutput`.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

pub mod wire;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::protocol::ServerProtocol;

use wire::UnwrappedXmlRestXml;

/// The first URI path segments another emulated service that SHARES S3's `s3` SigV4
/// signing name owns, and which S3 must therefore not claim.
///
/// S3's own modeled path space cannot discriminate: every bucket/object operation binds
/// `/{Bucket}` or `/{Bucket}/{Key+}`, a path LABEL that matches ANY first segment, which
/// is why the generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY ("no constraint") for this
/// service. S3 Control
/// (`services/service-com-amazonaws-s3control-awss3controlservicev20180820`) signs as
/// `s3` too — `smithy_query(query="service_info")` on its model reports sigv4Name `s3`
/// with endpointPrefix `s3-control` — and binds EVERY one of its operations under the
/// single literal first segment below (`/v20180820/bucket/{Bucket}`, `/v20180820/jobs`,
/// `/v20180820/accesspoint`, …). Excluding that one segment is what leaves exactly one
/// claimant for every request signed for `s3`, instead of two and a
/// `500 AmbiguousService` for both.
///
/// AWS itself separates the two by endpoint host, which an emulator cannot use: a client
/// configured with one local endpoint URL sends the same `Host` for both services. The
/// one request this costs is a bucket literally NAMED `v20180820`, which is the narrowest
/// possible loss and strictly better than refusing every `s3` request.
const PEER_PATH_SEGMENTS: &[&str] = &["v20180820"];

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses the path space of a peer service listed in
/// `peer_segments`. Matching is EXACT segment equality, deliberately never a prefix test:
/// a bucket named `v20180820-logs` is S3's, not the peer's.
fn addresses_peer_path(parts: &http::request::Parts, peer_segments: &[&str]) -> bool {
    match first_path_segment(parts.uri.path()) {
        Some(segment) => peer_segments.contains(&segment),
        // The ROOT path (`GET /?x-id=ListBuckets`) is S3's own; no peer claims it.
        None => false,
    }
}

/// The INPUT members this service binds as the request's XML DOCUMENT: every `@httpPayload`
/// member of an operation input whose target is a structured shape (a structure, union,
/// list or map), named the way a validation path names it — lowerCamel.
///
/// This is the one thing a refusal needs to know that a [`Violation`] does not carry: a
/// value that failed a constraint INSIDE the request document is a document that does not
/// validate against S3's published schema, and a value that failed in a header, a query
/// parameter or a URI label is a bad argument. The two are different wire codes (see
/// [`s3_refusal`]), and the only thing that tells them apart is whether the violation's
/// path starts at one of these members.
///
/// Source: the `@httpPayload` members of this service's operation inputs, re-derived from
/// the model by `tests/all/refusal_codes.rs` so a model update that binds another document
/// cannot quietly start reporting it as a bad argument.
pub const XML_DOCUMENT_PAYLOAD_MEMBERS: &[&str] = &[
    "abacStatus",
    "accelerateConfiguration",
    "accessControlPolicy",
    "analyticsConfiguration",
    "annotationTableConfiguration",
    "bucketLoggingStatus",
    "cORSConfiguration",
    "createBucketConfiguration",
    "delete",
    "intelligentTieringConfiguration",
    "inventoryConfiguration",
    "inventoryTableConfiguration",
    "journalTableConfiguration",
    "legalHold",
    "lifecycleConfiguration",
    "metadataConfiguration",
    "metadataTableConfiguration",
    "metricsConfiguration",
    "multipartUpload",
    "notificationConfiguration",
    "objectEncryption",
    "objectLockConfiguration",
    "ownershipControls",
    "publicAccessBlockConfiguration",
    "replicationConfiguration",
    "requestPaymentConfiguration",
    "restoreRequest",
    "retention",
    "serverSideEncryptionConfiguration",
    "tagging",
    "versioningConfiguration",
    "websiteConfiguration",
];

/// Whether `violation` names a value that arrived inside the request's XML document.
///
/// Two things decide it. The path is the trail of member names from the operation input
/// inwards, so its FIRST segment is the input member the value came in on and everything
/// below it travelled INSIDE that member — which is why a value refused in the document
/// always names a member of the container, and so always has a path that descends past
/// it. A path that is the member ALONE is either the DOCUMENT itself, which is the one
/// refusal this service mints for a body (the XML charset rule in
/// [`wire::xml_charset_violation`]), or a SCALAR binding of a member whose name another
/// operation uses for its document — `Tagging` is the one such name in this service, the
/// `@httpPayload` of the two tagging writes and the `x-amz-tagging` HEADER of the three
/// object writes — and a header is not a document.
fn violation_is_in_request_document(violation: &Violation) -> bool {
    let mut segments = violation.path.split('.');
    let Some(root) = segments.next() else {
        return false;
    };
    if !XML_DOCUMENT_PAYLOAD_MEMBERS.contains(&root) {
        return false;
    }
    segments.next().is_some()
        || violation.kind == ViolationKind::Pattern(wire::XML_CHARACTER_SET.to_string())
}

/// The S3 error code and message for one boundary refusal.
///
/// S3 is a restXml service: every refusal it answers carries an S3 error code in an
/// `<Error><Code>` body, and which code depends on WHERE the offending value came from.
/// A value that fails a constraint inside the request's XML document is a document that
/// "did not validate against our published schema", which is `MalformedXML`; a value that
/// fails one in a header, a query parameter or a URI label is `InvalidArgument`, "invalid
/// argument". Both are HTTP 400, the status the protocol already renders a refusal with.
///
/// A field S3 PUBLISHES its own refusal for is answered with that refusal (see
/// [`published_refusal`]) ahead of either, because the published wording is the contract
/// for that field. Otherwise `MalformedXML` carries the fixed text its own error-responses
/// entry publishes, because that is what S3 sends however the document failed to validate,
/// and `InvalidArgument` keeps the clause the validator built from the request — which
/// member, and the constraint it broke — since that is the only thing left that says which
/// argument was rejected.
fn s3_refusal(violation: &Violation) -> Refused {
    if let Some(published) = published_refusal(violation) {
        return published;
    }
    if violation_is_in_request_document(violation) {
        return Refused {
            code: crate::state::MALFORMED_XML_CODE.to_string(),
            message: crate::state::MALFORMED_XML_MESSAGE.to_string(),
        };
    }
    Refused {
        code: crate::state::INVALID_ARGUMENT_CODE.to_string(),
        message: violation.message(),
    }
}

/// The refusal S3 publishes for the request field `violation` names, or `None` for a field
/// it publishes none for.
///
/// S3 does not report a refused value by naming the Smithy member path it travelled on: for
/// each of these fields it publishes ONE sentence of its own, which a runbook and a
/// message-matching client key on. The path's FIRST segment is the input member the value
/// arrived on, so that is what the sentence is chosen by; everything the sentence does
/// interpolate comes from the request.
///
/// Each row's source is the AWS S3 API reference for the operation that binds the field —
/// `CopyObject`'s two directive headers, `GetObjectAttributes`'
/// `x-amz-object-attributes`, `CreateBucket`'s `x-amz-object-ownership`, `ListBuckets`'
/// `max-buckets`, and `UpdateObjectEncryption`'s `ObjectEncryption` payload, whose
/// `InvalidRequest` messages that operation's own `@documentation` lists
/// (`smithy_query(query="doc",shape="UpdateObjectEncryption")`).
fn published_refusal(violation: &Violation) -> Option<Refused> {
    let mut segments = violation.path.split('.');
    let member = segments.next()?;
    let rejected = violation.value.as_deref().unwrap_or_default();
    let (code, message) = match member {
        // The two `CopyObject` directives, each an enum over [COPY, REPLACE].
        "taggingDirective" => (
            crate::state::INVALID_ARGUMENT_CODE,
            "Unknown tagging directive.".to_string(),
        ),
        "metadataDirective" => (
            crate::state::INVALID_ARGUMENT_CODE,
            "Unknown metadata directive.".to_string(),
        ),
        // `GetObjectAttributes` names the attribute that is not one, whichever element of
        // the header's list carried it.
        "objectAttributes" => (
            crate::state::INVALID_ARGUMENT_CODE,
            "Invalid attribute name specified.".to_string(),
        ),
        // `CreateBucket` echoes the header value it did not recognise.
        "objectOwnership" => (
            crate::state::INVALID_ARGUMENT_CODE,
            format!("Invalid x-amz-object-ownership header: {rejected}"),
        ),
        // `ListBuckets` states the bound rather than the value: the range is
        // `MaxBuckets`' own (`min 1`, `max 10000`).
        "maxBuckets" => (
            crate::state::INVALID_ARGUMENT_CODE,
            "Argument max-buckets must be an integer between 1 and 10000".to_string(),
        ),
        // `UpdateObjectEncryption`'s payload: a KMS key ARN that is not one is an
        // `InvalidRequest` naming the requirement and then WHY the value does not meet it,
        // the way S3 quotes an ARN parser's own diagnostic
        // ([`crate::state::malformed_arn_reason`]); anything else about the container is
        // the `InvalidRequest` that operation publishes for a payload that names no usable
        // encryption type.
        "objectEncryption" => {
            let message = if segments.any(|segment| segment == "kMSKeyArn") {
                format!(
                    "Requests modifying object encryption configuration to SSE-KMS require a \
                     valid target kms key arn: {}",
                    crate::state::malformed_arn_reason(rejected)
                )
            } else {
                "Requests that modify an object encryption configuration require a valid \
                 new encryption type. Valid values are SSEKMS. Modify the request to \
                 specify SSE-KMS for the updated encryption type, and then try again."
                    .to_string()
            };
            (crate::state::INVALID_REQUEST_CODE, message)
        }
        _ => return None,
    };
    Some(Refused {
        code: code.to_string(),
        message,
    })
}

/// The query parameter that addresses S3's Select data plane.
///
/// `SelectObjectContent` is bound at `POST /{Bucket}/{Key+}?select&select-type=2`
/// (`smithy_query(query="operation",operation="SelectObjectContent",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports that `@http` uri; `OPERATIONS.md` restates it), so the `select` SUBRESOURCE is
/// what names the feature — `select-type` only says which generation of it the client
/// speaks, and the refusal below is the same for every value of it.
const SELECT_SUBRESOURCE: &str = "select";

/// Whether `query` — a request URI's query string — carries the parameter `name`, with or
/// without a value. A subresource arrives as a bare flag (`?select`), so a reader that
/// insisted on `=` would miss it.
fn query_carries(query: &str, name: &str) -> bool {
    query
        .split('&')
        .any(|parameter| parameter.split('=').next().unwrap_or(parameter) == name)
}

/// Whether `path` addresses an OBJECT — S3's `/{Bucket}/{Key+}`, a non-empty bucket
/// segment with a non-empty key below it — rather than a bucket or the service root.
fn addresses_object(path: &str) -> bool {
    match path.trim_start_matches('/').split_once('/') {
        Some((bucket, key)) => !bucket.is_empty() && !key.is_empty(),
        None => false,
    }
}

/// Whether `uri` addresses the S3 Select data plane: the `select` subresource of an
/// object.
///
/// The method is deliberately not part of this predicate, and costs nothing: it is
/// consulted ONLY for a request no modeled operation claimed, and every other method on an
/// object path (`GET`/`HEAD`/`PUT`/`DELETE`) resolves to its own modeled operation before
/// ever reaching here. What is left is a request that asked S3 for Select and nothing else.
fn addresses_s3_select(uri: &str) -> bool {
    query_carries(wire::request_query(uri), SELECT_SUBRESOURCE)
        && addresses_object(wire::request_path(uri))
}

/// The `com-amazonaws-s3-amazons3` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it. See [`wire`] for what it
    /// changes about the generated one: the output document of the operations the model
    /// marks `s3UnwrappedXmlOutput` carries no envelope.
    protocol: UnwrappedXmlRestXml,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: UnwrappedXmlRestXml::new(),
        }
    }

    /// The refusal S3 answers a request against a resource it does not serve at all:
    /// HTTP 405 with the error code `MethodNotAllowed` and that code's published message
    /// (see [`crate::state::METHOD_NOT_ALLOWED_CODE`]).
    ///
    /// Rendered through THIS service's own protocol, in the same
    /// [`crate::scaffolded::Unmodeled`] channel an implemented handler answers this code
    /// with (`GetObjectTorrent` is the one beside it), so the envelope a client parses —
    /// the bare `<Error>` root with its `<Code>`, `<Message>` and request-correlation
    /// elements, and the matching `x-amz-request-id` / `x-amz-id-2` headers — is byte for
    /// byte the one every other S3 refusal carries. No `Allow` header accompanies it: the
    /// feature is withdrawn rather than bound to some other method, so there is no method
    /// against this resource to name.
    ///
    /// `None` on the serializer failure a fixed code and a fixed message cannot actually
    /// provoke, which leaves the framework's own routing refusal to answer rather than
    /// panicking in the request path.
    fn method_not_allowed(&self) -> Option<aws_smithy_runtime_api::http::Response> {
        let refusal = crate::scaffolded::Unmodeled {
            code: crate::state::METHOD_NOT_ALLOWED_CODE,
            status: crate::state::METHOD_NOT_ALLOWED_STATUS,
            message: crate::state::METHOD_NOT_ALLOWED_MESSAGE.to_string(),
        };
        self.protocol
            .serialize_error(
                &refusal,
                &crate::scaffolded::UNMODELED_SCHEMA,
                refusal.code,
                refusal.status,
            )
            .ok()
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
        // This service signs as `s3` — a signing name it SHARES with S3 Control, whose
        // own endpoint is `s3-control`. The router's routing key comes from the SigV4
        // credential scope first, so both services resolve to `s3`, and matching on the
        // signing name ALONE makes both claim every request — which the router answers
        // `500 AmbiguousService` for, to BOTH services' traffic.
        //
        // S3 cannot narrow itself by its own path space (every operation is bound under
        // the `{Bucket}` label, so its segment list is empty), so it claims every path
        // under the shared signing name EXCEPT the peer's own (see `PEER_PATH_SEGMENTS`).
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                !addresses_peer_path(parts, PEER_PATH_SEGMENTS)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service word it. S3's refusals carry S3's own error codes, never the
        // framework's generic `ValidationException`: see [`s3_refusal`] for the
        // rule — a value refused inside the request's XML document is
        // `MalformedXML`, one refused in a header, a query parameter or a URI
        // label is `InvalidArgument`. The FIRST violation is what the answer
        // names, the way the protocol's own envelope reports one code.
        match refusal {
            Refusal::Invalid(violations) => violations.first().map(s3_refusal),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // S3's SELECT data plane is exactly that case: `SelectObjectContent` is the one
        // operation of this model whose output is a `@streaming` event-stream union, which
        // the scaffold generator declines to type (`src/ops/SKIPPED_STUBS.md`), so no
        // routing-table entry claims `?select` and the default would answer the framework's
        // 501 `NotImplemented` — "nobody has written this yet". That is not one of the two
        // things S3 answers a Select request with: it either serves the rows, or — having
        // withdrawn S3 Select from every account that had not already enabled it — refuses
        // the request outright, with the 405 `MethodNotAllowed` it answers any resource that
        // does not serve the method asked of it (`GetObjectTorrent`, which no account can
        // enable, is refused the same way). The withdrawal is account-wide rather than
        // per-object, so the refusal is the same for every Select request, whatever
        // expression, serialization, scan range or encryption it carries and whichever
        // bucket or key it names — and a client that probes Select and falls back on this
        // code sees the code it branches on instead of an unmodeled exception.
        //
        // Everything else keeps the framework's default, so an operation that really is
        // merely unwritten still answers the 501 that counts it as such.
        if !addresses_s3_select(req.uri()) {
            return None;
        }
        self.method_not_allowed()
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restXml protocol in every respect but one: an operation the
        // model marks `aws.customizations#s3UnwrappedXmlOutput` answers the document AWS
        // sends for it, its single output member promoted to the response root, which the
        // framework's generic envelope cannot express. See [`wire`].
        Some(&self.protocol)
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
