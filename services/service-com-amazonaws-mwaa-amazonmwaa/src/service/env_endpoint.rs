// SPDX-License-Identifier: CC0-1.0

//! The refusal payload of the environment-scoped data endpoint, for the one operation
//! this service serves THROUGH that endpoint's Apache Airflow proxy.
//!
//! # What the endpoint answers
//!
//! `InvokeRestApi` is the only operation the model declares as a PROXY: it "invokes the
//! Apache Airflow REST API on the webserver" of the environment the request names, and its
//! output is a document (`RestApiResponse`) carrying whatever that web server answered. Its
//! `@endpoint` host prefix is `env.`, the per-environment data endpoint, so a request that
//! names no stored environment is refused by the front door that resolves the request to a
//! web server — ahead of the code that owns this service's modelled exceptions.
//!
//! A refusal from that front door is NOT a serialized modelled exception. Its body's ROOT
//! is the endpoint's own document — a JSON string — and it carries no error-code member
//! (`__type`), while the status and the `x-amzn-errortype` header a restJson1 client
//! resolves the error shape from stay the modelled ones (404 /
//! `ResourceNotFoundException`):
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//! x-amzn-errortype: ResourceNotFoundException
//!
//! "Environment not found"
//! ```
//!
//! A client therefore sees the not-found CODE where it dispatches on it, but a body it must
//! read as a document rather than as an exception object — which is exactly what a caller of
//! a proxying operation does with every other answer of that operation.
//!
//! # Why a wrapper and not the handler
//!
//! A handler returns a typed error and the framework serializes it; there is no seam for a
//! handler to hand back a body of its own. So the operation keeps returning its modelled
//! `ResourceNotFoundException` — the framework still owns the status, the wire code, the
//! header and the telemetry classification — and this wrapper reshapes only the BODY of that
//! one answer, from the serialized exception object into the endpoint's document payload.
//!
//! # What is deliberately left alone
//!
//! * Every other answer of `InvokeRestApi`: the scaffold's 501 for an environment that
//!   exists (no Apache Airflow runs in this process), the framework's 400 refusal of an
//!   input outside its modelled constraints, and any success.
//! * The control-plane operations, including the other two `env.`-host-prefixed ones
//!   (`CreateCliToken`, `CreateWebLoginToken`). Those mint a token in the service itself
//!   rather than proxying a request to an environment's web server, and the recorded
//!   behaviour this emulator is held to shows their refusal arriving as the modelled
//!   exception object (`issues/things-that-now-work/20261022T022901Z-mwaa-control-plane-divergences-createweblogintoken-invokerestapi-answer-501.md`,
//!   cause 1), so nothing here touches them.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The operation served through the environment's Apache Airflow proxy — the one whose
/// refusal the data endpoint's front door words, rather than this service's modelled-error
/// serializer.
///
/// Source: the Smithy model's own `@endpoint`/doc for this operation
/// (`smithy_query(query="operation",operation="InvokeRestApi",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`
/// — host prefix `env.`, output `RestApiResponse: document`).
const PROXIED_OPERATION: &str = "InvokeRestApi";

/// The status the model declares for this service's not-found class, and so the only status
/// whose body this module reshapes.
const NOT_FOUND_STATUS: u16 = 404;

/// The modelled not-found shape of the proxied operation. Only a body naming THIS code is
/// reshaped: a 404 that named anything else would not be the environment lookup failing.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The body members an error-type discriminator can arrive in on the JSON protocols:
/// `__type` is what this emulator's restJson1 renderer writes, `code` / `Code` are the other
/// spellings the same parsers accept.
const DISCRIMINATOR_MEMBERS: [&str; 3] = ["__type", "code", "Code"];

/// The bare shape name a discriminator value names: a value may arrive namespaced
/// (`com.amazonaws.mwaa#ResourceNotFoundException`) or with a trailing locator
/// (`ResourceNotFoundException:http://…`), and neither is part of the code.
fn shape_name(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace.split(':').next().unwrap_or(after_namespace)
}

/// The serialized modelled exception rewritten as the data endpoint's own payload: the
/// exception's `message` as the body's ROOT, a JSON string, with no error-code member left
/// anywhere in it.
///
/// `None` when there is nothing to reshape — the body is not a JSON object, names no
/// discriminator, or names a code that is not the proxied operation's not-found class — and
/// then the body is left to its renderer byte for byte.
fn document_payload(body: &[u8]) -> Option<Vec<u8>> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object()?;
    let named = DISCRIMINATOR_MEMBERS
        .iter()
        .find_map(|member| object.get(*member).and_then(Value::as_str))?;
    if shape_name(named) != RESOURCE_NOT_FOUND_EXCEPTION {
        return None;
    }
    let message = object
        .get("message")
        .or_else(|| object.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    serde_json::to_vec(&Value::String(message.to_string())).ok()
}

