// SPDX-License-Identifier: CC0-1.0

//! MediaLive's error envelope: the members every error body this service answers carries
//! beside `message`, and the request-id header every response carries.
//!
//! A failed MediaLive call is answered with an error document that names the failure and
//! the response it belongs to:
//!
//! ```json
//! {
//!   "message": "Name already in use",
//!   "code": "ConflictException",
//!   "statusCode": 409,
//!   "retryable": false,
//!   "requestId": "7f9c1e2a-0b3d-4c5e-8a7b-6d5e4f3a2b1c",
//!   "time": "2026-10-22T17:11:17.412Z"
//! }
//! ```
//!
//! Only `message` belongs to a modelled shape: every exception this service's model
//! declares carries `Message` (jsonName `message`) and nothing else —
//! `smithy_query(query="shape",shape="ConflictException",model_path="services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json")`.
//! The other five members belong to the RESPONSE, not to the error value, so the
//! framework's restJson1 renderer — which writes `__type` plus the error shape's own
//! members and stops — omits all five. Two of them, `requestId` and `time`, no error shape
//! could carry: they identify the response rather than the failure.
//!
//! This module therefore completes that envelope on every response this service answers,
//! through [`ErrorEnvelopeOperation`] — the wrapper the service hands back from
//! `Service::override_operation`. It serves the modelled operation the generated routing
//! table names for the request, so the framework still owns the whole
//! deserialize → validate → handle → serialize sequence and every status, code and message
//! it answers with, and then
//!
//! * stamps the response's request id in the header AWS reports it in, on a success as
//!   well as on a failure — it is the handle a caller correlates ANY call by; and
//! * adds the envelope members to an error body that names one of this service's own
//!   modelled exceptions, whichever layer produced it.
//!
//! Every value follows the request or the response, never a recording:
//!
//! * `code` is the error code the response already names (`__type` in the body, which is
//!   also the `x-amzn-errortype` header restJson1 resolves the shape from);
//! * `statusCode` is that response's own HTTP status, repeated in the body;
//! * `retryable` says whether a client may retry the same call — true for a server fault
//!   and for a throttle, false for every other client failure (see [`retryable`]);
//! * `requestId` is the id the response reports in its request-id header, minted per
//!   response when it reports none — it names ONE request for correlation, so two
//!   responses must be distinguishable;
//! * `time` is the instant this response was produced, in the `date-time` form the model
//!   uses for every timestamp it declares.
//!
//! The modelled `message` is left to the framework with ONE exception: where the error
//! value carries none, the key is reported as an explicit JSON `null` rather than dropped,
//! because a MediaLive error document always carries the member (see [`complete_envelope`]).
//! A message the error value DID state is never touched. What grounds that claim is the
//! EventBridge rule-template hierarchy's recorded 404s, the only refusals of this service
//! that state no message; it is written here for every modelled exception because the key's
//! presence is a property of the document rather than of one family, and it must be
//! re-grounded if another family's recording ever shows the member dropped instead.
//!
//! What is deliberately left alone: the HTTP status, `__type`, the success body
//! and every other header stay with the framework renderer that owns them; and a body
//! whose code is NOT one of this service's modelled exceptions — the scaffold's 501
//! `NotImplemented`, a framework `SerializationException` — is left exactly as the
//! framework wrote it rather than dressed up as a MediaLive exception.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::date_time::Format;
use serde_json::{Map, Value};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Every exception shape this service's model declares, with the HTTP status and the
/// `errorKind` it declares — the error codes a MediaLive error body can name.
///
/// Source: the `errors` lists in `services/service-com-amazonaws-medialive-medialive/OPERATIONS.md`,
/// which is this model's own rendering, cross-checked with
/// `smithy_query(query="shape",shape="BadGatewayException,BadRequestException,ConflictException,ForbiddenException,GatewayTimeoutException,InternalServerErrorException,NotFoundException,TooManyRequestsException,UnprocessableEntityException",model_path="services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json")`.
/// These nine are the whole set: no operation of this service declares any other error.
///
/// The envelope accompanies one of THESE. A code the framework synthesises for a signal
/// MediaLive has no error for — the scaffold's `NotImplemented` (501), answered for an
/// operation nobody has written yet — is not a MediaLive exception, so its body is left
/// exactly as the framework wrote it.
pub const MODELLED_EXCEPTIONS: [&str; 9] = [
    "BadGatewayException",
    "BadRequestException",
    "ConflictException",
    "ForbiddenException",
    "GatewayTimeoutException",
    "InternalServerErrorException",
    "NotFoundException",
    "TooManyRequestsException",
    "UnprocessableEntityException",
];

