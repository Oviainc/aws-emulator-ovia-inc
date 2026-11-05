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
//!
//! ## The one refusal this service answers in its OWN envelope
//!
//! An AUTHENTICATION refusal is the exception, and it is why this file implements
//! [`smithy_server_runtime::ErasedServiceDefinition`] over the generated entry rather than
//! handing the entry back bare: every model-derived datum is delegated to the entry
//! unchanged, and the one method that is not is `refusal_response`, the renderer the
//! framework calls for a caller authentication refused. The `override_refusal` seam cannot
//! word that one, because the answer it returns is a code and a sentence — and S3's
//! authentication refusals differ from the framework's rendering in two further ways that a
//! client reads: the STATUS (a presigned URL whose query string cannot be used is a `400`,
//! not a `403`) and the ELEMENTS (a signature mismatch carries the SigV4 signing
//! diagnostics beside its code and message). [`sigv4`] holds that rule; a refusal it
//! declines to word still renders through the entry's own protocol-correct default.

// What AUTHORIZES one request: the bucket's own policy, evaluated with the condition keys
// the request carries, and the account the request says owns the bucket — see
// `service/authorization.rs`.
pub mod authorization;
// What an update of one of an S3 Metadata configuration's tables must satisfy about the
// configuration's journal table — see `service/metadata_tables.rs`.
pub mod metadata_tables;
// What an object WRITE must satisfy about the key it names and the class it asks for, before
// anything is stored — see `service/object_write.rs`.
pub mod object_write;
pub mod sigv4;
pub mod wire;

use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, OperationRoutingTable,
    Refusal, Refused, ServerHttpProtocol, Service, ServiceIdentity,
};
use std::borrow::Cow;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::protocol::ServerProtocol;
use aws_smithy_schema_server::ProtocolError;

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

/// The SigV4 signing name the S3 Express One Zone surface of this model is signed for.
///
/// A client that has taken a session on a directory bucket signs its Zonal endpoint calls
/// for `s3express` (the endpoint ruleset's signing name for a directory bucket), not for
/// `s3` — the emulator's own signature verification already treats the two as one family —
/// so a request carrying that scope is this service's, and is claimed as such.
const S3_EXPRESS_SIGNING_NAME: &str = "s3express";

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

/// The shortest and longest a general purpose bucket's name may be: "Bucket names must be
/// between 3 (min) and 63 (max) characters long" (Amazon S3 User Guide, *Bucket naming
/// rules*).
const BUCKET_NAME_CHARS: ::std::ops::RangeInclusive<usize> = 3..=63;

/// Whether `label` is SHAPED like a bucket name: "Bucket names can consist only of
/// lowercase letters, numbers, dots (.), and hyphens (-)", between 3 and 63 characters, and
/// must "begin and end with a letter or number" (same rules).
///
/// A shape test, not a claim that any such bucket exists: it is what tells a host whose
/// first label could be a bucket from one whose first label is an endpoint's own
/// (`s3.us-east-1.amazonaws.com`, whose `s3` is two characters).
///
/// `pub(crate)`: the copy-source reader asks the same question of the bucket half of an
/// `x-amz-copy-source` ([`crate::state::parse_copy_source`]), so one rule decides which
/// text can be a bucket name wherever this service reads one out of a request.
pub(crate) fn bucket_name_shaped(label: &str) -> bool {
    let allowed = |character: char| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '.' | '-')
    };
    let alphanumeric = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    BUCKET_NAME_CHARS.contains(&label.len())
        && label.chars().all(allowed)
        && label.chars().next().is_some_and(alphanumeric)
        && label.chars().next_back().is_some_and(alphanumeric)
}

/// Whether the store in `engine` holds a bucket named `name`.
///
/// One reader for the two places that ask — this service's own seams and the wire layer's
/// address normalization — so they cannot disagree about which names are buckets.
fn holds_bucket_in<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| state.lock().bucket(name).is_some())
}

/// Whether `host` carries a BUCKET as its first label, by shape alone: a host of at least
/// two labels whose first could be a bucket name.
///
/// Shape is not enough to decide the address — an endpoint's own first label can be
/// bucket-shaped (`s3-us-east-1.amazonaws.com`, `my-emulator.example.com`) — so
/// [`addressed_in_host`] is what resolves it, against the buckets this emulator holds.
fn host_style_label(host: &str) -> Option<&str> {
    let host = host.rsplit('@').next().unwrap_or(host);
    let (label, rest) = host.split_once('.')?;
    (!rest.is_empty() && bucket_name_shaped(label)).then_some(label)
}

/// The bucket a VIRTUAL-HOSTED-STYLE request addresses in `host`, when it is one —
/// `holds_bucket` answering whether a name is a bucket this emulator holds.
///
/// "Both the virtual-hosted-style requests and the path-style requests are supported"
/// (`GetObject`'s own documentation): a path-style request carries the bucket as the first
/// segment of its path, and a virtual-hosted-style request carries it as the first label of
/// its host — `https://bucket-name.s3.region-code.amazonaws.com/key-name` — leaving the path
/// to be the key alone. Every AWS SDK's URL presigner writes the second form for an endpoint
/// addressed by name, so a client that reads an object through a presigned URL addresses it
/// that way.
///
/// WHICH form a request is in cannot be read off the host's shape, because an endpoint's own
/// first label may be bucket-shaped and an emulator's endpoint is reached under a name
/// nothing here knows. So it is resolved against the buckets this emulator HOLDS: the first
/// host label addresses a bucket when it names one of them and the request's first path
/// segment does not. A path-style request's first path segment names its bucket, so it is
/// never read out of the host; and where both could (a bucket whose name is also the
/// endpoint's first label), the PATH wins, which leaves every request that resolves today
/// resolving the same way.
///
/// The LIMIT of resolving it that way: a host-style request naming a bucket this emulator
/// does NOT hold is read as path-style, so `PUT /` with `Host: new-bucket.<endpoint>` (a
/// host-style `CreateBucket`) and a read of an absent bucket resolve from the bare path
/// instead of from the host. Both answer about the address the path names — a create of no
/// bucket at all, a listing of the account's buckets — rather than about the host's. Serving
/// them would need the endpoint's own name, which a client configures and never sends, so
/// the choice here is to leave the requests that resolve today resolving unchanged;
/// `tests/all/presigned_object_reads.rs` pins the boundary.
pub(crate) fn addressed_in_host<'h>(
    host: &'h str,
    path: &str,
    holds_bucket: &dyn Fn(&str) -> bool,
) -> Option<&'h str> {
    let label = host_style_label(host)?;
    if !holds_bucket(label) {
        return None;
    }
    if first_path_segment(path).is_some_and(holds_bucket) {
        return None;
    }
    Some(label)
}