/// The answer as the environment's data endpoint gives it: a not-found body carried as the
/// endpoint's document payload instead of as a serialized exception object.
///
/// Everything else is left exactly as the framework rendered it — the status, the
/// `x-amzn-errortype` header the error code travels in, every other header, and every
/// answer that is not this operation's not-found refusal.
pub fn bare_document(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() != NOT_FOUND_STATUS {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable here;
        // a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match document_payload(bytes) {
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

/// The operation the generated routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Whether this request is the one served through the environment's Apache Airflow proxy —
/// the condition under which this service claims it for [`EnvDataEndpoint`].
pub fn proxies_the_airflow_rest_api<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .is_ok_and(|key| key == PROXIED_OPERATION)
}

/// Serves the proxied operation through the framework exactly as the routing table would,
/// then answers its not-found refusal with the data endpoint's own payload.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk — deserialize,
/// validate, handle, serialize — and every status, code and header it answers with; this
/// wrapper owns only the SHAPE of that one body.
pub struct EnvDataEndpoint;

impl ErasedOperation for EnvDataEndpoint {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some(op) => bare_document(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a request
                // only when the routing key names the proxied operation, which the table
                // holds; answered as the framework answers an unroutable request rather
                // than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the operation
        // this wrapper delegates to — so its model-declared schemes decide and this wrapper
        // cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    //! What the reshaping does to a body, over the forms a body arrives in. The end-to-end
    //! behaviour of the operation is pinned by the crate's integration tests
    //! (`tests/all/env_endpoint_payload.rs`).

    use super::*;

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("the rewritten body is JSON")
    }

    /// A 404 response carrying `body`, with the header the renderer sets beside it.
    fn not_found(body: &str) -> RtResponse {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(NOT_FOUND_STATUS).expect("404"),
            SdkBody::from(body),
        );
        resp.headers_mut()
            .insert("x-amzn-errortype", RESOURCE_NOT_FOUND_EXCEPTION);
        resp
    }

    /// The not-found body becomes a root-level JSON string carrying the message, whichever
    /// spelling the discriminator and the message member arrive in.
    #[test]
    fn the_not_found_body_becomes_a_root_level_string() {
        for (case, body, expected) in [
            (
                "__type and message",
                r#"{"__type":"ResourceNotFoundException","message":"Environment not found"}"#,
                "Environment not found",
            ),
            (
                "namespaced __type",
                r#"{"__type":"com.amazonaws.mwaa#ResourceNotFoundException","message":"a"}"#,
                "a",
            ),
            (
                "a discriminator with a locator",
                r#"{"__type":"ResourceNotFoundException:http://internal/","Message":"b"}"#,
                "b",
            ),
            (
                "the Code spelling",
                r#"{"Code":"ResourceNotFoundException","message":"c"}"#,
                "c",
            ),
            (
                "no message member at all",
                r#"{"__type":"ResourceNotFoundException"}"#,
                "",
            ),
        ] {
            let rewritten = document_payload(body.as_bytes())
                .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite of {body}"));
            let document = decoded(&rewritten);
            assert_eq!(
                document,
                Value::String(expected.to_string()),
                "case {case:?}: expected the root to be the string {expected:?}, got {document:?}"
            );
        }
    }

    /// Nothing to reshape: an answer that is not this operation's not-found refusal keeps
    /// every byte its renderer wrote.
    #[test]
    fn any_other_answer_is_left_byte_for_byte() {
        for (case, body) in [
            (
                "the scaffold's 501 body",
                r#"{"__type":"NotImplemented","message":"no Airflow"}"#,
            ),
            (
                "a framework refusal",
                r#"{"__type":"ValidationException","message":"bad"}"#,
            ),
            (
                "the tagging operations' wire code",
                r#"{"__type":"NotFoundException","message":"Resource was not found"}"#,
            ),
            ("a body naming no code", r#"{"message":"unlabelled"}"#),
            ("a root-level string already", r#""Environment not found""#),
            ("a body that is not JSON", "<Error/>"),
            ("an empty body", ""),
        ] {
            assert!(
                document_payload(body.as_bytes()).is_none(),
                "case {case:?}: expected no rewrite"
            );
        }

        // And on the response: a status that is not the not-found one is untouched even
        // when its body names the not-found code.
        let exception = r#"{"__type":"ResourceNotFoundException","message":"x"}"#;
        let mut other = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(501u16).expect("501"),
            SdkBody::from(exception),
        );
        other.headers_mut().insert("content-type", "application/json");
        let answered = bare_document(other);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            exception.as_bytes(),
            "a status that is not {NOT_FOUND_STATUS} must keep its body"
        );
    }

    /// End to end on a response: the status and the error-code header stay, and the body
    /// carries no error-code member any more.
    #[test]
    fn the_status_and_the_error_code_header_survive_the_reshaping() {
        let answered = not_found(
            r#"{"__type":"ResourceNotFoundException","message":"Environment not found"}"#,
        );
        let answered = bare_document(answered);
        assert_eq!(
            answered.status().as_u16(),
            NOT_FOUND_STATUS,
            "the status must not move, got {}",
            answered.status().as_u16()
        );
        assert_eq!(
            answered.headers().get("x-amzn-errortype"),
            Some(RESOURCE_NOT_FOUND_EXCEPTION),
            "restJson1 resolves the shape from the header, got {:?}",
            answered.headers().get("x-amzn-errortype")
        );
        let document = decoded(answered.body().bytes().expect("bytes"));
        assert!(
            document.is_string(),
            "the payload's root must be a string, got {document:?}"
        );
        assert!(
            DISCRIMINATOR_MEMBERS
                .iter()
                .all(|member| document.get(*member).is_none()),
            "no error-code member may survive, got {document:?}"
        );
    }
}
