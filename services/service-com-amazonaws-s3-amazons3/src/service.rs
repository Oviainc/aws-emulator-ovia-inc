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
//!   `ValidationException`, because a client branches on the S3 code — and a request
//!   whose SIGNATURE did not verify is refused in AWS's own words (see
//!   `signature_refusal`).
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
use std::cell::RefCell;
use std::sync::Arc;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::protocol::ServerProtocol;

use wire::UnwrappedXmlRestXml;

/// The error code AWS answers a request whose access key id DOES resolve but whose
/// signature does not verify under that credential's secret: the credential was found, so
/// the caller is known; what failed is the signature.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// How the SigV4 signing layer words that refusal for this service — the fixed sentence it
/// produces for this one condition, which tells the caller to look at the key and the
/// signing method rather than at the request.
const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check \
     your key and signing method.";

thread_local! {
    /// The credential the request this service is serving presented, for the length of that
    /// one request.
    ///
    /// Not state: a request-scoped note, needed because the two seams that decide an
    /// authentication refusal see different halves of it. [`Service::can_handle`] is handed
    /// the request and records it here; [`Service::override_refusal`] is handed only the
    /// STRUCTURAL refusal — `Refusal::Unauthenticated`, which says that authentication
    /// refused the caller and nothing about WHICH condition refused it — and has to tell a
    /// signature that did not verify from the conditions AWS words differently. The
    /// framework calls both within one synchronous stretch of the one dispatch, on the
    /// thread that dispatch is running on, so the value a refusal reads is the one the claim
    /// of that same request recorded.
    static PRESENTED_CREDENTIAL: RefCell<Option<PresentedCredential>> =
        const { RefCell::new(None) };
}

/// What a refusal needs to know about the credential a request presented.
struct PresentedCredential {
    /// The access key id it named, however it travelled ([`presented_access_key_id`]).
    access_key_id: String,
    /// Whether it is a PRESIGNED request whose validity window has already passed
    /// ([`presigned_window_passed`]).
    presigned_window_passed: bool,
}

/// Record the credential `parts` presented as the one this thread is serving.
fn remember_presented_credential(parts: &http::request::Parts) {
    let presented = PresentedCredential {
        access_key_id: presented_access_key_id(parts),
        presigned_window_passed: presigned_window_passed(parts),
    };
    PRESENTED_CREDENTIAL.with(|recorded| {
        *recorded.borrow_mut() = Some(presented);
    });
}