/// `path` with `bucket` put back in front of it, which is the PATH-STYLE address of the same
/// object: a virtual-hosted-style request's path is the key alone, so the labels every
/// operation of this service binds (`/{Bucket}`, `/{Bucket}/{Key+}`) read it from here.
///
/// The one place the two address forms are reduced to one, so the request this service
/// ROUTES and the request it DESERIALIZES cannot disagree about which bucket was named.
pub(crate) fn path_with_bucket(bucket: &str, path: &str) -> String {
    match path {
        // `https://bucket.s3.amazonaws.com/` addresses the BUCKET itself (`HeadBucket`,
        // `ListObjects`), so its path-style form carries no trailing segment at all.
        "" | "/" => format!("/{bucket}"),
        path => format!("/{bucket}{path}"),
    }
}

/// The query parameters a LEGACY presigned URL carries its credential in: the access key
/// id, the signature and the expiry, all three required — "`AWSAccessKeyId`, `Signature`
/// and `Expires`" (AWS General Reference, *Authenticating requests: using query
/// parameters*). The signing scope of that form names no service, which is why a request
/// carrying it resolves no routing key at all.
const LEGACY_PRESIGNED_PARAMETERS: [&str; 3] = ["AWSAccessKeyId", "Signature", "Expires"];

/// Whether `uri`'s query carries all three of [`LEGACY_PRESIGNED_PARAMETERS`] — the shape
/// that makes a request a legacy presigned S3 URL rather than any other unroutable request.
fn carries_legacy_presigned_credential(uri: &http::Uri) -> bool {
    let Some(query) = uri.query() else {
        return false;
    };
    let names: ::std::collections::BTreeSet<&str> = query
        .split('&')
        .filter_map(|pair| pair.split('=').next())
        .collect();
    LEGACY_PRESIGNED_PARAMETERS
        .iter()
        .all(|parameter| names.contains(parameter))
}

/// The host a request was addressed to: its `Host` header, else the authority its URI
/// carries (which is where HTTP/2 puts it, and where an in-process caller's absolute URI
/// carries it).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())
}

/// [`request_host`] for the runtime-API request the routing seam is handed.
fn request_host_of(req: &aws_smithy_runtime_api::http::Request) -> Option<&str> {
    req.headers()
        .get(http::header::HOST.as_str())
        .or_else(|| authority_of(req.uri()))
}

/// The authority of an absolute request URI (`https://host/path` -> `host`), or `None` for
/// the origin-form URI (`/path`) a request from the wire carries.
fn authority_of(uri: &str) -> Option<&str> {
    let after_scheme = uri.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(after_scheme);
    (!authority.is_empty()).then_some(authority)
}