/// The response header restJson1 resolves an error shape from, and the fallback source of
/// the error code when a body carries no `__type`.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The response headers a request id is reported in, in the order they are consulted: the
/// JSON families' spelling first, then the one the XML families use. The first is the one
/// a response of this service is stamped with.
///
/// The envelope's `requestId` and the header name the SAME request, so when the response
/// already reports one, that is the id the body carries.
const REQUEST_ID_HEADERS: [&str; 2] = ["x-amzn-requestid", "x-amz-request-id"];

/// The lowest HTTP status that is a failure, and so the lowest one that carries an error
/// body at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// The lowest HTTP status that is a server fault.
const FIRST_SERVER_FAULT_STATUS: u16 = 500;

/// The status a throttled call is refused with (`TooManyRequestsException`, HTTP 429 in
/// this model).
const THROTTLED_STATUS: u16 = 429;

/// Whether a client may retry the same call after a failure answered with `status`.
///
/// A server fault (5xx) and a throttle are transient: the same request may succeed later,
/// and this is the classification every AWS SDK's default retry policy applies. Every
/// other client failure — a conflict, a missing resource, a malformed request — is
/// permanent for that request, so retrying it cannot help.
fn retryable(status: u16) -> bool {
    status >= FIRST_SERVER_FAULT_STATUS || status == THROTTLED_STATUS
}

/// The error code a response names, reduced to the bare shape name.
///
/// Read from the body's `__type` first, since that is what the error renderers write, and
/// from the `x-amzn-errortype` header otherwise. Either source may carry a namespace
/// (`com.amazonaws.medialive#ConflictException`) or a trailing locator
/// (`ConflictException:http://…`), neither of which is part of the code.
fn error_code(object: &Map<String, Value>, header: Option<&str>) -> Option<String> {
    let raw = object
        .get("__type")
        .and_then(Value::as_str)
        .or(header)?
        .trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    let code = after_namespace.split(':').next().unwrap_or(after_namespace);
    (!code.is_empty()).then(|| code.to_string())
}

/// Add `member` unless the body already carries it, and answer whether it was added.
///
/// A member already in the body is left as it is, so completing an envelope is idempotent
/// and a modelled member of the error shape always wins over the envelope's default for it.
fn insert_absent(object: &mut Map<String, Value>, member: &str, value: Value) -> usize {
    if object.contains_key(member) {
        return 0;
    }
    object.insert(member.to_string(), value);
    1
}

/// Set `member` from the RESPONSE, whatever the body said, and answer whether that
/// changed the document.
///
/// For the two members that are properties of the response itself — the status it was
/// answered with and the id of the request it answers — the response is the authority:
/// a body claiming another value would contradict the status line and the request-id
/// header, and the envelope's contract is that the two cannot disagree.
fn set_from_response(object: &mut Map<String, Value>, member: &str, value: Value) -> usize {
    if object.get(member) == Some(&value) {
        return 0;
    }
    object.insert(member.to_string(), value);
    1
}

