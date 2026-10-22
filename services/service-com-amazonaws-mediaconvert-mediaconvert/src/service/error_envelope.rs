// SPDX-License-Identifier: CC0-1.0

//! MediaConvert's error envelope: the members every error body this service answers
//! carries beside `message`.
//!
//! AWS answers a failed MediaConvert call with
//!
//! ```json
//! {
//!   "message": "The resource you specified in id was not found.",
//!   "errorType": "NotFoundException",
//!   "httpStatus": 404,
//!   "accountId": "000000000000",
//!   "requestId": "7f9c1e2a-0b3d-4c5e-8a7b-6d5e4f3a2b1c",
//!   "operation": "GetJob",
//!   "settingsValidationErrorsJsonBlob": ""
//! }
//! ```
//!
//! Only `message` belongs to a modelled shape: every exception this service's model
//! declares carries `Message` (jsonName `message`) and nothing else —
//! `smithy_query(query="shape",shape="NotFoundException",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`.
//! The other six members belong to the ENVELOPE the service wraps an exception in, so the
//! framework's restJson1 renderer — which writes `__type` plus the error shape's own
//! members and stops — omits all six. One of them, `operation`, no error shape could
//! carry: it names the operation being SERVED, which the error value does not know.
//!
//! This module therefore completes the envelope where that operation is known.
//! [`ErrorEnvelopeOperation`] is the wrapper this service hands back from
//! `Service::override_operation`: it serves the modelled operation the generated routing
//! table names for the request — the framework still owns the whole
//! deserialize → validate → handle → serialize sequence, and every status, code and
//! message it answers with — and then adds the envelope members to the error body that
//! came back, whichever layer produced it:
//!
//! * a handler-returned modelled exception, and
//! * a framework refusal raised while that operation was being served (an absent required
//!   member, a value outside a declared bound), which `Service::override_refusal` already
//!   states as one of this service's own modelled exceptions.
//!
//! Every value follows the request or the response, never a recording:
//!
//! * `errorType` is the error code the response already names (`__type` in the body, which
//!   is also the `x-amzn-errortype` header restJson1 resolves the shape from);
//! * `httpStatus` is that response's own HTTP status, repeated in the body;
//! * `operation` is the operation the routing key named;
//! * `accountId` is the emulator's account, the one every ARN this service reports carries;
//! * `requestId` is the id the response itself reports in its request-id header, and is
//!   minted per response when it reports none — it names ONE request for correlation, so
//!   two responses must be distinguishable;
//! * `settingsValidationErrorsJsonBlob` is the settings-validation report, empty for a
//!   failure that is not one (see [`NO_SETTINGS_VALIDATION_ERRORS`]).
//!
//! What is deliberately left alone: the HTTP status, `__type`, `message`, every header and
//! the whole success path stay with the framework renderer that owns them, and a failure
//! answered before any operation is served — a request naming no modelled operation, a
//! transport-level failure, a pre-dispatch authentication refusal — keeps the framework's
//! rendering, because the envelope's `operation` member has no value there.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::{Map, Value};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Every exception shape this service's model declares — the error codes a MediaConvert
/// error body can name.
///
/// Source: the `errors` list every operation in this service carries, e.g.
/// `smithy_query(query="operation",operation="GetJob",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
/// — `BadRequestException` (400), `ConflictException` (409), `ForbiddenException` (403),
/// `InternalServerErrorException` (500), `NotFoundException` (404),
/// `ServiceQuotaExceededException` (402), `TooManyRequestsException` (429), the same seven
/// on every operation.
///
/// The envelope accompanies one of THESE. A code the framework synthesises for a signal
/// MediaConvert has no error for — the scaffold's `NotImplemented` (501), answered for an
/// operation nobody has written yet — is not a MediaConvert exception, so its body is left
/// exactly as the framework wrote it rather than dressed up as one.
const MODELLED_EXCEPTIONS: [&str; 7] = [
    "BadRequestException",
    "ConflictException",
    "ForbiddenException",
    "InternalServerErrorException",
    "NotFoundException",
    "ServiceQuotaExceededException",
    "TooManyRequestsException",
];

/// `settingsValidationErrorsJsonBlob` for a failure that is not a settings-validation
/// failure.
///
/// AWS carries the validation report in this member when it rejects a job's `Settings`,
/// and an empty string otherwise. This emulator runs no settings validation — nothing in
/// `src/ops/` rejects a request for the content of `Settings` — so every error it answers
/// is the "otherwise" case.
const NO_SETTINGS_VALIDATION_ERRORS: &str = "";

/// The response header restJson1 resolves an error shape from, and the fallback source of
/// the error code when a body carries no `__type`.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The response headers a request id is reported in, in the order they are consulted: the
/// JSON families' spelling first, then the one the XML families use.
///
/// The envelope's `requestId` and the header name the SAME request in real MediaConvert, so
/// when the response already reports one, that is the id the body carries; a fresh one is
/// minted only when it reports none.
const REQUEST_ID_HEADERS: [&str; 2] = ["x-amzn-requestid", "x-amz-request-id"];

