// SPDX-License-Identifier: CC0-1.0

//! How this service names, in the body, the exception a refused call is refused with.
//!
//! Every failure Internet Monitor answers states the exception shape it refuses with in
//! its JSON body, as the `code` member beside `message`:
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//!
//! {"code":"ResourceNotFoundException","message":"No resource found for account …"}
//! ```
//!
//! `code` belongs to no modelled shape: every exception this service's model declares
//! carries exactly one member, `message` —
//! `smithy_query(query="shape",shape="ValidationException,ResourceNotFoundException,ConflictException",model_path="services/service-com-amazonaws-internetmonitor-internetmonitor20210603/smithy-model/com-amazonaws-internetmonitor-internetmonitor20210603.json")`.
//! It is a member of the error ENVELOPE, so no error serializer driven by a shape's
//! schema can write it: the framework's restJson1 renderer writes `__type` plus the error
//! shape's own members and stops, which leaves the body with no member a client reading
//! the error type out of it can branch on.
//!
//! This module completes that envelope, through [`ErrorCodeOperation`] — the wrapper the
//! service hands back from `Service::override_operation`. It serves the modelled operation
//! the generated routing table names for the request, so the framework still owns the
//! whole deserialize → validate → handle → serialize sequence and every status, shape and
//! message it answers with, and then adds `code` to an error body that names one of this
//! service's own modelled exceptions, whichever layer produced it — a handler's modelled
//! refusal or the boundary validator's.
//!
//! The value follows the response, never a recording: `code` is the bare name of the
//! exception shape the response already names, read from the body's `__type` and from the
//! `x-amzn-errortype` header otherwise.
//!
//! What is deliberately left alone: the HTTP status, `__type`, `message`, every other
//! member and header, and the whole success path stay with the renderers that own them;
//! and a body whose code is NOT one of this service's modelled exceptions — the scaffold's
//! 501 `NotImplemented`, a framework `SerializationException` — is left exactly as the
//! framework wrote it rather than dressed up as an Internet Monitor exception.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::{Map, Value};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Every exception shape this service's model declares — the codes an Internet Monitor
/// error body can name, and so the ones this module states in `code`.
///
/// Source: the `errors` lists of this service's operations plus its service-common error,
/// as rendered by
/// `smithy_query(query="digest",model_path="services/service-com-amazonaws-internetmonitor-internetmonitor20210603/smithy-model/com-amazonaws-internetmonitor-internetmonitor20210603.json")`
/// — `AccessDeniedException` (403), `BadRequestException` (400), `ConflictException` (409),
/// `InternalServerErrorException` (500), `InternalServerException` (500),
/// `LimitExceededException` (403), `NotFoundException` (404),
/// `ResourceNotFoundException` (404), `ThrottlingException` (429),
/// `TooManyRequestsException` (429) and `ValidationException` (400). These eleven are the
/// whole set: no operation of this service declares any other error.
///
/// A code the framework synthesises for a signal Internet Monitor has no error for — the
/// scaffold's `NotImplemented` (501), answered for an operation nobody has written yet —
/// is not one of this service's exceptions, so its body is left as the framework wrote it.
pub const MODELLED_EXCEPTIONS: [&str; 11] = [
    "AccessDeniedException",
    "BadRequestException",
    "ConflictException",
    "InternalServerErrorException",
    "InternalServerException",
    "LimitExceededException",
    "NotFoundException",
    "ResourceNotFoundException",
    "ThrottlingException",
    "TooManyRequestsException",
    "ValidationException",
];

/// The response header restJson1 resolves an error shape from, and the fallback source of
/// the error code when a body carries no `__type`.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The body member the framework's JSON error renderer writes the error shape into.
const TYPE_MEMBER: &str = "__type";

/// The body member this service states the exception's name in.
const CODE_MEMBER: &str = "code";

/// The lowest HTTP status that is a failure, and so the lowest one that carries an error
/// body at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// The bare shape name a discriminator value names.
///
/// A value may arrive namespaced (`com.amazonaws.internetmonitor#ValidationException`) or
/// with a trailing locator (`ValidationException:http://…`); neither is part of the code.
fn shape_name(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace.split(':').next().unwrap_or(after_namespace)
}

/// The error code a response names, reduced to the bare shape name.
///
/// Read from the body's `__type` first, since that is what the error renderers write, and
/// from the `x-amzn-errortype` header otherwise.
fn error_code(object: &Map<String, Value>, header: Option<&str>) -> Option<String> {
    let raw = object.get(TYPE_MEMBER).and_then(Value::as_str).or(header)?;
    let code = shape_name(raw);
    (!code.is_empty()).then(|| code.to_string())
}

/// The error body with the `code` member added, or `None` when there is nothing to add:
/// the body is not a JSON object, it names no error code, the code is not one of this
/// service's modelled exceptions, or the member is already there.
///
/// Adding is idempotent, and a member the error value itself stated always wins: a body
/// that already carries `code` is left to its renderer byte for byte.
fn with_code_member(body: &[u8], header_code: Option<&str>) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let code = error_code(object, header_code)?;
    if !MODELLED_EXCEPTIONS.contains(&code.as_str()) || object.contains_key(CODE_MEMBER) {
        return None;
    }
    object.insert(CODE_MEMBER.to_string(), Value::from(code));
    serde_json::to_vec(&document).ok()
}