/// Add the envelope members this body does not already carry, set the two the response
/// itself is the authority for, and answer how many of them changed the document.
///
/// `message` is the modelled member and is NEVER overwritten here — whatever the error
/// value carried survives. It is only ADDED, as an explicit JSON `null`, when the error
/// value carried none: a MediaLive error document always has the `message` key, and
/// reports "this failure states no message" as that key's null rather than by dropping
/// it. The restJson1 renderer omits an unset optional member, so this is the one place
/// the difference can be written, and it is a real one to a client: a reader of the
/// member gets a null it can branch on instead of a missing key.
fn complete_envelope(
    object: &mut Map<String, Value>,
    status: u16,
    code: &str,
    request_id: &str,
    time: &str,
) -> usize {
    let mut added = 0;
    added += insert_absent(object, "message", Value::Null);
    added += insert_absent(object, "code", Value::from(code));
    added += set_from_response(object, "statusCode", Value::from(status));
    added += insert_absent(object, "retryable", Value::from(retryable(status)));
    added += set_from_response(object, "requestId", Value::from(request_id));
    added += insert_absent(object, "time", Value::from(time));
    added
}

/// The error body with its envelope completed, or `None` when there is nothing to add:
/// the body is not a JSON object, it names no error code, the code is not one of this
/// service's modelled exceptions, or every envelope member is already there.
fn with_envelope_members(
    body: &[u8],
    status: u16,
    header_code: Option<&str>,
    request_id: &str,
    time: &str,
) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let code = error_code(object, header_code)?;
    if !MODELLED_EXCEPTIONS.contains(&code.as_str()) {
        return None;
    }
    if complete_envelope(object, status, &code, request_id, time) == 0 {
        return None;
    }
    serde_json::to_vec(&document).ok()
}