/// The access key id `parts` presented, however it travelled.
///
/// A signed request carries it in the `Credential` field of its `Authorization` header, and
/// a PRESIGNED one in the `X-Amz-Credential` query parameter instead —
/// "`X-Amz-Credential`: … `<your-access-key-id>/<date>/<AWS Region>/<AWS
/// service>/aws4_request`" (AWS General Reference, *Authenticating requests: using query
/// parameters*) — so both forms are read, and the key id is the scope's first element in
/// either.
fn presented_access_key_id(parts: &http::request::Parts) -> String {
    if parts.headers.contains_key(http::header::AUTHORIZATION) {
        return ::engine::extract_akid(&parts.headers);
    }
    presigned_parameter(parts, PRESIGNED_CREDENTIAL_PARAMETER)
        .and_then(|scope| {
            scope
                .split('/')
                .next()
                .filter(|key_id| !key_id.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// The value of the presigned query parameter `name`, percent-decoded — a presigned URL
/// encodes the separators of the values it carries.
fn presigned_parameter(parts: &http::request::Parts, name: &str) -> Option<String> {
    let decode = |text: &str| {
        ::percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    };
    parts
        .uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(parameter, _)| decode(parameter) == name)
        .map(|(_, value)| decode(value))
}

/// Whether `parts` is a PRESIGNED request whose validity window has already passed.
///
/// A presigned URL carries the instant it was signed at (`X-Amz-Date`) and how long it is
/// valid for (`X-Amz-Expires`, in seconds, at most [`MAX_PRESIGNED_EXPIRES_SECS`]), and AWS
/// refuses one past that window for the EXPIRY — not for the signature, which is intact.
/// The framework's authentication refuses both conditions through the same structural
/// refusal, so this is what keeps an expired URL from being re-worded as a signature
/// mismatch. A request that is not presigned, or whose window cannot be read, is not an
/// expired one.
fn presigned_window_passed(parts: &http::request::Parts) -> bool {
    let signed_at = presigned_parameter(parts, PRESIGNED_DATE_PARAMETER)
        .as_deref()
        .and_then(amz_instant);
    let expires_secs = presigned_parameter(parts, PRESIGNED_EXPIRES_PARAMETER)
        .and_then(|value| value.trim().parse::<u64>().ok());
    let (Some(signed_at), Some(expires_secs)) = (signed_at, expires_secs) else {
        return false;
    };
    if expires_secs > MAX_PRESIGNED_EXPIRES_SECS {
        return true;
    }
    signed_at
        .checked_add(::std::time::Duration::from_secs(expires_secs))
        .is_none_or(|valid_until| valid_until < ::std::time::SystemTime::now())
}

/// The instant the SigV4 stamp `YYYYMMDDTHHMMSSZ` names.
///
/// Parsed by the library's own RFC 3339 reader rather than by any calendar arithmetic
/// written here: SigV4's basic form differs from RFC 3339 only in the separators it omits,
/// so they are put back and the timestamp type reads it. `None` for anything that is not
/// that form, which is why the digit check precedes the split.
fn amz_instant(stamp: &str) -> Option<::std::time::SystemTime> {
    let (date, time) = stamp.strip_suffix('Z')?.split_once('T')?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    if !date.chars().chain(time.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rfc3339 = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &date[..4],
        &date[4..6],
        &date[6..],
        &time[..2],
        &time[2..4],
        &time[4..]
    );
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &rfc3339,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    ::std::time::SystemTime::try_from(parsed).ok()
}

/// The query parameter a presigned URL carries its credential scope in.
const PRESIGNED_CREDENTIAL_PARAMETER: &str = "X-Amz-Credential";

/// The query parameter a presigned URL carries its signing instant in.
const PRESIGNED_DATE_PARAMETER: &str = "X-Amz-Date";

/// The query parameter a presigned URL carries its validity, in seconds, in.
const PRESIGNED_EXPIRES_PARAMETER: &str = "X-Amz-Expires";

/// The longest validity AWS accepts for a presigned URL: seven days (AWS General Reference,
/// *Authenticating requests: using query parameters* — "the maximum expiration time is one
/// week"). A URL asking for more is outside the window from the moment it is made.
const MAX_PRESIGNED_EXPIRES_SECS: u64 = 7 * 24 * 60 * 60;

/// The credential recorded by [`remember_presented_credential`] for the request this thread
/// is serving, TAKEN: the note is request-scoped, so reading it is what ends it. A later
/// refusal on this thread that was not preceded by this service's own claim then reads no
/// credential at all rather than the previous request's.
fn presented_credential() -> Option<PresentedCredential> {
    PRESENTED_CREDENTIAL.with(|presented| presented.borrow_mut().take())
}

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
fn bucket_name_shaped(label: &str) -> bool {
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
            engine,
            protocol: UnwrappedXmlRestXml::new().with_buckets(holds_bucket),
        }
    }

    /// Whether `name` is a bucket this emulator holds — the question an ADDRESS resolves
    /// against (see [`addressed_in_host`]). Read-only: the guard is taken, the answer is
    /// read, and nothing is written.
    fn holds_bucket(&self, name: &str) -> bool {
        holds_bucket_in(&*self.engine, name)
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

    /// How this service words a request whose SIGNATURE did not verify, or `None` for an
    /// authentication refusal that is not that one.
    ///
    /// The framework's structural refusal says only that authentication refused the caller,
    /// so which refusal it is follows from what the request presented. A key id this
    /// emulator ISSUED resolves to a principal, so the only thing left for authentication to
    /// have refused is the signature computed under that credential's secret — the condition
    /// AWS answers with [`SIGNATURE_MISMATCH_CODE`] and the fixed sentence above, which a
    /// CLI, an SDK and a signing runbook all match on.
    ///
    /// Two cases keep the framework's own wording, because neither is a signature that did
    /// not verify: a key id that resolves to nothing (the credential was refused, not the
    /// signature), and a presigned URL whose validity window has passed — AWS refuses that
    /// one for the EXPIRY, with its signature intact.
    fn signature_refusal(&self) -> Option<Refused> {
        let presented = presented_credential()?;
        if presented.presigned_window_passed {
            return None;
        }
        self.issued_credential(&presented.access_key_id)
            .then(|| Refused {
                code: SIGNATURE_MISMATCH_CODE.to_string(),
                message: SIGNATURE_MISMATCH_MESSAGE.to_string(),
            })
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
            None => carries_legacy_presigned_credential(&parts.uri),
        };
        if claimed {
            // The credential this request presented, kept for the length of the request so
            // an authentication refusal can be worded for the condition that refused it;
            // see [`PRESENTED_CREDENTIAL`].
            remember_presented_credential(parts);
        }
        claimed
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
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
        // `None` for every path-style request, which leaves the generated routing table to
        // resolve it exactly as before.
        let host = request_host_of(req)?;
        let bucket = addressed_in_host(host, wire::request_path(req.uri()), &|name| {
            self.holds_bucket(name)
        })?;
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
        self.entry.routing_table().get(&operation)
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
            // A caller whose SIGNATURE did not verify is told so in AWS's own words
            // ([`Self::signature_refusal`]); every other authentication refusal, and any
            // refusal kind added later, keeps the framework's wording.
            Refusal::Unauthenticated => self.signature_refusal(),
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