/// The error code a response names, reduced to the bare shape name.
///
/// Read from the body's `__type` first, since that is what the error renderers write, and
/// from the `x-amzn-errortype` header otherwise. Either source may carry a namespace
/// (`com.amazonaws.mediaconvert#NotFoundException`) or a trailing locator
/// (`NotFoundException:http://…`), neither of which is part of the code.
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

/// Add the envelope members this body does not already carry, and answer how many were
/// added.
///
/// `reported_request_id` is the id the response itself reports (see
/// [`REQUEST_ID_HEADERS`]); one is minted only when the response reports none AND the body
/// needs one, so no digest is computed for a value that would be discarded.
fn complete_envelope(
    object: &mut Map<String, Value>,
    status: u16,
    code: &str,
    operation: &str,
    reported_request_id: Option<&str>,
) -> usize {
    let mut added = 0;
    added += insert_absent(object, "errorType", Value::from(code));
    added += insert_absent(object, "httpStatus", Value::from(status));
    added += insert_absent(
        object,
        "accountId",
        Value::from(::engine::emulator_account_id()),
    );
    added += insert_absent(object, "operation", Value::from(operation));
    added += insert_absent(
        object,
        "settingsValidationErrorsJsonBlob",
        Value::from(NO_SETTINGS_VALIDATION_ERRORS),
    );
    if !object.contains_key("requestId") {
        let id = reported_request_id
            .map(str::to_string)
            .unwrap_or_else(new_request_id);
        added += insert_absent(object, "requestId", Value::from(id));
    }
    added
}

/// The error body with its envelope completed, or `None` when there is nothing to add:
/// the body is not a JSON object, it names no error code, the code is not one of this
/// service's modelled exceptions, or every envelope member is already there.
fn with_envelope_members(
    body: &[u8],
    status: u16,
    header_code: Option<&str>,
    operation: &str,
    reported_request_id: Option<&str>,
) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let code = error_code(object, header_code)?;
    if !MODELLED_EXCEPTIONS.contains(&code.as_str()) {
        return None;
    }
    if complete_envelope(object, status, &code, operation, reported_request_id) == 0 {
        return None;
    }
    serde_json::to_vec(&document).ok()
}

/// The response with MediaConvert's envelope completed in its error body.
///
/// A response that is not a failure, or whose body is not an error body this service owns
/// (see [`with_envelope_members`]), is returned untouched — status, headers and every byte
/// of the body exactly as the renderer wrote them.
pub fn with_error_envelope(mut resp: RtResponse, operation: &str) -> RtResponse {
    let status = resp.status().as_u16();
    if status < 400 {
        return resp;
    }
    let header_code = resp.headers().get(ERROR_TYPE_HEADER).map(str::to_string);
    let reported_request_id = REQUEST_ID_HEADERS
        .iter()
        .find_map(|header| resp.headers().get(header))
        .map(str::to_string);
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
            operation,
            reported_request_id.as_deref(),
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
/// crate's existing `sha2`/`hex` dependencies already provide. The counter alone makes two
/// ids minted in the same nanosecond differ.
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

/// The operation the generated routing table names for this request, with the routing key
/// that named it — which, on this service's restJson1 protocol, is the operation's own
/// shape name and so the value the envelope's `operation` member carries.
fn modeled_operation<'s>(
    svc: &'s dyn Service,
    req: &'s RtRequest,
) -> Option<(&'s str, &'s dyn ErasedOperation)> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    let op = svc.definition().routing_table().get(key)?;
    Some((key, op))
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
/// then completes the error envelope of the response.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// code and message it answers with; this wrapper owns the envelope's six members.
pub struct ErrorEnvelopeOperation;