/// The response as Internet Monitor answers it: an error body of one of this service's
/// modelled exceptions naming that exception in its `code` member.
///
/// A response that is not a failure keeps every byte of its body; so does a failure whose
/// error body this service does not own (see [`with_code_member`]).
pub fn with_error_code(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS {
        return resp;
    }
    let header_code = resp.headers().get(ERROR_TYPE_HEADER).map(str::to_string);
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_code_member(bytes, header_code.as_deref()) {
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

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ErrorCodeOperation`].
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
/// then states the exception's name in the `code` member of whatever error body it
/// answered.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// shape and message it answers with; this wrapper owns only the one envelope member.
pub struct ErrorCodeOperation;

impl ErasedOperation for ErrorCodeOperation {
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
                Some(op) => with_error_code(op.run(svc, req, ctx).await),
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

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: Vec<u8>) -> Map<String, Value> {
        serde_json::from_slice::<Value>(&bytes)
            .expect("the rewritten body is JSON")
            .as_object()
            .expect("the rewritten body is a JSON object")
            .clone()
    }

    /// One row per exception the model declares, so the member is not keyed to the shapes
    /// of one recorded refusal: each gets its own name in `code`, and its `message`
    /// survives untouched.
    #[test]
    fn every_modelled_exception_names_itself_in_the_code_member() {
        for code in MODELLED_EXCEPTIONS {
            let body = format!(r#"{{"{TYPE_MEMBER}":"{code}","message":"refused"}}"#);
            let rewritten = with_code_member(body.as_bytes(), None)
                .unwrap_or_else(|| panic!("case {code:?}: expected a rewrite"));
            let document = decoded(rewritten);
            assert_eq!(
                document.get(CODE_MEMBER).and_then(Value::as_str),
                Some(code),
                "case {code:?}: expected the code member to name the shape, got {document:?}"
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some("refused"),
                "case {code:?}: the message must survive, got {document:?}"
            );
            assert_eq!(
                document.get(TYPE_MEMBER).and_then(Value::as_str),
                Some(code),
                "case {code:?}: the discriminator must survive, got {document:?}"
            );
        }
    }

    /// The code is recognised whichever form the response names it in: bare, namespaced,
    /// with a trailing locator, or in the header alone when the body carries no `__type`.
    #[test]
    fn the_code_is_recognised_in_every_form_it_arrives_in() {
        for (case, body, header) in [
            ("bare", r#"{"__type":"ValidationException"}"#, None),
            (
                "namespaced",
                r#"{"__type":"com.amazonaws.internetmonitor#ValidationException"}"#,
                None,
            ),
            (
                "with a locator",
                r#"{"__type":"ValidationException:http://internal.amazon.com/coral/"}"#,
                None,
            ),
            (
                "from the header alone",
                r#"{"message":"refused"}"#,
                Some("ValidationException"),
            ),
        ] {
            let rewritten = with_code_member(body.as_bytes(), header)
                .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
            let document = decoded(rewritten);
            assert_eq!(
                document.get(CODE_MEMBER).and_then(Value::as_str),
                Some("ValidationException"),
                "case {case:?}: expected the bare shape name in the code member, got \
                 {document:?}"
            );
        }
    }

    /// Nothing to add: a code this service does not model, a body naming no code at all, a
    /// body that already carries the member, and a body that is not a JSON object are all
    /// left to their renderer.
    #[test]
    fn a_body_this_service_does_not_own_is_left_alone() {
        for (case, body) in [
            (
                "the scaffold's 501",
                r#"{"__type":"NotImplemented","message":"no"}"#,
            ),
            ("no code at all", r#"{"message":"no"}"#),
            (
                "the member already stated",
                r#"{"__type":"ValidationException","code":"ValidationException"}"#,
            ),
            ("not an object", r#"["ValidationException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                with_code_member(body.as_bytes(), None).is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// End to end on a response: the status and the headers stay, the error body gains the
    /// member, and a success is byte-identical.
    #[test]
    fn the_response_keeps_its_status_and_gains_only_the_code_member() {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(409u16).expect("409"),
            SdkBody::from(r#"{"__type":"ConflictException","message":"in use"}"#),
        );
        resp.headers_mut()
            .insert(ERROR_TYPE_HEADER, "ConflictException");
        let answered = with_error_code(resp);
        assert_eq!(
            answered.status().as_u16(),
            409,
            "the status must not move, got {}",
            answered.status().as_u16()
        );
        assert_eq!(
            answered.headers().get(ERROR_TYPE_HEADER),
            Some("ConflictException"),
            "the error-type header must survive, got {:?}",
            answered.headers().get(ERROR_TYPE_HEADER)
        );
        let document = decoded(answered.body().bytes().expect("bytes").to_vec());
        assert_eq!(
            document.get(CODE_MEMBER).and_then(Value::as_str),
            Some("ConflictException"),
            "the body must name the shape in its code member, got {document:?}"
        );

        let success = r#"{"MonitorName":"kept"}"#;
        let answered = with_error_code(RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(success),
        ));
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            success.as_bytes(),
            "a success body must be byte-identical"
        );
    }
}