/// The instant `at`, in the `date-time` form (ISO-8601 / RFC-3339, UTC) this model uses
/// for every timestamp it declares — `__timestampIso8601` carries
/// `@timestampFormat("date-time")`.
fn response_time(at: ::std::time::SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(Format::DateTime)
        // `fmt` can only fail for an instant outside the formattable range; a response
        // still reports one rather than carrying a half-built envelope, so the epoch is
        // the fallback no reachable clock reading produces.
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The id the response reports, stamped into the header a restJson1 client reads it from.
///
/// Every AWS response names the request it answers, on a success as well as on a failure:
/// it is the handle a caller correlates a call by, and the envelope's `requestId` reports
/// the same value as the header so the two cannot disagree. An id another layer already
/// reported is kept, under either spelling; the canonical JSON-family header always ends
/// up carrying it, because that is the one an SDK of this restJson1 service reads into
/// `$metadata.requestId`.
fn stamped_request_id(resp: &mut RtResponse) -> String {
    let reported = REQUEST_ID_HEADERS
        .iter()
        .find_map(|header| resp.headers().get(header))
        .map(str::to_string);
    let id = reported.unwrap_or_else(new_request_id);
    if resp.headers().get(REQUEST_ID_HEADERS[0]).is_none() {
        // Dropped rather than fatal if the HTTP layer cannot carry it: the body still
        // states the id, and a header is not worth failing a modelled error for.
        let _ = resp
            .headers_mut()
            .try_insert(REQUEST_ID_HEADERS[0], id.clone());
    }
    id
}

/// The response with its request id stamped and, when it is a failure this service owns,
/// MediaLive's envelope completed in its error body.
///
/// A response that is not a failure keeps every byte of its body; so does a failure whose
/// body is not an error body this service owns (see [`with_envelope_members`]).
pub fn with_request_id_and_envelope(mut resp: RtResponse) -> RtResponse {
    let request_id = stamped_request_id(&mut resp);
    let status = resp.status().as_u16();
    if status < FIRST_FAILURE_STATUS {
        return resp;
    }
    let time = response_time(::std::time::SystemTime::now());
    let header_code = resp.headers().get(ERROR_TYPE_HEADER).map(str::to_string);
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_envelope_members(
            bytes,
            status,
            header_code.as_deref(),
            &request_id,
            &time,
        ) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert("content-length", length);
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// Mint the request id one response carries, when the response reports none of its own.
///
/// UUID-SHAPED: the same 8-4-4-4-12 lowercase-hex form as the request ids AWS reports, not
/// a version-tagged UUIDv4 — nothing reads the version or variant bits. Fresh per
/// response: the id names ONE request for correlation, so two responses must be
/// distinguishable. It is an identifier and not a secret — nothing authenticates with it —
/// so a digest of a per-process counter and the current instant is all it needs, which the
/// `sha2`/`hex` crates provide (no crate in this workspace's dependency set generates
/// UUIDs, and this is the same minting the sibling Elemental service uses). The counter
/// alone makes two ids minted in the same nanosecond differ.
fn new_request_id() -> String {
    use ::sha2::{Digest, Sha256};

    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = Sha256::new();
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = ::hex::encode(hasher.finalize());

    // Every slice is taken with `get`, so a shorter digest could never panic here.
    [(0, 8), (8, 12), (12, 16), (16, 20), (20, 32)]
        .iter()
        .map(|&(from, to)| digest.get(from..to).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ErrorEnvelopeOperation`].
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then completes the request id and error envelope of the response.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// code and message it answers with; this wrapper owns the envelope's five members and the
/// request-id header.
pub struct ErrorEnvelopeOperation;

impl ErasedOperation for ErrorEnvelopeOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            let modeled = definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key));
            match modeled {
                Some(op) => with_request_id_and_envelope(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => definition.routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request id and a response instant, fixed so an assertion reads the member the
    /// envelope wrote rather than a clock.
    const ID: &str = "11111111-2222-3333-4444-555555555555";
    const TIME: &str = "2026-10-22T17:11:17.412Z";

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: Vec<u8>) -> Map<String, Value> {
        serde_json::from_slice::<Value>(&bytes)
            .expect("the rewritten body is JSON")
            .as_object()
            .expect("the rewritten body is a JSON object")
            .clone()
    }

    /// One modelled exception per row — every code the model declares, at the status it
    /// declares — so the envelope is not keyed to the one the issue recorded. The
    /// `retryable` column is the rule's own answer for that status: a server fault and a
    /// throttle may be retried, nothing else may.
    #[test]
    fn every_modelled_exception_gets_the_envelope() {
        for (code, status, expected_retryable) in [
            ("BadRequestException", 400u16, false),
            ("ForbiddenException", 403, false),
            ("NotFoundException", 404, false),
            ("ConflictException", 409, false),
            ("UnprocessableEntityException", 422, false),
            ("TooManyRequestsException", 429, true),
            ("InternalServerErrorException", 500, true),
            ("BadGatewayException", 502, true),
            ("GatewayTimeoutException", 504, true),
        ] {
            let body = format!(r#"{{"__type":"{code}","message":"no"}}"#);
            let rewritten = with_envelope_members(body.as_bytes(), status, None, ID, TIME)
                .unwrap_or_else(|| panic!("case {code:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get("code").and_then(Value::as_str),
                Some(code),
                "case {code:?}: expected code {code:?}, got {object:?}"
            );
            assert_eq!(
                object.get("statusCode").and_then(Value::as_u64),
                Some(u64::from(status)),
                "case {code:?}: expected statusCode {status}, got {object:?}"
            );
            assert_eq!(
                object.get("retryable").and_then(Value::as_bool),
                Some(expected_retryable),
                "case {code:?}: expected retryable {expected_retryable}, got {object:?}"
            );
            assert_eq!(
                object.get("requestId").and_then(Value::as_str),
                Some(ID),
                "case {code:?}: expected the response's request id, got {object:?}"
            );
            assert_eq!(
                object.get("time").and_then(Value::as_str),
                Some(TIME),
                "case {code:?}: expected the response instant, got {object:?}"
            );
            assert_eq!(
                object.get("message").and_then(Value::as_str),
                Some("no"),
                "case {code:?}: the message must survive, got {object:?}"
            );
        }
    }

    /// The code may arrive namespaced, or with a trailing locator, or only in the
    /// `x-amzn-errortype` header — every form names the same shape.
    #[test]
    fn the_error_code_is_read_whichever_form_it_arrives_in() {
        for (label, body, header) in [
            ("bare __type", r#"{"__type":"ConflictException"}"#, None),
            (
                "namespaced __type",
                r#"{"__type":"com.amazonaws.medialive#ConflictException"}"#,
                None,
            ),
            (
                "__type with a locator",
                r#"{"__type":"ConflictException:http://internal.amazon.com/coral/"}"#,
                None,
            ),
            (
                "header only",
                r#"{"message":"no"}"#,
                Some("ConflictException"),
            ),
        ] {
            let rewritten = with_envelope_members(body.as_bytes(), 409, header, ID, TIME)
                .unwrap_or_else(|| panic!("case {label:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get("code").and_then(Value::as_str),
                Some("ConflictException"),
                "case {label:?}: expected the bare shape name, got {object:?}"
            );
        }
    }

    /// The modelled `message` is reported either way: a message the error value stated
    /// survives verbatim, and an error value that stated none reports the member as an
    /// explicit null rather than dropping it.
    ///
    /// The rows span what the rule generalises over: whether the body arrived with the
    /// member, the value it arrived with (a sentence, and the empty string — a stated
    /// message that happens to be empty is NOT the same as none), and the modelled code
    /// the body names.
    #[test]
    fn a_message_the_error_stated_survives_and_an_absent_one_is_reported_as_null() {
        // (case, the body the renderer wrote, the value the member must end up with)
        let table: [(&str, String, Value); 5] = [
            (
                "a stated message",
                r#"{"__type":"ConflictException","message":"Name already in use"}"#.to_string(),
                Value::from("Name already in use"),
            ),
            (
                "a stated empty message",
                r#"{"__type":"ConflictException","message":""}"#.to_string(),
                Value::from(""),
            ),
            (
                "no member at all",
                r#"{"__type":"NotFoundException"}"#.to_string(),
                Value::Null,
            ),
            (
                "the member already null",
                r#"{"__type":"NotFoundException","message":null}"#.to_string(),
                Value::Null,
            ),
            (
                "no member, code from the header",
                r#"{}"#.to_string(),
                Value::Null,
            ),
        ];

        for (case, body, expected) in table {
            let header = (case == "no member, code from the header").then_some("NotFoundException");
            let rewritten = with_envelope_members(body.as_bytes(), 404, header, ID, TIME)
                .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get("message"),
                Some(&expected),
                "case {case:?}: expected the message member to be {expected:?}, got {object:?}"
            );
        }
    }

    /// Nothing to add: a body the framework synthesised for a signal MediaLive has no
    /// error for, a body carrying no code at all, one that is not a JSON object, and one
    /// that already carries the envelope, are all left alone.
    #[test]
    fn nothing_is_added_to_a_body_this_service_does_not_own() {
        let already = format!(
            r#"{{"__type":"ConflictException","message":"no","code":"ConflictException","statusCode":409,"retryable":false,"requestId":"{ID}","time":"{TIME}"}}"#
        );
        for (label, body) in [
            (
                "the scaffold's 501",
                r#"{"__type":"NotImplemented","message":"no"}"#,
            ),
            (
                "a framework code",
                r#"{"__type":"SerializationException","message":"no"}"#,
            ),
            ("no code at all", r#"{"message":"no"}"#),
            ("not an object", r#"["ConflictException"]"#),
            ("not JSON", "<Error/>"),
            ("already enveloped", already.as_str()),
        ] {
            assert!(
                with_envelope_members(body.as_bytes(), 409, None, ID, TIME).is_none(),
                "case {label:?}: expected no rewrite"
            );
        }
    }

    /// The response is the authority for the two members that describe it — the status
    /// it was answered with and the id of the request it answers — so a body claiming
    /// another value is corrected rather than kept; a member that describes the FAILURE
    /// is completed, never overwritten.
    #[test]
    fn the_response_decides_its_own_status_and_request_id() {
        let body = r#"{"__type":"NotFoundException","requestId":"stale","statusCode":499,"retryable":true}"#;
        let object = decoded(
            with_envelope_members(body.as_bytes(), 404, None, ID, TIME).expect("a rewrite"),
        );
        assert_eq!(
            object.get("requestId").and_then(Value::as_str),
            Some(ID),
            "the response's own requestId must win, got {object:?}"
        );
        assert_eq!(
            object.get("statusCode").and_then(Value::as_u64),
            Some(404),
            "the response's own status must win, got {object:?}"
        );
        assert_eq!(
            object.get("retryable").and_then(Value::as_bool),
            Some(true),
            "a member describing the failure is completed, not overwritten, got {object:?}"
        );
    }

    /// A success is stamped with a request id and keeps its body byte for byte.
    #[test]
    fn a_success_keeps_its_body_and_still_reports_a_request_id() {
        let body = r#"{"id":"1000000","name":"group"}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_request_id_and_envelope(resp);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
        assert!(
            answered
                .headers()
                .get(REQUEST_ID_HEADERS[0])
                .is_some_and(|id| !id.is_empty()),
            "a success must still report a request id"
        );
    }

    /// The id the response reports in its request-id header is the id the body carries, so
    /// the two name the same request; one is minted only when the response reports none.
    #[test]
    fn the_request_id_follows_the_response_when_the_response_reports_one() {
        let body = r#"{"__type":"NotFoundException","message":"no"}"#;
        for header in REQUEST_ID_HEADERS {
            let mut resp = RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(404u16).expect("404"),
                SdkBody::from(body),
            );
            resp.headers_mut().insert(header, "reported-by-the-response");
            let answered = with_request_id_and_envelope(resp);
            let object = decoded(answered.body().bytes().expect("bytes").to_vec());
            assert_eq!(
                object.get("requestId").and_then(Value::as_str),
                Some("reported-by-the-response"),
                "case {header:?}: the body must carry the reported id, got {object:?}"
            );
            // Whichever spelling reported it, the header a restJson1 client reads
            // carries that same id.
            assert_eq!(
                answered.headers().get(REQUEST_ID_HEADERS[0]),
                Some("reported-by-the-response"),
                "case {header:?}: expected {:?} to carry the reported id, got {:?}",
                REQUEST_ID_HEADERS[0],
                answered.headers().get(REQUEST_ID_HEADERS[0])
            );
        }

        // No header: the body carries one, and the header reports the same value.
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(404u16).expect("404"),
            SdkBody::from(body),
        );
        let answered = with_request_id_and_envelope(resp);
        let stamped = answered
            .headers()
            .get(REQUEST_ID_HEADERS[0])
            .expect("a minted request id")
            .to_string();
        let object = decoded(answered.body().bytes().expect("bytes").to_vec());
        assert_eq!(
            object.get("requestId").and_then(Value::as_str),
            Some(stamped.as_str()),
            "case no header: the body and the header must name one request, got {object:?}"
        );
    }

    /// Two request ids are distinct and both carry the 8-4-4-4-12 lowercase-hex form.
    #[test]
    fn request_ids_are_distinct_and_well_formed() {
        let ids = [new_request_id(), new_request_id()];
        assert_ne!(
            ids[0], ids[1],
            "two minted request ids must differ, got {ids:?}"
        );
        for id in &ids {
            let widths: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                widths,
                vec![8, 4, 4, 4, 12],
                "case {id:?}: expected the 8-4-4-4-12 form, got widths {widths:?}"
            );
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "case {id:?}: expected lowercase hex and hyphens"
            );
        }
    }

    /// The response instant is rendered in the `date-time` form, not as the epoch-seconds
    /// number the JSON codec falls back to for a timestamp member.
    #[test]
    fn the_response_time_is_an_iso8601_date_time() {
        for (label, at) in [
            ("the epoch", ::std::time::UNIX_EPOCH),
            (
                "a second past the epoch",
                ::std::time::UNIX_EPOCH + ::std::time::Duration::from_secs(1),
            ),
            ("now", ::std::time::SystemTime::now()),
        ] {
            let rendered = response_time(at);
            assert!(
                rendered.ends_with('Z') && rendered.contains('T') && rendered.len() >= 20,
                "case {label:?}: expected an ISO-8601 UTC date-time, got {rendered:?}"
            );
            assert!(
                ::aws_smithy_types::DateTime::from_str(&rendered, Format::DateTime).is_ok(),
                "case {label:?}: expected a parseable date-time, got {rendered:?}"
            );
        }
    }
}