/// `req` with the PATH-STYLE address of the same object: the request the model's routing
/// table can name, which is what [`Service::override_operation`] resolves the operation
/// from. Everything else about the request — its method, its query and its headers — is
/// carried through unchanged, because the route is decided by all three.
///
/// `None` when the rewritten URI is not one the HTTP layer can hold, which leaves the
/// request to the generated routing table rather than failing the lookup.
fn with_path_style_address(
    req: &aws_smithy_runtime_api::http::Request,
    bucket: &str,
) -> Option<aws_smithy_runtime_api::http::Request> {
    let uri = req.uri();
    let path = wire::request_path(uri);
    let query = wire::request_query(uri);
    let rewritten = match query.is_empty() {
        true => path_with_bucket(bucket, path),
        false => format!("{}?{query}", path_with_bucket(bucket, path)),
    };
    // The body is not read by a route lookup (this service's protocol routes on the method,
    // the path, the query and the presence of bound headers), so an empty one stands in for
    // it rather than copying the payload of every object write.
    let mut routable =
        aws_smithy_runtime_api::http::Request::new(::aws_smithy_types::body::SdkBody::empty());
    routable.set_uri(rewritten.as_str()).ok()?;
    routable.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        routable
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Some(routable)
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

/// The XML element a member of a request DOCUMENT is spelled with: the member's own
/// declared name, which a violation path carries with its first character lower-cased (the
/// constraint walk's spelling of a member), so it is recovered by upper-casing that
/// character back. restXml writes a member under its declared name, and no member read
/// through here is renamed by the model.
fn element_of(member_path_segment: &str) -> String {
    let mut characters = member_path_segment.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
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
/// `max-buckets`, the `id` query parameter the id-addressed bucket configuration operations
/// share ([`CONFIGURATION_ID_PARAMETER`]), and `UpdateObjectEncryption`'s `ObjectEncryption`
/// payload, whose `InvalidRequest` messages that operation's own `@documentation` lists
/// (`smithy_query(query="doc",shape="UpdateObjectEncryption")`).
fn published_refusal(violation: &Violation) -> Option<Refused> {
    let mut segments = violation.path.split('.');
    let member = segments.next()?;
    // The validating walk WITHHOLDS the value of a `@sensitive` member as it records the
    // violation (`KMSKeyArn` is one), leaving the clause no value half at all, so a
    // sentence that quotes the value reads it from the request's own XML document instead
    // — the same reading an `<ArgumentValue>` echo makes, and the one place this service
    // recovers the TEXT a caller sent.
    let withheld = violation
        .sensitive
        .then(|| {
            violation
                .path
                .rsplit('.')
                .next()
                .map(element_of)
                .and_then(|element| wire::spelled_in_request(&element))
        })
        .flatten();
    let rejected = withheld
        .as_deref()
        .or(violation.value.as_deref())
        .unwrap_or_default();
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
        // The `x-amz-copy-source` of the two copies. The value is put into the form the
        // member's pattern is applied to before it is judged
        // ([`crate::state::canonical_copy_source`]), so what is left for the pattern to
        // refuse is a value naming no key at all — which is the same thing the operation's
        // own reader refuses a value it cannot resolve for, so the sentence is the one that
        // reader's fault publishes ([`crate::state::copy_source_fault`]) and the document
        // carries the same argument pair beside it ([`refused_argument`]).
        "copySource" => (
            crate::state::INVALID_ARGUMENT_CODE,
            crate::state::copy_source_fault(rejected)
                .unwrap_or(crate::state::CopySourceFault::Uri)
                .message()
                .to_string(),
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
        // The id a bucket configuration is addressed by ([`CONFIGURATION_ID_PARAMETER`]): a
        // call that states none names no configuration at all, which S3 reports against the
        // id itself rather than as a bare bad argument — the same refusal a handler answers
        // an id it was handed empty with
        // (`crate::state::ConfigurationIdFault::Unstated`), so the two cannot disagree about
        // a call that identifies nothing.
        "id" => (
            crate::state::INVALID_CONFIGURATION_ID_CODE,
            crate::state::INVALID_CONFIGURATION_ID_MESSAGE.to_string(),
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
        // `PutBucketNotificationConfiguration`'s container: S3 refuses every notification
        // validation failure with the one generic code, so the MESSAGE is what says which
        // rule was broken — and an `Events` entry outside the published `Event` set is the
        // one such value the model can refuse by itself, which would otherwise go out as the
        // generic schema refusal and blame the document's shape instead of the event name.
        // The sentence is FIXED text, and it is the same one the handler's own check of a
        // stored container answers with
        // (`crate::state::NotificationConfigurationRejection::UnsupportedEvent`).
        //
        // Nothing else in the container is refused here: every other rule it breaks is one
        // the model cannot state, which the handler refuses itself.
        "notificationConfiguration" => {
            let unsupported_event = matches!(violation.kind, ViolationKind::EnumValue(_))
                && segments.any(|segment| segment == "events");
            if !unsupported_event {
                return None;
            }
            (
                crate::state::INVALID_ARGUMENT_CODE,
                crate::state::UNSUPPORTED_EVENT_MESSAGE.to_string(),
            )
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
///
/// `pub(crate)`: the store asks the same question of a request's query string when it reads
/// whether the request presented a credential there
/// ([`crate::state::request_is_anonymous`]), so both read a parameter list the one way.
pub(crate) fn query_carries(query: &str, name: &str) -> bool {
    query
        .split('&')
        .any(|parameter| parameter.split('=').next().unwrap_or(parameter) == name)
}

/// The VALUE a request URI's query string gives the parameter `name`, or `None` when it
/// carries no such parameter. A parameter spelled as a bare flag (`?id`) states the empty
/// value, which is what it names.
///
/// The first occurrence is what answers, the way a query reader takes one value for one
/// parameter; nothing here decodes, because the callers ask only whether a value is empty.
fn query_value<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query.split('&').find_map(|parameter| {
        match parameter.split_once('=') {
            Some((key, value)) if key == name => Some(value),
            // A bare flag: the parameter is there and names the empty value.
            None if parameter == name => Some(""),
            _ => None,
        }
    })
}

/// Whether `path` addresses an OBJECT — S3's `/{Bucket}/{Key+}`, a non-empty bucket
/// segment with a non-empty key below it — rather than a bucket or the service root.
fn addresses_object(path: &str) -> bool {
    match path.trim_start_matches('/').split_once('/') {
        Some((bucket, key)) => !bucket.is_empty() && !key.is_empty(),
        None => false,
    }
}

/// Whether a request addresses the S3 Select data plane: the `select` subresource of an
/// object, read from `query` and from `path` — the request's address in PATH-STYLE form
/// ([`EmulatedService::path_style_path`]), so a Select request resolves the same whether
/// its bucket arrived in the path or in the host.
///
/// The method is deliberately not part of this predicate, and costs nothing: it is
/// consulted ONLY for a request no modeled operation claimed, and every other method on an
/// object path (`GET`/`HEAD`/`PUT`/`DELETE`) resolves to its own modeled operation before
/// ever reaching here. What is left is a request that asked S3 for Select and nothing else.
fn addresses_s3_select(path: &str, query: &str) -> bool {
    query_carries(query, SELECT_SUBRESOURCE) && addresses_object(path)
}

/// The query parameter that addresses a DIRECTORY bucket's session.
///
/// `CreateSession` is bound at `GET /{Bucket}?session`
/// (`smithy_query(query="operation",operation="CreateSession",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports that `@http` uri), so the `session` SUBRESOURCE is what names the feature.
const SESSION_SUBRESOURCE: &str = "session";

/// The operation a `GET /{Bucket}?session` against a GENERAL PURPOSE bucket names.
///
/// A session is a DIRECTORY bucket's subresource: `CreateSession` "creates a session that
/// establishes temporary security credentials to support fast authentication and
/// authorization for the Zonal endpoint API operations on directory buckets" (its own
/// documentation), and a general purpose bucket has no such subresource at all. S3 does not
/// refuse a query parameter that names no subresource of the bucket it was addressed to — it
/// ignores it — so what is left of the request is `GET /{Bucket}`, the bucket's own listing,
/// which is the operation bound to that method and path with no subresource.
const LISTING_WITHOUT_SUBRESOURCE: &str = "ListObjects";

/// The query parameter a bucket configuration's id arrives in.
///
/// Every one of the twelve id-addressed bucket configuration operations binds its id as
/// `Id` `@httpQuery("id")` (`smithy_query(query="operation",
/// operation="PutBucketAnalyticsConfiguration")`; `OPERATIONS.md` restates the same binding
/// for the metrics, S3 Inventory and S3 Intelligent-Tiering families), so ONE parameter
/// name names the id of all of them.
const CONFIGURATION_ID_PARAMETER: &str = "id";

/// The bucket subresources whose READ, WRITE and DELETE are addressed by a configuration
/// id, as `(subresource, method, operation)`.
///
/// Each row is one `@http` uri the model binds — `GET`/`PUT`/`DELETE`
/// `/{Bucket}?<subresource>` with `Id` required — for the four families a bucket holds
/// per-id configurations of: analytics, metrics, S3 Inventory and S3 Intelligent-Tiering.
/// The `GET` row names the family's LISTING, which is the operation the model binds at that
/// same method and path with NO id and so the one a read that identifies no configuration
/// asks for.
///
/// Read by [`EmulatedService::configuration_subresource_operation`], the one place a
/// subresource call that states no id is kept off the bucket-level route.
const ID_ADDRESSED_CONFIGURATION_ROUTES: &[(&str, &str, &str)] = &[
    ("analytics", "GET", "ListBucketAnalyticsConfigurations"),
    ("analytics", "PUT", "PutBucketAnalyticsConfiguration"),
    ("analytics", "DELETE", "DeleteBucketAnalyticsConfiguration"),
    ("metrics", "GET", "ListBucketMetricsConfigurations"),
    ("metrics", "PUT", "PutBucketMetricsConfiguration"),
    ("metrics", "DELETE", "DeleteBucketMetricsConfiguration"),
    ("inventory", "GET", "ListBucketInventoryConfigurations"),
    ("inventory", "PUT", "PutBucketInventoryConfiguration"),
    ("inventory", "DELETE", "DeleteBucketInventoryConfiguration"),
    (
        "intelligent-tiering",
        "GET",
        "ListBucketIntelligentTieringConfigurations",
    ),
    (
        "intelligent-tiering",
        "PUT",
        "PutBucketIntelligentTieringConfiguration",
    ),
    (
        "intelligent-tiering",
        "DELETE",
        "DeleteBucketIntelligentTieringConfiguration",
    ),
];

/// The refusal S3 publishes for a body member whose value the validating walk WITHHELD.
///
/// The walk drops the value of a member the model marks `@sensitive`
/// (`SSEKMSEncryption$KMSKeyArn` is one), and S3's published refusal for that member names
/// WHY the submitted ARN is not one — a reason read off the value itself
/// ([`crate::state::malformed_arn_reason`]). So the value half is restored from the door's
/// own echo of the request and the clause is then worded by [`s3_refusal`], the one place
/// S3's refusal rules live: a value this service publishes a refusal for keeps that
/// refusal, and everything else keeps exactly the wording the walk behind the door would
/// have given it.
fn submitted_value_wording(
    refused: &[(Violation, ::service_common::validation::SubmittedEcho)],
) -> Option<Refused> {
    let (violation, echo) = refused.first()?;
    let restored =
        ::service_common::validation_message::quoting_the_whole_submitted_value(violation.clone(), echo);
    Some(s3_refusal(&restored))
}

/// The refusal this service words for a value the boundary rejected OUTSIDE the request
/// document, or `None` for one it leaves to the layers behind the door.
///
/// Only a value whose document carries MEMBERS of its own is worded here
/// ([`refused_argument`] decides which): those members can be written only by the renderer
/// below ([`EmulatedService::detailed_refusal`]), which the front door is what routes such a
/// refusal through — every other refused value keeps exactly the wording the walk behind the
/// door gives it, through [`Service::override_refusal`].
///
/// The value half is restored from the door's own echo of the request first, because the
/// sentence depends on it: which of the two copy-source faults a value carries is read off
/// the value ([`crate::state::copy_source_fault`]).
fn uri_value_wording(
    refused: &[(Violation, ::service_common::validation::SubmittedEcho)],
) -> Option<Refused> {
    let (violation, echo) = refused.first()?;
    let restored = ::service_common::validation_message::quoting_the_whole_submitted_value(
        violation.clone(),
        echo,
    );
    refused_argument(&restored).map(|_| s3_refusal(&restored))
}

/// The `<ArgumentName>` / `<ArgumentValue>` pair S3's document carries for the request field
/// `violation` names, or `None` for a field whose refusal carries no member of its own.
///
/// S3's `InvalidArgument` document names the field it rejected and the value that field
/// carried (Amazon S3 API Reference, *Error responses*), which for a HEADER is the header as
/// the API publishes it and the value as the request sent it. The two copy DIRECTIVES are
/// that case; `x-amz-copy-source` is the one where the refusal is about the value being
/// unreadable AS a reference, so the document names the FIELD in both halves — there is no
/// text to echo that is not the caller's own.
///
/// Only a field of an operation served behind the door is reached here ([`front_door`]),
/// which is what the rows are: the value is put into the form the member's pattern is applied
/// to before the pattern sees it ([`crate::state::canonical_copy_source`]), so what is left
/// for the boundary to reject on the copies is one of these three headers.
fn refused_argument(violation: &Violation) -> Option<crate::scaffolded::ErrorDetail> {
    let member = violation.path.split('.').next()?;
    let (name, value) = match member {
        COPY_SOURCE_MEMBER => (
            crate::state::COPY_SOURCE_ARGUMENT,
            crate::state::COPY_SOURCE_ARGUMENT.to_string(),
        ),
        "metadataDirective" => (
            METADATA_DIRECTIVE_HEADER,
            violation.value.clone().unwrap_or_default(),
        ),
        "taggingDirective" => (
            TAGGING_DIRECTIVE_HEADER,
            violation.value.clone().unwrap_or_default(),
        ),
        _ => return None,
    };
    Some(crate::scaffolded::ErrorDetail::Argument {
        name: name.to_string(),
        value: Some(value),
    })
}

/// The input member the two copies bind `x-amz-copy-source` from, as a violation path spells
/// it (lowerCamel): `CopySource`
/// (`smithy_query(query="operation",operation="CopyObject,UploadPartCopy")`).
const COPY_SOURCE_MEMBER: &str = "copySource";

/// The headers `CopyObject` binds its two directives from
/// (`smithy_query(query="operation",operation="CopyObject")` reports
/// `MetadataDirective: {httpHeader: "x-amz-metadata-directive"}` and
/// `TaggingDirective: {httpHeader: "x-amz-tagging-directive"}`) — the names S3's document
/// reports as the argument it rejected.
const METADATA_DIRECTIVE_HEADER: &str = "x-amz-metadata-directive";
/// See [`METADATA_DIRECTIVE_HEADER`].
const TAGGING_DIRECTIVE_HEADER: &str = "x-amz-tagging-directive";

/// The operations served behind this service's FRONT DOOR, and why each is.
///
/// A door re-reads the request it is handed, finds the modelled constraints it breaks, and
/// words the refusal through the service's own renderer — which is the only way a refusal
/// the BOUNDARY makes can carry the members S3's document carries for it (the walk the
/// framework runs behind the door renders its own refusal directly). Everything a door does
/// not refuse is handed to the operation, which owns deserialize -> validate -> handle ->
/// serialize exactly as before.
///
/// * `UpdateObjectEncryption` — the refusal of a KMS key ARN that is not one names WHY, a
///   reason read off the submitted ARN, which the validating walk withholds because the
///   model marks that member `@sensitive` ([`submitted_value_wording`]).
/// * `CopyObject` and `UploadPartCopy` — the refusal of an `x-amz-copy-source` the modelled
///   pattern rejects carries the sentence its fault publishes and the argument pair naming
///   the header ([`uri_value_wording`]).
fn front_door<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> smithy_server_runtime::OperationRoutingTable {
    use ::service_common::front_door::{FrontDoor, FrontDoorTable};
    use smithy_server_runtime::Operation;

    let mut door = FrontDoorTable::worded(submitted_value_wording);
    door.serve(crate::ops::update_object_encryption::UpdateObjectEncryption::new(
        Arc::clone(engine),
    ));
    let mut table = door.finish();
    table.add_operation(
        crate::ops::copy_object::CopyObject::<Eng>::NAME,
        Box::new(
            FrontDoor::worded(
                crate::ops::copy_object::CopyObject::new(Arc::clone(engine)),
                submitted_value_wording,
            )
            .wording_uri(uri_value_wording),
        ),
    );
    table.add_operation(
        crate::ops::upload_part_copy::UploadPartCopy::<Eng>::NAME,
        Box::new(
            FrontDoor::worded(
                crate::ops::upload_part_copy::UploadPartCopy::new(Arc::clone(engine)),
                submitted_value_wording,
            )
            .wording_uri(uri_value_wording),
        ),
    );
    table
}

/// The `com-amazonaws-s3-amazons3` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threaded into every operation, kept here
    /// too because the refusal wording below has to ask whether this service ISSUED the
    /// credential a refused request presented, and the sessions that answer that are the
    /// ones `CreateSession` recorded ([`Self::issued_credential`]). Never a store of its
    /// own: it is the `Arc` the router handed to `new`.
    engine: Arc<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it. See [`wire`] for what it
    /// changes about the generated one: the output document of the operations the model
    /// marks `s3UnwrappedXmlOutput` carries no envelope.
    protocol: UnwrappedXmlRestXml,
    /// The operations served behind this service's front door, each for the refusal only a
    /// door can word — see [`front_door`] for the list and the reason. Built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    door: smithy_server_runtime::OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // What the wire layer asks when it has to tell a virtual-hosted-style address from a
        // path-style one ([`addressed_in_host`]): whether a name is a bucket this emulator
        // holds. The closure captures the SAME shared engine every operation reads, so the
        // answer is the store's rather than a copy of it; it is read-only.
        let buckets = Arc::clone(&engine);
        let holds_bucket: wire::BucketLookup =
            Arc::new(move |name: &str| holds_bucket_in(&*buckets, name));
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            door: front_door(&engine),
            engine,
            protocol: UnwrappedXmlRestXml::new().with_buckets(holds_bucket),
        }
    }

    /// The front-door entry for `req`, when the operation its address names is one served
    /// behind the door ([`front_door`]), else `None` so the generated routing table resolves
    /// the request exactly as it always did.
    ///
    /// The key is the one the ENTRY's protocol resolves — the same key the generated table
    /// is read by — so this names no route of its own.
    fn front_door_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.door.get(key)
    }

    /// Whether `name` is a bucket this emulator holds — the question an ADDRESS resolves
    /// against (see [`addressed_in_host`]). Read-only: the guard is taken, the answer is
    /// read, and nothing is written.
    fn holds_bucket(&self, name: &str) -> bool {
        holds_bucket_in(&*self.engine, name)
    }

    /// The bucket a VIRTUAL-HOSTED-STYLE request names in the first label of its host, when
    /// it is one ([`addressed_in_host`]) — the one place both routing seams below ask that
    /// question, so the route a request resolves and the address its refusal is judged
    /// against cannot disagree about which bucket was named. `path` is the request's own
    /// path, which the caller has already read and which the answer depends on (a path-style
    /// request names its bucket there, and is never read out of the host).
    fn host_addressed_bucket<'r>(&self, req: &'r RtRequest, path: &str) -> Option<&'r str> {
        let host = request_host_of(req)?;
        addressed_in_host(host, path, &|name| self.holds_bucket(name))
    }

    /// The address `req` names, in the PATH-STYLE form every route of this service is bound
    /// in (`/{Bucket}`, `/{Bucket}/{Key+}`): a virtual-hosted-style request carries its
    /// bucket in the host and the key alone in the path, so the bucket is put back in front
    /// of it ([`path_with_bucket`], the same normalization [`Service::override_operation`]
    /// routes from); a path-style request's path already is that form and is borrowed
    /// unchanged.
    fn path_style_path<'r>(&self, req: &'r RtRequest) -> Cow<'r, str> {
        let path = wire::request_path(req.uri());
        match self.host_addressed_bucket(req, path) {
            Some(bucket) => Cow::Owned(path_with_bucket(bucket, path)),
            None => Cow::Borrowed(path),
        }
    }

    /// Whether `access_key_id` names a credential this emulator ISSUED, and so one an
    /// authentication refusal has RESOLVED: the key pair the process publishes at start, or
    /// a session `CreateSession` vended and has not released.
    ///
    /// Each is read from the record that owns it rather than from a copy, so a session that
    /// has expired stops resolving at the moment its own record goes
    /// ([`crate::state::ComAmazonawsS3Amazons3State::release_expired_sessions`]).
    fn issued_credential(&self, access_key_id: &str) -> bool {
        if ::emulator_server::request_authentication::SigV4Authenticator::minted_credentials()
            .is_some_and(|published| published.access_key_id == access_key_id)
        {
            return true;
        }
        crate::state::service_state(&*self.engine)
            .is_some_and(|state| state.lock().holds_vended_session(access_key_id))
    }

    /// Whether `req` asks for the `session` subresource of a bucket that has none: a
    /// GENERAL PURPOSE bucket this emulator holds, addressed in either form (the address is
    /// read in the PATH-STYLE form every route of this service is bound in, so a
    /// virtual-hosted-style request resolves the same way).
    ///
    /// Read-only: the guard is taken, the bucket's kind is read, and nothing is written. A
    /// name this emulator holds no bucket for answers `false` — there is no bucket whose kind
    /// says the subresource is not its, so the request keeps the route the model gives it and
    /// is answered about the address it named.
    fn addresses_session_of_general_purpose_bucket(&self, req: &RtRequest) -> bool {
        if !query_carries(wire::request_query(req.uri()), SESSION_SUBRESOURCE) {
            return false;
        }
        let path = self.path_style_path(req);
        let Some(name) = first_path_segment(&path) else {
            return false;
        };
        if addresses_object(&path) {
            return false;
        }
        crate::state::service_state(&*self.engine).is_some_and(|state| {
            state
                .lock()
                .bucket(name)
                .is_some_and(|bucket| !bucket.is_directory())
        })
    }

    /// The operation a request on an ID-ADDRESSED bucket configuration subresource names
    /// when it states no configuration id — none at all, or an empty one — or `None` for
    /// every other request, which keeps the route the generated table gives it.
    ///
    /// The model binds each of those reads, writes and deletes at `/{Bucket}?<subresource>`
    /// with `Id` as a REQUIRED `@httpQuery` member
    /// ([`ID_ADDRESSED_CONFIGURATION_ROUTES`]), and the generated table drops a route whose
    /// required bindings the request does not carry — so a call that names the subresource
    /// but no id falls through to the route bound to the BARE path, which for `PUT` is
    /// `CreateBucket` and for `DELETE` is `DeleteBucket`. A bucket subresource is not the
    /// bucket: S3 serves such a request as the subresource call it addressed — the LISTING of
    /// that subresource for a `GET`, which is the operation the model binds there with no id,
    /// and the write or the delete it addressed for the other two, refused for the id it did
    /// not state — so it is resolved here rather than becoming a create or a delete of the
    /// bucket itself.
    ///
    /// An id the request states EMPTY identifies no configuration either, so it resolves the
    /// same way: a `GET` of the subresource with an empty id is that subresource's listing.
    ///
    /// The address is read in the PATH-STYLE form every route of this service is bound in
    /// ([`Self::path_style_path`]), so the two address forms resolve alike, and an OBJECT
    /// path is left alone: these subresources are a bucket's.
    fn configuration_subresource_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let query = wire::request_query(req.uri());
        // An id the request states non-empty leaves the generated table's own lookup to
        // resolve the call, and the handler to judge the value.
        if query_value(query, CONFIGURATION_ID_PARAMETER).is_some_and(|id| !id.is_empty()) {
            return None;
        }
        let path = self.path_style_path(req);
        if first_path_segment(&path).is_none() || addresses_object(&path) {
            return None;
        }
        let operation = ID_ADDRESSED_CONFIGURATION_ROUTES
            .iter()
            .find_map(|(subresource, method, operation)| {
                (*method == req.method() && query_carries(query, subresource)).then_some(*operation)
            })?;
        self.entry.routing_table().get(operation)
    }

    /// How this service answers the AUTHENTICATION refusal of the request it is serving, or
    /// `None` for a refusal this service does not word — which keeps the framework's own.
    ///
    /// The framework's structural refusal says only that authentication refused the caller,
    /// so which refusal it is follows from what the REQUEST presented, which [`sigv4`] read
    /// once for this request and answers from. Rendered through THIS service's own protocol,
    /// in the same channel every other refusal of this service goes out in, so the envelope
    /// a client parses — the bare `<Error>` root with its `<Code>`, `<Message>` and
    /// request-correlation elements, and the matching `x-amz-request-id` / `x-amz-id-2`
    /// headers — is the one it already reads; the one difference is the elements S3 carries
    /// INSIDE it for a signature mismatch, and the status it answers an unusable
    /// query string with.
    ///
    /// `None` also on the serializer failure fixed text cannot actually provoke, which
    /// leaves the framework's own refusal to answer rather than panicking in the request
    /// path.
    fn authentication_refusal_response(&self) -> Option<RtResponse> {
        let presented = sigv4::presented()?;
        let refusal = sigv4::refusal_for(
            &presented,
            &|access_key_id| self.issued_credential(access_key_id),
            &|bucket| self.anonymous_refusal_message(bucket),
        )?;
        self.protocol
            .serialize_error(
                &refusal,
                &sigv4::REFUSAL_SCHEMA,
                refusal.code,
                refusal.status,
            )
            .ok()
    }

    /// The sentence S3 refuses a request carrying NO credential with, for a request whose
    /// address resolves to `bucket` (`None` for one that addresses no bucket).
    ///
    /// The code is `AccessDenied` for every such refusal, so the sentence is the only thing
    /// telling a caller WHICH refusal it is — and which one that is turns on the bucket the
    /// request addresses, read from the store rather than from the request: a bucket that
    /// makes the REQUESTER pay cannot bill a caller that names no account, whatever it
    /// grants to everyone ([`crate::state::Bucket::requester_pays_anonymous_refusal`] holds
    /// that rule, the same one an operation's own authorization answers with), and anything
    /// else is the authorization refusal S3 answers a caller a bucket grants nothing to.
    ///
    /// Read-only: the guard is taken, the bucket's configuration is read, and nothing is
    /// written — including for a name the store holds no bucket for, which is the generic
    /// refusal because no bucket's configuration says otherwise.
    fn anonymous_refusal_message(&self, bucket: Option<&str>) -> &'static str {
        bucket
            .and_then(|name| {
                let state = crate::state::service_state(&*self.engine)?;
                let store = state.lock();
                store.bucket(name)?.requester_pays_anonymous_refusal()
            })
            .unwrap_or(crate::state::ACCESS_DENIED_MESSAGE)
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

    /// `refused` rendered with the DETAIL members S3's document carries for it
    /// ([`refused_argument`]), in the same envelope and at the same status a constraint
    /// refusal is rendered with: HTTP 400, the status every protocol answers a value outside
    /// its declared constraints with.
    ///
    /// This is the one renderer a BOUNDARY refusal can carry members through: the wording
    /// seam ([`Service::override_refusal`]) chooses the code and the message and nothing
    /// else, so a document that also names the argument it rejected has to be written here.
    /// An implemented handler's own refusal carries the same members through the same
    /// serializer ([`crate::scaffolded::Scaffolded::naming`]).
    ///
    /// `None` on the serializer failure a fixed code, a fixed message and one named argument
    /// cannot actually provoke, which leaves the entry's renderer to answer rather than
    /// panicking in the request path.
    fn detailed_refusal(
        &self,
        refused: &Refused,
        detail: crate::scaffolded::ErrorDetail,
    ) -> Option<RtResponse> {
        let refusal = crate::scaffolded::UnmodeledWithDetail {
            message: refused.message.clone(),
            detail,
        };
        self.protocol
            .serialize_error(
                &refusal,
                &crate::scaffolded::UNMODELED_SCHEMA,
                &refused.code,
                crate::state::INVALID_ARGUMENT_STATUS,
            )
            .ok()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry, which
        // the implementation below delegates every one of them to, UNCHANGED. The one
        // exception is the renderer for an AUTHENTICATION refusal, which S3 answers with
        // its own status and its own elements; see this file's header and [`sigv4`]. Do
        // not edit the entry.
        self
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
        //
        // It also claims the S3 EXPRESS signing name: the Zonal endpoint surface of this
        // same model — `CreateSession` and the object operations on a directory bucket —
        // is signed for `s3express` rather than for `s3`, and this is the service that
        // serves those operations. No other emulated service signs as `s3express`, so the
        // widened claim leaves exactly one claimant for such a request instead of none.
        let claimed = match smithy_server_runtime::resolve_route_key(parts) {
            Some(key)
                if key == self.entry.identity().signing_name() || key == S3_EXPRESS_SIGNING_NAME =>
            {
                !addresses_peer_path(parts, PEER_PATH_SEGMENTS)
            }
            // A VIRTUAL-HOSTED-STYLE request that names no service in a credential: with no
            // `Authorization` header and no presigned credential scope, the only routable
            // signal left is the host — whose first label is the BUCKET, not a service, so
            // the resolved key is that bucket's name and no service would claim the request
            // at all. It is this service's when that label addresses a bucket this emulator
            // holds ([`addressed_in_host`]); requiring the resolved key to BE that label is
            // what keeps a request signed for another service (whose own key the credential
            // named) out of this branch.
            Some(key) => request_host(parts)
                .and_then(|host| {
                    addressed_in_host(host, parts.uri.path(), &|name| self.holds_bucket(name))
                })
                .is_some_and(|bucket| key == bucket),
            // A request whose ONLY credential is a query-string one the framework's routing
            // key does not read: the legacy presigned form, whose three parameters
            // ([`LEGACY_PRESIGNED_PARAMETERS`]) are S3's own — "you can use query parameters
            // to … authenticate the request" (AWS General Reference, *Authenticating
            // requests: using query parameters*). It names no service in a scope, so the
            // framework resolves no routing key and no service would claim it; where the
            // credential travels is not part of addressing, so it is this service's request
            // and is claimed as one. Whether it then VERIFIES is the framework's own
            // judgement, exactly as for the SigV4 query form.
            //
            // An ANONYMOUS (unsigned) request names no service either: it carries no
            // credential scope and no `X-Amz-Target`, and the endpoint host a client of this
            // emulator addresses is a loopback name the framework reads no service label out
            // of — so no routing key is resolved and no service would claim it, leaving the
            // router to answer its "no such service" 404. S3 is nonetheless reached
            // anonymously by design ("if you grant READ access to the anonymous user, the
            // GetObject operation returns the object without using an authorization header",
            // `GetObject`'s own documentation), and such a request is this service's when
            // its PATH addresses a bucket this emulator holds — the path-style half of the
            // host-addressed rule above, resolved against the same buckets. What the caller
            // may then do is the operation's own authorization question, which a refusal
            // answers with S3's own code rather than with a routing error.
            None => {
                carries_legacy_presigned_credential(&parts.uri)
                    || first_path_segment(parts.uri.path())
                        .is_some_and(|segment| self.holds_bucket(segment))
            }
        };
        claimed
    }

    fn override_operation(
        &self,
        req: &RtRequest,
    ) -> Option<&dyn ErasedOperation> {
        // What this request presented to authentication, kept for the length of the
        // request so a refusal of it can be worded for the condition that refused it (see
        // [`sigv4`]). Recorded HERE because this is the seam the framework reaches with the
        // whole request — its body included, which is what the payload half of a canonical
        // request is built from — and it reaches it BEFORE it authenticates, for the one
        // service that claimed the request.
        //
        // The BUCKET the request addresses travels with it, resolved from the request's
        // PATH-STYLE address ([`Self::path_style_path`]) so a virtual-hosted-style request
        // names the same bucket a path-style one does: a refusal of an anonymous caller is
        // worded from that bucket's own configuration, and the seam that words it is handed
        // no request to read it from.
        sigv4::remember(req, first_path_segment(&self.path_style_path(req)));
        // A `?session` request against a bucket that HAS no session subresource — a general
        // purpose bucket — is the bucket's own listing and not `CreateSession`; see
        // [`LISTING_WITHOUT_SUBRESOURCE`]. Resolved before the model's routing table, which
        // matches the subresource whatever bucket it was addressed to, and from the request's
        // PATH-STYLE address so the two address forms resolve alike.
        if self.addresses_session_of_general_purpose_bucket(req) {
            return self
                .entry
                .routing_table()
                .get(LISTING_WITHOUT_SUBRESOURCE);
        }
        // A write or a delete addressing an ID-ADDRESSED bucket configuration subresource
        // that states no `id` is that subresource's call with an id missing, never the
        // bucket-level `CreateBucket` or `DeleteBucket` bound to the same method and the bare
        // path; see [`Self::configuration_subresource_operation`]. Resolved before the
        // model's routing table, which drops the subresource route for the required binding
        // the request does not carry.
        if let Some(operation) = self.configuration_subresource_operation(req) {
            return Some(operation);
        }
        // A VIRTUAL-HOSTED-STYLE request names its bucket in the host, so its path is the
        // key alone — `GET /key-name` — which the model's routing table cannot name: every
        // route of this service begins with the `{Bucket}` label, so the key would be read
        // as the bucket and a read of an object would resolve as a read of a BUCKET that
        // does not exist. The route is resolved from the PATH-STYLE address of the same
        // object instead ([`path_with_bucket`]), through the same routing table and the same
        // protocol the model-derived lookup uses; the matched operation then binds its
        // `{Bucket}` label from that same normalized path (see
        // [`wire::UnwrappedXmlRestXml::deserialize_input`]).
        //
        // For a PATH-STYLE request the generated routing table resolves it exactly as
        // before, except for the one operation served behind the front door below.
        let Some(bucket) = self.host_addressed_bucket(req, wire::request_path(req.uri())) else {
            return self.front_door_operation(req);
        };
        let rewritten = with_path_style_address(req, bucket)?;
        // The ENTRY's protocol, which is the one carrying this service's model-derived
        // routes (this file's own `self.protocol` serializes and deserializes; it was built
        // with no route table, because routing is the entry's job).
        let operation = self
            .entry
            .protocol()
            .routing_key(&rewritten)
            .ok()?
            .to_string();
        self.front_door_operation(&rewritten)
            .or_else(|| self.entry.routing_table().get(&operation))
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, a value outside a declared bound, or a caller
        // authentication refused) to let this service word it. S3's refusals carry
        // S3's own error codes, never the framework's generic
        // `ValidationException`: see [`s3_refusal`] for the rule — a value refused
        // inside the request's XML document is `MalformedXML`, one refused in a
        // header, a query parameter or a URI label is `InvalidArgument`. The FIRST
        // violation is what the answer names, the way the protocol's own envelope
        // reports one code.
        match refusal {
            Refusal::Invalid(violations) => violations.first().map(s3_refusal),
            // An AUTHENTICATION refusal is worded in
            // [`Self::authentication_refusal_response`] instead, which this seam cannot
            // reach: S3's answer to one differs from the framework's rendering in its
            // STATUS and in the ELEMENTS it carries, and neither travels on a [`Refused`].
            // Declining here leaves the framework's own code and message for the renderer
            // to fall back on, which is what a refusal this service does not word keeps.
            _ => None,
        }
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
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
        //
        // WHICH requests those are is decided on the request's PATH-STYLE address
        // ([`Self::path_style_path`]), not on the raw path: a virtual-hosted-style Select
        // request — the form every SDK sends for an endpoint addressed by bucket name —
        // carries its bucket in the host and the key alone in the path, and reading that raw
        // path as the address made it look like a request against a bucket rather than
        // against an object, which is not Select and fell through to the 501.
        if !addresses_s3_select(&self.path_style_path(req), wire::request_query(req.uri())) {
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
        error: &ProtocolError,
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

/// The model-derived half of this service, DELEGATED to the generated entry — every datum
/// baked at codegen is read off it, so nothing here can drift from the model — with the one
/// renderer this service owns.
///
/// `refusal_response` is that renderer, and it is the reason this file implements the trait
/// at all rather than handing the entry back bare: it is the only place an AUTHENTICATION
/// refusal's STATUS and ELEMENTS can be chosen, and S3 chooses both (see the file header and
/// [`sigv4`]). Everything else is the entry's.
impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for EmulatedService<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.entry.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.entry.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        // The ONE refusal the framework renders through here is the authentication one: a
        // caller it refused before any handler ran, which this service words itself.
        // Anything else — and an authentication refusal this service declines to word — falls
        // through to the ENTRY's renderer, `chosen` included, so a wording chosen through
        // `override_refusal` is never dropped on the way past.
        if matches!(error, ProtocolError::Unauthenticated { .. }) {
            if let Some(response) = self.authentication_refusal_response() {
                return response;
            }
        }
        // A refusal a FRONT DOOR worded ([`front_door`]) reaches this renderer with the
        // violations it found, which is what lets the document also carry the members S3
        // names the refused argument by ([`refused_argument`]); everything else keeps the
        // entry's rendering of the code and message alone.
        if let ProtocolError::Invalid(violations) = &error {
            if let Some(violation) = violations.first() {
                if let Some(detail) = refused_argument(violation) {
                    let refused = chosen.clone().unwrap_or_else(|| s3_refusal(violation));
                    if let Some(response) = self.detailed_refusal(&refused, detail) {
                        return response;
                    }
                }
            }
        }
        self.entry.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
    }
}