impl ErasedOperation for ErrorEnvelopeOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some((operation, op)) => {
                    with_error_envelope(op.run(svc, req, ctx).await, operation)
                }
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
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

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: Vec<u8>) -> Map<String, Value> {
        serde_json::from_slice::<Value>(&bytes)
            .expect("the rewritten body is JSON")
            .as_object()
            .expect("the rewritten body is a JSON object")
            .clone()
    }

    /// One modelled exception per row — every code the model declares, at the status it
    /// declares — so the envelope is not keyed to the one the issue recorded.
    #[test]
    fn every_modelled_exception_gets_the_envelope_for_the_operation_being_served() {
        for (code, status, operation) in [
            ("NotFoundException", 404u16, "GetJob"),
            ("NotFoundException", 404, "GetQueue"),
            ("BadRequestException", 400, "CreateJob"),
            ("ConflictException", 409, "DeleteQueue"),
            ("ForbiddenException", 403, "UpdateJobTemplate"),
            ("ServiceQuotaExceededException", 402, "CreateQueue"),
            ("TooManyRequestsException", 429, "ListJobs"),
            ("InternalServerErrorException", 500, "GetPreset"),
        ] {
            let body = format!(r#"{{"__type":"{code}","message":"no"}}"#);
            let rewritten = with_envelope_members(body.as_bytes(), status, None, operation, None)
                .unwrap_or_else(|| panic!("case {code:?}/{operation:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get("errorType").and_then(Value::as_str),
                Some(code),
                "case {code:?}/{operation:?}: expected errorType {code:?}, got {object:?}"
            );
            assert_eq!(
                object.get("httpStatus").and_then(Value::as_u64),
                Some(u64::from(status)),
                "case {code:?}/{operation:?}: expected httpStatus {status}, got {object:?}"
            );
            assert_eq!(
                object.get("operation").and_then(Value::as_str),
                Some(operation),
                "case {code:?}/{operation:?}: expected operation {operation:?}, got {object:?}"
            );
            assert_eq!(
                object.get("accountId").and_then(Value::as_str),
                Some(::engine::emulator_account_id()),
                "case {code:?}/{operation:?}: expected the emulator account, got {object:?}"
            );
            assert_eq!(
                object.get("settingsValidationErrorsJsonBlob").and_then(Value::as_str),
                Some(NO_SETTINGS_VALIDATION_ERRORS),
                "case {code:?}/{operation:?}: expected an empty validation blob, got {object:?}"
            );
            assert_eq!(
                object.get("message").and_then(Value::as_str),
                Some("no"),
                "case {code:?}/{operation:?}: the message must survive, got {object:?}"
            );
        }
    }

    /// The code may arrive namespaced, or with a trailing locator, or only in the
    /// `x-amzn-errortype` header — every form names the same shape.
    #[test]
    fn the_error_code_is_read_whichever_form_it_arrives_in() {
        for (label, body, header) in [
            ("bare __type", r#"{"__type":"NotFoundException"}"#, None),
            (
                "namespaced __type",
                r#"{"__type":"com.amazonaws.mediaconvert#NotFoundException"}"#,
                None,
            ),
            (
                "__type with a locator",
                r#"{"__type":"NotFoundException:http://internal.amazon.com/coral/"}"#,
                None,
            ),
            ("header only", r#"{"message":"no"}"#, Some("NotFoundException")),
        ] {
            let rewritten = with_envelope_members(body.as_bytes(), 404, header, "GetJob", None)
                .unwrap_or_else(|| panic!("case {label:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get("errorType").and_then(Value::as_str),
                Some("NotFoundException"),
                "case {label:?}: expected the bare shape name, got {object:?}"
            );
        }
    }

    /// Nothing to add: a body the framework synthesised for a signal MediaConvert has no
    /// error for, a body carrying no code at all, one that is not a JSON object, and one
    /// that already carries the envelope, are all left alone.
    #[test]
    fn nothing_is_added_to_a_body_this_service_does_not_own() {
        let already = r#"{"__type":"NotFoundException","message":"no","errorType":"NotFoundException","httpStatus":404,"accountId":"000000000000","requestId":"kept","operation":"GetJob","settingsValidationErrorsJsonBlob":""}"#;
        for (label, body) in [
            ("the scaffold's 501", r#"{"__type":"NotImplemented","message":"no"}"#),
            ("a framework code", r#"{"__type":"SerializationException","message":"no"}"#),
            ("no code at all", r#"{"message":"no"}"#),
            ("not an object", r#"["NotFoundException"]"#),
            ("not JSON", "<Error/>"),
            ("already enveloped", already),
        ] {
            assert!(
                with_envelope_members(body.as_bytes(), 404, None, "GetJob", None).is_none(),
                "case {label:?}: expected no rewrite"
            );
        }
    }

    /// A member the body already carries wins: the envelope completes a body, it does not
    /// overwrite it.
    #[test]
    fn a_member_the_body_already_carries_is_kept() {
        let body = r#"{"__type":"NotFoundException","requestId":"kept","httpStatus":499}"#;
        let object = decoded(
            with_envelope_members(body.as_bytes(), 404, None, "GetJob", Some("reported"))
                .expect("a rewrite"),
        );
        assert_eq!(
            object.get("requestId").and_then(Value::as_str),
            Some("kept"),
            "the body's own requestId must survive, got {object:?}"
        );
        assert_eq!(
            object.get("httpStatus").and_then(Value::as_u64),
            Some(499),
            "the body's own httpStatus must survive, got {object:?}"
        );
    }

    /// A successful response is not an error body: the envelope never reaches it.
    #[test]
    fn a_success_is_returned_untouched() {
        let body = r#"{"queue":{"name":"Default"}}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_error_envelope(resp, "GetQueue");
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
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
            let answered = with_error_envelope(resp, "GetJob");
            let object = decoded(answered.body().bytes().expect("bytes").to_vec());
            assert_eq!(
                object.get("requestId").and_then(Value::as_str),
                Some("reported-by-the-response"),
                "case {header:?}: the body must carry the reported id, got {object:?}"
            );
        }

        // No header: the body still carries one, minted for this response.
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(404u16).expect("404"),
            SdkBody::from(body),
        );
        let object = decoded(
            with_error_envelope(resp, "GetJob")
                .body()
                .bytes()
                .expect("bytes")
                .to_vec(),
        );
        assert!(
            object
                .get("requestId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case no header: expected a minted requestId, got {object:?}"
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
}
